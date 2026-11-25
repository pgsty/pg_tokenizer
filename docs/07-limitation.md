# Operational Notes and Limitations

## Backend-Local Definition Caches

Each PostgreSQL backend caches compiled text analyzers, tokenizers, models, stopword lists, and synonym lists in memory and validates them against the single-row `tokenizer_catalog.definition_version` counter, a shared invalidation epoch, and a `FullTransactionId` change barrier.

- **Snapshot consistency**: A BEFORE STATEMENT trigger marks the writer’s local cache before row triggers run. After the statement succeeds, an AFTER STATEMENT trigger increments `definition_version` and advances the shared epoch and transaction barrier. Once a primary backend has validated its cache against a snapshot strictly newer than the latest change barrier, subsequent primary reads avoid per-statement SPI and `pg_depend` probes while preserving MVCC visibility. Transactions with older or imported snapshots (`SET TRANSACTION SNAPSHOT`), uncommitted local catalog writes, physical standbys, or un-upgraded pre-1.3.0 SQL fall back to reading `definition_version` under the caller's active SQL snapshot with a cached read-only SPI plan. `READ COMMITTED` sessions observe committed changes on their next statement; `REPEATABLE READ` transactions retain their snapshot's definitions until transaction end.
- **Rollback safety**: Aborted transactions and subtransactions that modified `tokenizer_catalog` definitions roll back catalog rows and clear backend-local caches automatically. Unrelated `ROLLBACK` and `ROLLBACK TO SAVEPOINT` statements in transactions that did not modify tokenizer definitions preserve already loaded models and analyzers:

```sql
BEGIN;
SELECT create_text_analyzer('text_analyzer1', $$
pre_tokenizer = "unicode_segmentation"
$$);
SELECT apply_text_analyzer('hello', 'text_analyzer1');
ROLLBACK;

SELECT apply_text_analyzer('hello', 'text_analyzer1'); -- raises "not found"
```

- **Physical standbys**: `definition_version` replicates over WAL, so hot-standby sessions read `definition_version` under their active snapshot and refresh cached definitions once replay passes the commit LSN and the query uses a new snapshot, including across promotion.
- **Trigger-bypass maintenance boundary**: Normal API calls and direct DML on `tokenizer_catalog` tables fire the extension's `AFTER STATEMENT` triggers automatically. If a superuser deliberately bypasses those triggers (for example via `ALTER TABLE ... DISABLE TRIGGER` or `SET session_replication_role = replica`) while mutating `tokenizer_catalog` tables, backends on the primary fast path will not be notified until a trigger fires. After such maintenance, re-enable triggers and restore `session_replication_role = origin`, perform a committed no-op update on one definition table (such as `UPDATE tokenizer_catalog.stopwords SET config = config WHERE name = 'lucene_english'`) so both `definition_version` and the shared primary barrier advance, and wait for standby WAL replay before resuming traffic.
- **Two-phase commit.** Once a definition-changing API call has modified the catalog, its outer transaction rejects `PREPARE TRANSACTION`, even after `ROLLBACK TO SAVEPOINT` undoes that change. End the transaction with `COMMIT` or `ROLLBACK` before starting work that needs two-phase commit. Read-only transactions can be prepared without evicting unchanged local caches; direct-DML prepared transactions keep concurrent readers on the snapshot-checked `definition_version` path until `COMMIT PREPARED` or `ROLLBACK PREPARED`.
- **Persisted outputs and volatility**: `apply_text_analyzer`, `tokenize`, `analyze_to_tsvector`, and `tokenize_to_tsvector` are `STABLE` (not `IMMUTABLE`) because definitions reside in mutable catalog tables. Store their outputs in ordinary table columns maintained by triggers or application writes rather than expression indexes or stored generated columns. Replacing a definition does not rewrite existing rows; recompute stored columns and rebuild indexes explicitly when tokenization output changes.

## Cancellation and Resource Use

Session-settable GUC parameters bound stage inputs and outputs, raising SQLSTATE `54000` (`program_limit_exceeded`) rather than silently truncating results:

| Setting | Default | Scope |
| --- | ---: | --- |
| `pg_tokenizer.max_input_bytes` | `16,777,216` (16 MiB) | UTF-8 bytes admitted to one analyzer call |
| `pg_tokenizer.max_output_tokens` | `1,000,000` | Tokens or model IDs produced in one stage |
| `pg_tokenizer.max_output_bytes` | `67,108,864` (64 MiB) | Token UTF-8 bytes plus descriptors, ID bytes, character output, or n-gram offsets in one stage |
| `pg_tokenizer.max_piece_bytes` | `1,048,576` (1 MiB) | UTF-8 bytes passed to a single regex, Jieba, Lindera, Hugging Face, `wiki_tocken`, or `pg_dict` call |

- **Accounting scope**: These settings enforce per-stage admission and buffer limits, not a hard backend RSS cap. Detoasted PostgreSQL inputs, cached models (for example, `gemma2b` ~100 MiB and `llmlingua2` ~200 MiB; use `add_preload_model` to share them across backends), and internal allocations inside third-party tokenizers add memory outside stage accounting. `max_input_bytes` (16 MiB) bounds the whole analyzer input, while `max_piece_bytes` (1 MiB) bounds a single uninterruptible call into an opaque tokenizer, dictionary, or regex engine; keep the 1 MiB default for online workloads and raise it with `SET LOCAL` only for trusted batch loads. Use trusted Hugging Face model configurations and bound untrusted document sizes in the application.
- **Interrupt checks**: Query cancellation and `statement_timeout` are checked between pipeline stages, per token in model and `pg_dict` batches, and every 1,024 iterations in character, lightweight token filter, n-gram, and `tsvector` loops. A single long call inside an external regex or C/Rust dictionary routine completes before the next interrupt check runs.

## PostgreSQL Dictionaries and Privileges

- **`pg_dict` resolution**: `create_text_analyzer` and `create_tokenizer` resolve `pg_dict` against the creator's `search_path` and store the schema-qualified dictionary name. Each batch resolves the qualified name through PostgreSQL's syscache so `DROP`/`CREATE` and `ALTER TEXT SEARCH DICTIONARY` take effect without reconnecting; renaming or moving a dictionary to another schema requires recreating the analyzer.
- **Catalog privileges**: Catalog-modifying functions are `PARALLEL UNSAFE`; read-only tokenization functions are `PARALLEL SAFE`. Custom vocabulary tables and sequences reside in `tokenizer_data`, keeping `CREATE` privileges off `tokenizer_catalog`.

## Logical Backups

`text_analyzer`, `tokenizer`, `model`, `stopwords` (excluding the three built-in lists recreated by `CREATE EXTENSION`), and `synonym` are registered with `pg_extension_config_dump`. Standard `pg_dump` and `pg_restore` preserve user definitions alongside `tokenizer_data` custom vocabulary tables, identity sequences, and source-table triggers.

External Lindera dictionary files and `$PGDATA/pg_tokenizer/preload_models` live on the server filesystem outside PostgreSQL catalogs and must be backed up separately (and configured on each standby node when preloading models on replicas).
