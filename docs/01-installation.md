# Installation

`pg_tokenizer` 1.3.0 supports **PostgreSQL 15–19** on **`x86_64` and `aarch64`**.

`pg_tokenizer` must be loaded via `shared_preload_libraries` before PostgreSQL starts. If `shared_preload_libraries` already lists other extensions (such as `vchord` or `vchord_bm25`), keep them when adding `pg_tokenizer`.

## Linux Packages via Pigsty (Recommended)

On EL 8/9/10, Debian 12/13, and Ubuntu 22.04/24.04, install native packages from the [Pigsty repository](https://pigsty.io/ext/e/pg_tokenizer) (full package matrix on [pgext.cloud](https://pgext.cloud/ext/pg_tokenizer)):

```bash
curl -fsSL https://repo.pigsty.io/pig | bash
pig repo add pgsql -u
pig ext add pg_tokenizer
```

Pigsty RPM and DEB packages omit embedded Lindera dictionaries to keep package sizes small. For `pg_tokenizer` 1.3.0, provide external system dictionaries built with Lindera 7 (format v3). The GitHub release workflow enables `lindera-ipadic` to embed IPADIC in its `.deb` and `.zip` packages.

## GitHub Release Packages (`.deb` and `.zip`)

The 1.3.0 release workflow targets PostgreSQL 15–19 on `x86_64` (`amd64`) and `aarch64` (`arm64`). The examples use its package names. Check [GitHub Releases](https://github.com/pgsty/pg_tokenizer/releases) for published downloads.

Prebuilt GitHub binaries are compiled on Ubuntu 24.04 and require **`glibc >= 2.39`** (Ubuntu 24.04+ or Debian 13+). On older distributions such as EL 8/9, Debian 12, or Ubuntu 22.04, install from the Pigsty repository or build from source.

Check the server minor version as well as the major. Binaries linked to PostgreSQL's newer tuple-header functions require at least 15.20, 16.16, 17.12 or 18.7 on those respective branches. PostgreSQL 19 provides the functions from its first release. The DEB declares its server dependency. For a ZIP, read the minimum in `unzip -p pg_tokenizer.zip pg_tokenizer.control` before replacing a preloaded library. For an older server, use a build made with that server's `pg_config`.

### From `.deb` Package

1. Download the matching `.deb` for your PostgreSQL major version and CPU architecture, then install it:

```sh
sudo apt install ./postgresql-18-pg-tokenizer_1.3.0-1_amd64.deb
```

2. Configure `shared_preload_libraries` (and optionally `search_path`), then restart PostgreSQL:

```sh
psql -U postgres -c "ALTER SYSTEM SET shared_preload_libraries = 'pg_tokenizer';"
psql -U postgres -c "ALTER SYSTEM SET search_path TO \"\$user\", public, tokenizer_catalog;"
sudo systemctl restart postgresql.service
```

3. Enable the extension in each target database:

```sql
CREATE EXTENSION IF NOT EXISTS pg_tokenizer;
```

### From `.zip` Archive

1. Download and extract the matching `.zip` archive for your PostgreSQL major version and architecture:

```sh
wget https://github.com/pgsty/pg_tokenizer/releases/download/v1.3.0/postgresql-18-pg-tokenizer_1.3.0_x86_64-linux-gnu.zip -O pg_tokenizer.zip
unzip pg_tokenizer.zip -d pg_tokenizer
```

2. Install the shared library and extension SQL/control files using `install` (which replaces files atomically without truncating a library mapped by a running server):

```sh
sudo install -m 755 pg_tokenizer/pg_tokenizer.so "$(pg_config --pkglibdir)/"
sudo install -m 644 pg_tokenizer/pg_tokenizer--*.sql pg_tokenizer/pg_tokenizer.control "$(pg_config --sharedir)/extension/"
sudo install -Dm 644 pg_tokenizer/IPADIC-NOTICE.txt "$(pg_config --docdir)/pg_tokenizer/IPADIC-NOTICE.txt"
```

3. Configure `shared_preload_libraries`, restart PostgreSQL, and run `CREATE EXTENSION IF NOT EXISTS pg_tokenizer;`.

## From Source

Building from source requires Rust 1.99.1+, Python 3.11+, PostgreSQL 15–19 development headers, and matching Clang, libclang, and LLVM tools. With rustup, the checkout selects Rust 1.100.0. See [development](02-development.md).

```sh
git clone https://github.com/pgsty/pg_tokenizer.git
cd pg_tokenizer
cargo install cargo-pgrx --version 0.20.0 --locked
PG_CONFIG=/path/to/pg_config make build
sudo make PG_CONFIG=/path/to/pg_config install
```

`FEATURES` defaults to `lindera-ipadic`; pass `FEATURES="..."` to `make build` to embed other Lindera dictionaries (`lindera-ipadic-neologd`, `lindera-unidic`, `lindera-ko-dic`, `lindera-cc-cedict`). After installing, add `pg_tokenizer` to `shared_preload_libraries`, restart PostgreSQL, and create the extension.

## Upgrading

### Upgrading from 1.2.0 to 1.3.0

Install the new library, control file and SQL scripts on the primary and all physical replicas. Restart PostgreSQL on each node to load the new shared library. In each database using the extension, run the following on the primary.

```sql
ALTER EXTENSION pg_tokenizer UPDATE TO '1.3.0';
```

The new library can serve databases with the 1.2.0 SQL definitions during this rollout. Updating the SQL enables the primary cache fast path and native `tsvector` functions. The migration preserves existing vocabulary IDs, function dependencies and table triggers.

When pairing with the upstream Linux x86_64 PostgreSQL 18 ZIP for `vchord_bm25 1.3.0`, use a compatible PostgreSQL binary. That BM25 library loaded on PGDG 18.7 and required a symbol (`heap_datum_check`) absent from the tested PGDG 18.6 server. A BM25 build targeting that server can also be used. [BM25 release assets](https://github.com/pgsty/vchord_bm25/releases/tag/1.3.0)

### Upgrading from 1.1.0 to 1.2.0

1. Ensure PostgreSQL is on version 15–19 (`pgrx 0.20.0` drops PostgreSQL 14 support; upgrade PostgreSQL 14 clusters first).
2. If any model uses an **external Lindera system dictionary**, stage a format-v3 directory built or downloaded with Lindera 7.0.0 on every node that serves the model. Keep the v2 directory for rollback. With PostgreSQL stopped, move the new directory into the configured path. Embedded dictionaries ship with the library. Existing user dictionary `.csv` and `.bin` files can be reused with a compatible system dictionary.
3. Install the `1.2.0` shared library and SQL files on the primary and all replicas, then restart PostgreSQL on each node.
4. Update the extension in each database:

```sql
ALTER EXTENSION pg_tokenizer UPDATE;
```

The `1.1.0 → 1.2.0` update preserves SQL signatures, catalog definitions, and the IDs assigned to terms in `tokenizer_data` vocabularies. If an external dictionary is still v2, startup skips its preload entry with a warning. A privileged backend load then reports SQLSTATE `22023` with the expected dictionary format. Install the v3 directory at the same path and restart to restore preloading. Existing catalog rows and preload configuration files can be reused.

Three changes can affect the tokens produced from the same text.

- Lindera 7 changes segmentation and dictionary lookup, including with embedded IPADIC. For the default IPADIC model, `Google が 新しい` changes from `{19678,236490}` to `{19679,236490}`, and `龘々` changes from `{}` to `{103}`. `関西国際空港限定` still returns `{372982,374178}`.
- `regex` 1.13.2 corrects match boundaries when alternatives start with the same variable-length expression. For example, the pre-tokenizer pattern `id[0-9]*[0-9]|id[0-9]*-` applied to `id42-` now returns `id42`. Version 1.1.0 returned `id42-`.
- Rust 1.100.0 supplies Unicode 18.0 tables for `to_lowercase` and `skip_non_alphanumeric`. Rust 1.99.1 uses Unicode 17.0. For example, a build with Rust 1.100.0 lowercases U+A7DD to U+0277 and retains U+A7DD as a single alphanumeric token. A build with Rust 1.99.1 leaves its case unchanged and drops that single-character token in the filter.

Use the same compiler and dictionary build on nodes that share data. Compare document and query tokens across the upgrade for pipelines that use Lindera, these regex patterns, or affected Unicode characters. Where tokens differ, regenerate stored token arrays and `tsvector` values from the source text before serving queries with the new tokenizer. With `create_custom_model_tokenizer_and_trigger`, updating the source column on an affected row reruns both triggers, adding new terms to the vocabulary before recomputing the embedding. PostgreSQL updates indexes on the stored arrays or vectors as those rows are rewritten.

Valid preload files from earlier versions work in place. Each node reads `$PGDATA/pg_tokenizer/preload_models` at startup. If either directory component or a model entry is a symlink, move its intended contents into real directories and regular files before restart.

To recreate a damaged file, call `tokenizer_catalog.add_preload_model(name)` in the database containing the intended definition and restart that node. This also restores sharing when a hand-edited catalog row has different JSON whitespace from its older preload file. Version 1.2.0 copies the exact stored configuration. See [preload models](06-model.md#preload-model).

### Upgrading from 1.0.0 or 0.1.x

`pg_tokenizer` ships the full migration chain `0.1.0 → 0.1.1 → 1.0.0 → 1.1.0 → 1.2.0 → 1.3.0`.

When upgrading across `1.0.0 → 1.1.0`:

1. **Pre-upgrade checks**:
   - Schema-qualify any unqualified `pg_dict` dictionary names that exist in multiple schemas (including custom dictionaries that share a `pg_catalog` built-in name).
   - Ensure no unrelated `tokenizer_data` schema exists in the database, and run as a superuser if custom models are owned by multiple roles.
   - Review the [`pg_tokenizer.*` analysis budgets](07-limitation.md#cancellation-and-resource-use) if your workload processes documents larger than 16 MiB.
2. **Run under `READ COMMITTED`**:
   The `1.0.0 → 1.1.0` migration moves `tokenizer_catalog.model_<name>` tables into `tokenizer_data` and rejects `REPEATABLE READ` and `SERIALIZABLE` (SQLSTATE `25000`) so concurrent model creation cannot be missed:

```sql
BEGIN ISOLATION LEVEL READ COMMITTED;
ALTER EXTENSION pg_tokenizer UPDATE;
COMMIT;
```

Key behavior changes across earlier major steps:
- **Vocabulary schema (`1.1.0`)**: Registered `model_<name>` tables, indexes, and identity sequences move from `tokenizer_catalog` to `tokenizer_data` via `ALTER TABLE ... SET SCHEMA`, preserving OIDs, owners, ACLs, token IDs, and views. Update any direct SQL queries that referenced `tokenizer_catalog.model_<name>`.
- **Jieba modes & long custom tokens (`1.1.0`)**: Versions `1.0.0` and earlier swapped Jieba's `full` and `precise` modes (`search` was unaffected) and could drop custom-model tokens longer than 2600 bytes. Recompute stored outputs and rebuild dependent indexes if affected.
- **`apply_text_analyzer` volatility (`1.0.0`)**: `apply_text_analyzer` changed from `IMMUTABLE` to `STABLE` in `1.0.0`. Drop any expression indexes, stored generated columns, partition keys, or extended statistics referencing `apply_text_analyzer` before upgrading from `0.1.x`.
- **Lindera 6.2 (`1.0.0`)** switched embedded IPADIC from the `2007-08-01` to the `2025-09-20` edition and moved user dictionary IDs to a separate range starting at `1073741824`. When upgrading from `0.1.x` through to `1.2.0`, install format-v3 external system dictionaries for the target version and regenerate stored Lindera tokens from the source text.
- **Reserved stopword lists (`1.0.0`)**: Built-in names (`lucene_english`, `nltk_english`, `iso_english`) cannot be dropped or overwritten.

## Database-Owner Permissions

Installation and upgrade create two schemas and grant database-local management rights to `pg_database_owner`:
- `tokenizer_catalog`: `USAGE` on the schema, `EXECUTE` on database-local functions, `SELECT, INSERT, UPDATE, DELETE` on catalog tables, and `USAGE, SELECT` on sequences (`PUBLIC` has no catalog write access).
- `tokenizer_data`: `USAGE, CREATE` on the schema, plus `SELECT, INSERT, UPDATE, DELETE` on custom vocabulary tables and `USAGE, SELECT` on their identity sequences.

A non-superuser database owner can create and drop its own analyzers, tokenizers, stopwords, synonyms, and custom models after an administrator installs the extension. Dropping a custom model created by another role still requires that table's owner or a superuser. Other application roles require explicit `GRANT` statements.

- **Lindera server files**: Loading a Lindera `dictionary` or `user_dictionary` from a server filesystem path or `file://` URI requires superuser or `pg_read_server_files` privileges on every backend load. Unprivileged roles can use `embedded://` dictionaries or reuse a model preloaded by the postmaster whose catalog configuration matches.
- The **cluster preload list** lives in `$PGDATA/pg_tokenizer/preload_models`. The function owner and superusers can call `add_preload_model` and `remove_preload_model` or grant `EXECUTE` explicitly. A database owner receives `EXECUTE` on `list_preload_models()` by default. PostgreSQL reads the files on restart and logs a warning for each entry it skips.
