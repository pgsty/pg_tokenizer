# Limitation

## Backend-local definition caches

Since 1.1.0, `pg_tokenizer` validates its backend-local analyzer, tokenizer, model, stopword and synonym caches against a one-row `tokenizer_catalog.definition_version` table. Statement triggers update the counter in the same transaction as the definition. The counter and definitions are read through the caller's SQL snapshot, including in transactions that have already written data. A later statement or transaction checks again; a running statement and a REPEATABLE READ transaction retain their snapshot's definition. Rollbacks and subtransaction aborts do not publish a changed version.

Rollback cleanup is automatic, including when a subtransaction aborts while its outer transaction continues:

```sql
BEGIN;
SELECT create_text_analyzer('text_analyzer1', $$
pre_tokenizer = "unicode_segmentation"
$$);
SELECT apply_text_analyzer('hello', 'text_analyzer1');
ROLLBACK;

-- The catalog row and the backend-local object are both gone.
SELECT apply_text_analyzer('hello', 'text_analyzer1'); -- raises "not found"
```

If a tokenizer references an analyzer or model that is rebuilt under the same name, the dependent tokenizer cache is invalidated as well. Existing persisted embeddings or `tsvector` values are not rewritten automatically; recompute them when the definition changes.


Physical standbys use the same WAL-replicated counter. After replay reaches a definition's commit, a long-lived standby session sees it on its next eligible snapshot. Promotion retains this behavior without reconnecting. Queries can still see old definitions while replay is paused or behind, or while using an old transaction snapshot. Before generating query vectors that must match a primary-side change, wait for its commit LSN to replay and start a new snapshot.

The version read is shared by repeated lookups within one statement; it does not scan definition tables or run once per token. Definition changes serialize on the counter row until transaction end and write one additional heap update plus WAL per modifying statement. Ordinary custom-vocabulary inserts do not update the counter. Keep definition-changing transactions short. Direct DML now invalidates caches too, but still bypasses API configuration validation and remains an administrative operation.

At `PREPARE TRANSACTION`, the preparing backend clears its derived objects and validation state. Later queries reload definitions under their own snapshots, including after another connection rolls back the prepared transaction and a different definition reuses its version number. Read-only transactions can be prepared; definition-changing API calls still reject `PREPARE TRANSACTION`. Older installed SQL versions use the original, primary-only shared-memory invalidation until `ALTER EXTENSION ... UPDATE` installs the new table and triggers.

`apply_text_analyzer`, `tokenize`, and the native `tsvector` wrappers are `STABLE`, not immutable. Keep their output in ordinary columns maintained explicitly or by triggers. Do not use immutable wrappers to bypass PostgreSQL's expression-index/generated-column checks.

## Cancellation and resource use

The following positive, session-settable limits are available in 1.1.0. Exceeding one raises SQLSTATE `54000`; output is never silently truncated.

| Setting | Default | Scope |
| --- | ---: | --- |
| `pg_tokenizer.max_input_bytes` | 16,777,216 | UTF-8 bytes admitted to one analyzer call |
| `pg_tokenizer.max_output_tokens` | 1,000,000 | Tokens or model IDs in one stage |
| `pg_tokenizer.max_output_bytes` | 67,108,864 | Token UTF-8 bytes plus String descriptors, or ID bytes, in one stage; also character output and ngram offsets |
| `pg_tokenizer.max_piece_bytes` | 1,048,576 | Input to one regex, Jieba, Lindera, Hugging Face, wiki_tocken or PostgreSQL dictionary call |

Checks run before allocating owned ngrams, normalized character output, or copying pre-tokenizer results; aggregate checks stop successive filters from repeatedly expanding a batch. HF fixed padding and padding multiples are checked before encoding. Ngram counts for a token of `L` characters are `sum(L - g + 1)` for `min_gram <= g <= min(max_gram, L)`, plus the preserved original when required. Both token count and byte accounting constrain expansion, including Unicode and chains of filters.

These are admission and stage budgets, **not a hard backend RSS limit**. PostgreSQL has already detoasted input before entry; allocator capacity, simultaneous input/output buffers, cached models, and third-party internal workspace add memory. Arbitrary HF normalizers, post-processors and truncation overflow can allocate before their output is checked. Their internal expansion and regex work remain a limitation; load trusted model configurations and bound documents in the application. Raising limits increases memory exposure. The extension does not alter HF truncation settings or split input in ways that change token IDs.

Interrupt checks run between stages, at token boundaries, and every 1,024 iterations in bulk loops (including regex matches and Unicode output). A single regex search or opaque library/dictionary call can still delay a timeout until it returns. No asynchronous worker or unsafe cancellation is introduced.

## Dictionary names and schema privileges

New `pg_dict` definitions store the schema-qualified dictionary resolved by their creator. Each batch resolves that name again using PostgreSQL's namespace cache: DROP/CREATE finds the new OID, and PostgreSQL's dictionary cache handles ALTER. Renaming or moving a referenced dictionary requires updating the definition, like any missing qualified name. Changes to external dictionary files still require the normal PostgreSQL dictionary reload procedure.

Upgrade qualifies an old unqualified name only when exactly one dictionary has that name across all schemas. Missing or ambiguous names stop the upgrade with a hint to qualify the configuration first; the original creator's search_path cannot be reconstructed. A custom dictionary sharing a builtin name is also ambiguous, so explicitly choose either the custom schema or `pg_catalog` before upgrading. The same rule applies to legacy definitions loaded before the SQL upgrade. Changing dictionary behavior does not rewrite stored vectors or indexes.

Catalog-writing functions are `PARALLEL UNSAFE`; read-only analysis remains `PARALLEL SAFE`. Runtime vocabulary tables and sequences live in `tokenizer_data`; database owners receive CREATE there and only USAGE on `tokenizer_catalog`. Schema owners retain PostgreSQL's inherent owner powers. Upgrade preserves vocabulary OIDs, ownership, ACLs and dependencies; direct SQL using old qualified table names must change. It never adopts an unrelated schema or other pre-existing upgrade object. Conflicts fail transactionally and must be resolved by an administrator. Statements crossing the SQL upgrade with an older snapshot may receive SQLSTATE `40001` and need a transaction retry. The version table grants PUBLIC only SELECT; its trigger runs with a fixed `pg_catalog` search_path and has no public EXECUTE grant.

## Logical backups

The five definition tables (`text_analyzer`, `tokenizer`, `model`, `stopwords`, `synonym`) are registered with `pg_extension_config_dump`, so an ordinary `pg_dump` includes their rows together with custom vocabulary tables, their identity sequences, and source-table triggers. `CREATE EXTENSION` recreates the shipped `lucene_english`, `nltk_english`, and `iso_english` stopword lists, so those three names are not dumped. They are reserved and cannot be dropped or replaced, so a restore recreates the same lists.

External Lindera dictionaries and the cluster's `pg_tokenizer/preload_models` directory are outside the database and need their own backup. A physical PostgreSQL backup includes the catalog rows and the preload directory, but not external dictionary paths.
