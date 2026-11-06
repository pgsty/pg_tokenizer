# Limitation

## Backend-local definition caches

`pg_tokenizer` caches catalog-backed `text_analyzer`, `tokenizer`, `model`, `stopwords`, and `synonym` objects in each PostgreSQL backend. The catalog remains the source of truth. A successful `create_*` or `drop_*` clears the current backend immediately; a shared invalidation epoch tells other backends to clear their caches before their next lookup. The epoch is published on commit. A running statement or REPEATABLE READ transaction can still see the older catalog through its snapshot. Objects loaded after observing a new epoch are discarded before a later statement and at commit, abort, or read-only prepare, so a newer snapshot can reload the definitions. Rollbacks clear only local caches; rolled-back subtransactions do not publish changes.

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


Definition-changing API calls cannot participate in a prepared transaction: `PREPARE TRANSACTION` raises an error. Use ordinary transactions for this DDL. Direct catalog-table edits are unsupported and do not publish cache invalidations; use the `create_*` / `drop_*` APIs.

`apply_text_analyzer`, `tokenize`, and the native `tsvector` wrappers are `STABLE`, not immutable. Keep their output in ordinary columns maintained explicitly or by triggers. Do not use immutable wrappers to bypass PostgreSQL's expression-index/generated-column checks.

## Cancellation and resource use

Statement timeouts and cancel requests are processed between analyzer stages, for each token in token filters and models, and every 1,024 n-grams. A single call into Jieba, Lindera, or a Hugging Face tokenizer for one long piece of text runs to completion before the cancel takes effect. Analyzer output is held in backend memory outside PostgreSQL's memory accounting, and there are no input, output, or expansion size limits yet: an n-gram filter with a large `max_gram` on long unsplit text produces very many grams. Bound input size in the application and use `statement_timeout`.

## Known SQL-level limitations

These need SQL object or schema changes and are left for a later SQL version:

- `create_*`/`drop_*` for tokenizers, text analyzers, Hugging Face and Lindera models, stopwords, and synonyms are labeled `PARALLEL SAFE` although they write the catalog; do not call them from queries that may run in parallel workers.
- Database owners have `CREATE` on `tokenizer_catalog` (custom-model vocabulary tables live there), so they can create objects in the extension's schema.
- A `pg_dict` token filter resolves its dictionary name with the session's `search_path` and caches the dictionary OID; use schema-qualified dictionary names, and reconnect after dictionary DDL.

## Logical backups

The five definition tables (`text_analyzer`, `tokenizer`, `model`, `stopwords`, `synonym`) are registered with `pg_extension_config_dump`, so an ordinary `pg_dump` includes their rows together with custom vocabulary tables, their identity sequences, and source-table triggers. `CREATE EXTENSION` recreates the shipped `lucene_english`, `nltk_english`, and `iso_english` stopword lists, so those three names are not dumped. They are reserved and cannot be dropped or replaced, so a restore recreates the same lists.

External Lindera dictionaries and the cluster's `pg_tokenizer/preload_models` directory are outside the database and need their own backup. A physical PostgreSQL backup includes the catalog rows and the preload directory, but not external dictionary paths.
