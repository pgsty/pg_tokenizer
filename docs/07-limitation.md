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

## Logical backups

The five definition tables (`text_analyzer`, `tokenizer`, `model`, `stopwords`, `synonym`) are registered with `pg_extension_config_dump`, so an ordinary `pg_dump` includes their rows together with custom vocabulary tables, their identity sequences, and source-table triggers. `CREATE EXTENSION` recreates the shipped `lucene_english`, `nltk_english`, and `iso_english` stopword lists, so those three names are not dumped. They are reserved and cannot be dropped or replaced, so a restore recreates the same lists.

External Lindera dictionaries and the cluster's `pg_tokenizer/preload_models` directory are outside the database and need their own backup. A physical PostgreSQL backup includes the catalog rows and the preload directory, but not external dictionary paths.
