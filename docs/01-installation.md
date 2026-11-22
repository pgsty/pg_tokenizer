# Installation

`pg_tokenizer` 1.2.0 supports **PostgreSQL 15–19** on **`x86_64` and `aarch64`**.

`pg_tokenizer` must be loaded via `shared_preload_libraries` before PostgreSQL starts. If `shared_preload_libraries` already lists other extensions (such as `vchord` or `vchord_bm25`), keep them when adding `pg_tokenizer`.

## Linux Packages via Pigsty (Recommended)

On EL 8/9/10, Debian 12/13, and Ubuntu 22.04/24.04, install native packages from the [Pigsty repository](https://pigsty.io/ext/e/pg_tokenizer) (full package matrix on [pgext.cloud](https://pgext.cloud/ext/pg_tokenizer)):

```bash
curl -fsSL https://repo.pigsty.io/pig | bash
pig repo add pgsql -u
pig ext add pg_tokenizer
```

Pigsty RPM and DEB packages omit embedded Lindera dictionaries to keep package sizes small; provide external dictionaries compiled with Lindera 6 when using Lindera models. GitHub Release `.deb` and `.zip` packages enable `lindera-ipadic` with embedded IPADIC.

## GitHub Release Packages (`.deb` and `.zip`)

The 1.2.0 release workflow targets PostgreSQL 15–19 on `x86_64` (`amd64`) and `aarch64` (`arm64`). The examples use its package names. Check [GitHub Releases](https://github.com/pgsty/pg_tokenizer/releases) for published downloads.

Prebuilt GitHub binaries are compiled on Ubuntu 24.04 and require **`glibc >= 2.39`** (Ubuntu 24.04+ or Debian 13+). On older distributions such as EL 8/9, Debian 12, or Ubuntu 22.04, install from the Pigsty repository or build from source.

### From `.deb` Package

1. Download the matching `.deb` for your PostgreSQL major version and CPU architecture, then install it:

```sh
sudo apt install ./postgresql-18-pg-tokenizer_1.2.0-1_amd64.deb
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
wget https://github.com/pgsty/pg_tokenizer/releases/download/v1.2.0/postgresql-18-pg-tokenizer_1.2.0_x86_64-linux-gnu.zip -O pg_tokenizer.zip
unzip pg_tokenizer.zip -d pg_tokenizer
```

2. Install the shared library and extension SQL/control files using `install` (which replaces files atomically without truncating a library mapped by a running server):

```sh
sudo install -m 755 pg_tokenizer/pg_tokenizer.so "$(pg_config --pkglibdir)/"
sudo install -m 644 pg_tokenizer/pg_tokenizer--*.sql pg_tokenizer/pg_tokenizer.control "$(pg_config --sharedir)/extension/"
```

3. Configure `shared_preload_libraries`, restart PostgreSQL, and run `CREATE EXTENSION IF NOT EXISTS pg_tokenizer;`.

## From Source

Building from source requires Rust 1.99.1+, Python 3.11+, PostgreSQL 15–19 development headers, and matching Clang, libclang, and LLVM tools (see [development](02-development.md)).

```sh
cargo install cargo-pgrx --version 0.20.0 --locked
git clone https://github.com/pgsty/pg_tokenizer.git
cd pg_tokenizer
PG_CONFIG=/path/to/pg_config make build
sudo make PG_CONFIG=/path/to/pg_config install
```

`FEATURES` defaults to `lindera-ipadic`; pass `FEATURES="..."` to `make build` to embed other Lindera dictionaries (`lindera-ipadic-neologd`, `lindera-unidic`, `lindera-ko-dic`, `lindera-cc-cedict`). After installing, add `pg_tokenizer` to `shared_preload_libraries`, restart PostgreSQL, and create the extension.

## Upgrading

### Upgrading from 1.1.0 to 1.2.0

1. Ensure PostgreSQL is on version 15–19 (`pgrx 0.20.0` drops PostgreSQL 14 support; upgrade PostgreSQL 14 clusters first).
2. Install the `1.2.0` shared library and SQL files on the primary and all replicas, then restart PostgreSQL on each node.
3. Update the extension in each database:

```sql
ALTER EXTENSION pg_tokenizer UPDATE;
```

The `1.1.0 → 1.2.0` update preserves all SQL signatures, catalog definitions, `tokenizer_data` custom vocabularies, and token IDs. No re-tokenization or reindexing is required.

Valid preload files from earlier versions work in place. Each node reads `$PGDATA/pg_tokenizer/preload_models` at startup. If either directory component or a model entry is a symlink, move its intended contents into real directories and regular files before restart.

To recreate a damaged file, call `tokenizer_catalog.add_preload_model(name)` in the database containing the intended definition and restart that node. This also restores sharing when a hand-edited catalog row has different JSON whitespace from its older preload file. Version 1.2.0 copies the exact stored configuration. See [preload models](06-model.md#preload-model).

### Upgrading from 1.0.0 or 0.1.x

`pg_tokenizer` ships the full migration chain `0.1.0 → 0.1.1 → 1.0.0 → 1.1.0 → 1.2.0`.

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
- **Lindera 6.2 (`1.0.0`)**: Embedded IPADIC updated from `2007-08-01` to `2025-09-20`, external dictionaries must be compiled with Lindera 6, and user-dictionary token IDs start at `1073741824`. Recompute stored Lindera token arrays and rebuild indexes when upgrading from `0.1.x`.
- **Reserved stopword lists (`1.0.0`)**: Built-in names (`lucene_english`, `nltk_english`, `iso_english`) cannot be dropped or overwritten.

## Database-Owner Permissions

Installation and upgrade create two schemas and grant database-local management rights to `pg_database_owner`:
- `tokenizer_catalog`: `USAGE` on the schema, `EXECUTE` on database-local functions, `SELECT, INSERT, UPDATE, DELETE` on catalog tables, and `USAGE, SELECT` on sequences (`PUBLIC` has no catalog write access).
- `tokenizer_data`: `USAGE, CREATE` on the schema, plus `SELECT, INSERT, UPDATE, DELETE` on custom vocabulary tables and `USAGE, SELECT` on their identity sequences.

A non-superuser database owner can create and drop its own analyzers, tokenizers, stopwords, synonyms, and custom models after an administrator installs the extension. Dropping a custom model created by another role still requires that table's owner or a superuser. Other application roles require explicit `GRANT` statements.

- **Lindera server files**: Loading a Lindera `dictionary` or `user_dictionary` from a server filesystem path or `file://` URI requires superuser or `pg_read_server_files` privileges on every backend load. Unprivileged roles can use `embedded://` dictionaries or reuse a model preloaded by the postmaster whose catalog configuration matches.
- The **cluster preload list** lives in `$PGDATA/pg_tokenizer/preload_models`. The function owner and superusers can call `add_preload_model` and `remove_preload_model` or grant `EXECUTE` explicitly. A database owner receives `EXECUTE` on `list_preload_models()` by default. PostgreSQL reads the files on restart and logs a warning for each entry it skips.
