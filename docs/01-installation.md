# Installation

Version 1.1.0 release acceptance covers fresh installations on PostgreSQL 14–18. Upgrade instructions are retained for reference; upgrade compatibility, default-privilege migration and snapshots spanning an upgrade are outside this release's acceptance scope.

## Linux Packages via `pig`

On EL 8/9/10, Debian 12/13, and Ubuntu 22.04/24.04/26.04 (PostgreSQL 14–18), you can install the native package built for your distribution and architecture from the [Pigsty repository](https://pigsty.io/ext/e/pg_tokenizer) (see the package matrix on [pgext.cloud](https://pgext.cloud/ext/pg_tokenizer)):

```bash
curl -fsSL https://repo.pigsty.io/pig | bash
pig repo add pgsql -u
pig ext add pg_tokenizer
```

Native RPM/DEB packages do not embed Lindera dictionaries. Provide external dictionaries compiled with Lindera 6. GitHub `.deb`/ZIP packages enable `lindera-ipadic`; other dictionaries are optional build features.

## Docker

The upstream images below do not establish availability of PGSTY's 1.1.0 release. Use the matching release package or source tag for a 1.1.0 installation.

The [`tensorchord/vchord-suite`](https://vchord.pgsty.com/docs/vchord/getting-started/vectorchord-suite/) Docker image comes pre-configured with the Vchord extension suite:
- [`pg_tokenizer`](https://github.com/pgsty/pg_tokenizer) - This extension (`pgsty/pg_tokenizer`)
- [`vchord_bm25`](https://github.com/pgsty/vchord_bm25) - Native BM25 Ranking Index (`pgsty/vchord_bm25`)
- [`vchord`](https://github.com/pgsty/vchord) - Scalable, high-performance, and disk-efficient vector similarity search (`pgsty/vchord`)
- [`pgvector`](https://github.com/pgvector/pgvector) - Popular vector similarity search

Simply run the Docker container as shown below:

```bash
docker run \
  --name vchord-suite \
  -e POSTGRES_PASSWORD=postgres \
  -p 5432:5432 \
  -d tensorchord/vchord-suite:pg18-latest
  # If you want to use ghcr image, you can change the image to `ghcr.io/tensorchord/vchord-suite:pg18-latest`.
  # if you want to use the specific version, you can use the tag `pg17-20250414`, supported version can be found in the support matrix.
```

Once everything’s set up, you can connect to the database using the `psql` command line tool. The default username is `postgres`, and the default password is `postgres`. Here’s how to connect:

```sh
psql -h localhost -p 5432 -U postgres
```

After connecting, run the following SQL to make sure the extension is enabled:

```sql
CREATE EXTENSION pg_tokenizer;
```

## From Debian package

Use the `.deb` built for the exact distribution, PostgreSQL major version, and CPU architecture of the target host. GitHub 1.1.0 release packages are built on Ubuntu 24.04; use the Pigsty repository for native packages targeting other distributions. Dependency metadata and the target distribution determine compatibility. Do not infer compatibility from a generic GLIBC version floor.

Debian packages (`.deb`) are used in distributions based on Debian, such as Ubuntu and many others. They can be installed by `dpkg` or `apt-get`.

1. Download the matching deb package from the [1.1.0 release page](https://github.com/pgsty/pg_tokenizer/releases/tag/v1.1.0). For PostgreSQL 17 on Ubuntu 24.04 amd64, run `sudo apt install ./postgresql-17-pg-tokenizer_1.1.0-1_amd64.deb`.

2. Configure your PostgreSQL by modifying the `shared_preload_libraries` and `search_path` to include the extension.

```sh
psql -U postgres -c "ALTER SYSTEM SET shared_preload_libraries = 'pg_tokenizer.so';"
psql -U postgres -c "ALTER SYSTEM SET search_path TO \"\$user\", public, tokenizer_catalog;"
# You need restart the PostgreSQL cluster to take effects.
sudo systemctl restart postgresql.service   # for pg_tokenizer running with systemd
```

3. Connect to the database and enable the extension.

```sql
CREATE EXTENSION IF NOT EXISTS pg_tokenizer;
```

## From ZIP package

ZIP archives contain the extension library and SQL/control files without package-manager dependency metadata. Use an archive with the matching PostgreSQL major version and CPU architecture, and verify that the shared library is compatible with the target system's libc before installing. For supported EL, Debian, or Ubuntu systems, prefer the matching native RPM or `.deb` package.

For systems without a matching native package that cannot run a Docker container, follow these steps to install:

1. Before install, make sure that you have the necessary packages installed, including `PostgreSQL`, `pg_config`, `unzip`, `wget`.

```sh
# Example for RHEL 9 dnf
# Please check your package manager
sudo dnf install -y unzip wget libpq-devel
sudo dnf module install -y postgresql:15/server
sudo postgresql-setup --initdb
sudo systemctl start postgresql.service
sudo systemctl enable postgresql.service
```

2. Locate PostgreSQL's library and shared-data directories.

```sh
pg_config --pkglibdir
# Print something similar to:
# /usr/lib/postgresql/15/lib or
# /usr/lib64/pgsql

pg_config --sharedir
# Print something similar to:
# /usr/share/postgresql/15 or
# /usr/share/pgsql
```

3. Download the matching ZIP package from the [1.1.0 release page](https://github.com/pgsty/pg_tokenizer/releases/tag/v1.1.0) and extract it to a temporary directory. This example targets PostgreSQL 17 on x86_64:

```sh
wget https://github.com/pgsty/pg_tokenizer/releases/download/v1.1.0/postgresql-17-pg-tokenizer_1.1.0_x86_64-linux-gnu.zip -O pg_tokenizer.zip
unzip pg_tokenizer.zip -d pg_tokenizer
```

4. Install the extension files into the PostgreSQL directories.

```sh
# Install library into `$pkglibdir`
sudo install -m 755 pg_tokenizer/pg_tokenizer.so "$(pg_config --pkglibdir)/"
# Install schema and control files into `$sharedir`
sudo install -m 644 pg_tokenizer/pg_tokenizer--*.sql pg_tokenizer/pg_tokenizer.control "$(pg_config --sharedir)/extension/"
```

Use `install` to replace the library without truncating a file that PostgreSQL may already have loaded. Restart PostgreSQL afterward to load the installed library.

5. Configure your PostgreSQL by modifying the `shared_preload_libraries` and `search_path` to include the extension.

```sh
psql -U postgres -c "ALTER SYSTEM SET shared_preload_libraries = 'pg_tokenizer.so';"
psql -U postgres -c "ALTER SYSTEM SET search_path TO \"\$user\", public, tokenizer_catalog;"
# You need restart the PostgreSQL cluster to take effects.
sudo systemctl restart postgresql.service   # for pg_tokenizer running with systemd
```

6. Connect to the database and enable the extension.

```sql
CREATE EXTENSION IF NOT EXISTS pg_tokenizer;
```


## From Source

Before building from source, you could refer to the [development](02-development.md) guide to set up the development environment.

1. Build and install the extension.

```sh
cargo pgrx install --sudo --release --pg-config "$(command -v pg_config)" --features lindera-ipadic
```

2. Configure your PostgreSQL by modifying the `shared_preload_libraries` and `search_path` to include the extension.

```sh
psql -U postgres -c "ALTER SYSTEM SET shared_preload_libraries = 'pg_tokenizer.so';"
psql -U postgres -c "ALTER SYSTEM SET search_path TO \"\$user\", public, tokenizer_catalog;"
# You need restart the PostgreSQL cluster to take effects.
sudo systemctl restart postgresql.service   # for pg_tokenizer running with systemd
```

3. Connect to the database and enable the extension.

```sql
CREATE EXTENSION pg_tokenizer;
```

## Upgrading to 1.1.0

Upgrade acceptance is outside this release's scope. In particular, existing per-schema default privileges and transactions retaining a snapshot across an upgrade need separate validation before an upgrade is used in production.

After installing the 1.1.0 library and SQL files, restart PostgreSQL to load the new preloaded library, then update each database as the extension owner or an administrator. Run the update at `READ COMMITTED`:

```sql
BEGIN ISOLATION LEVEL READ COMMITTED;
ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0';
COMMIT;
```

The `1.0.0 → 1.1.0` script refuses `REPEATABLE READ` and `SERIALIZABLE` with SQLSTATE `25000`, including when they come from `default_transaction_isolation`. At those levels every statement of the script reuses the snapshot taken before it waits for the model catalog lock, so a custom model committed during that wait would be left behind in the old schema. An explicit `READ COMMITTED` transaction, as above, is enough. Replace the installed library without truncating the existing file, for example with the `install` commands in the ZIP instructions; never overwrite a library that the running server has preloaded in place.

The direct upgrade is `1.0.0 → 1.1.0`; older 0.1.0 and 0.1.1 installations use the existing chain through 1.0.0. There is no published 1.0.1 release or migration step. It preserves catalog data, existing custom-model token IDs, and dependencies on the six-argument helper (including its function OID). Standard `cargo pgrx install/package` and `tools/package.sh` include the migration scripts. After the new library is loaded, committed definition changes invalidate backend-local caches; each lookup respects its SQL snapshot, and objects loaded from an older snapshot are discarded before a later statement or transaction reuses them.

1.1.0 adds a transactional version table and definition triggers, corrects all catalog writers to `PARALLEL UNSAFE`, and pins legacy `pg_dict` names. Resolve missing or ambiguous legacy names before upgrading. A custom dictionary sharing a builtin name is ambiguous too; explicitly choose its schema or `pg_catalog`. SQL signatures and custom vocabulary IDs are preserved. New input/output budgets can reject previously accepted large documents with SQLSTATE `54000`; review [the defaults and remaining library limits](07-limitation.md#cancellation-and-resource-use) before upgrading a bulk ingestion workload. Deploy the matching library to every replica and restart it as well. The old primary-only cache path remains active until the SQL upgrade has committed and replayed.

The `full` and `precise` Jieba modes were swapped in 1.0.0 and earlier. Regenerate persisted output produced with either mode and rebuild dependent indexes after upgrading; the default `search` mode is unchanged.

### Runtime vocabulary migration

The upgrade moves registered `tokenizer_catalog.model_<name>` tables, their indexes and owned identity sequences into the extension-owned `tokenizer_data` schema. It uses `ALTER TABLE ... SET SCHEMA`, preserving OIDs, owners, table/sequence ACLs, token IDs, views and trigger dependencies. Existing nonowner schema USAGE/CREATE grants and grant options are copied to the data schema; CREATE is revoked from the catalog schema. Direct SQL that names an old vocabulary table must use `tokenizer_data.model_<name>` afterward. Function calls still use `tokenizer_catalog`; no data-schema search_path entry is required.

An unrelated existing `tokenizer_data` schema, a missing registered vocabulary table or an object-name collision aborts the entire upgrade. Resolve the conflicting object explicitly and retry. Do not drop or rename application objects blindly. The upgrader must be able to alter every registered vocabulary table; if models have different owners, use a superuser. The upgrade locks the model catalog and vocabulary tables; finish long-running model-management transactions first. A statement whose snapshot predates the upgrade may receive SQLSTATE `40001`; retry its transaction with a new snapshot. The new library supports 1.0.0 SQL until the migration commits.

### Database-owner permissions

Installation and upgrade grant `pg_database_owner` USAGE on `tokenizer_catalog`, USAGE/CREATE on `tokenizer_data`, EXECUTE on database-local management functions, SELECT/INSERT/UPDATE/DELETE on catalog and vocabulary tables, and USAGE/SELECT on their sequences. Newly created custom models explicitly grant table/sequence access. The grants follow a database ownership change. Nonowner CREATE grants are removed from `tokenizer_catalog`; PostgreSQL schema owners and roles with their privileges retain inherent owner powers.

A non-superuser database owner can create/drop its own custom models, analyzers, tokenizers, stopwords, and synonyms after an administrator installs the extension. Grants do not transfer table ownership: dropping a model created by a different role still needs that object's owner or an administrator. Other application roles require explicit grants; the extension does not give PUBLIC schema access or catalog write privileges. Public API functions continue to execute with the caller's privileges. The internal version trigger is SECURITY DEFINER, uses a fixed search_path, and only increments its protected counter; PUBLIC receives SELECT on that counter.

Lindera dictionaries can be files on the database server. Only superusers and roles with the privileges of `pg_read_server_files` (directly or through inherited membership) may load a Lindera model whose `segmenter.dictionary` or `segmenter.user_dictionary` is a filesystem path or `file://` URI; other roles, including non-superuser database owners, can use `embedded://` dictionaries. The check runs on every load: at `create_lindera_model` and whenever a backend first loads the model from the catalog, including rows written directly to `tokenizer_catalog.model`. An object already loaded in a backend, and a model preloaded by the postmaster whose catalog definition matches, remain usable by every role. Therefore an existing model with an external dictionary now fails with SQLSTATE `42501` for an unprivileged role whose session would load it; grant `pg_read_server_files` to that role or preload the model with `add_preload_model`. Lindera's own error details (which can quote file names and contents) are reported only to roles allowed to read server files.

The cluster-wide `add_preload_model` and `remove_preload_model` functions are restricted to the extension owner and explicitly authorized administrators. `PUBLIC` and `pg_database_owner` have no EXECUTE grant on these two functions. Changes to preload files are not transactional and take effect on restart. Broken preload entries now generate WARNING messages and are skipped; repair or remove the entry before expecting that model to be preloaded.

### Compatibility changes in 1.0.0

- `apply_text_analyzer` is now `STABLE`, like `tokenize`: analyzer definitions are catalog data, so `IMMUTABLE` was incorrect and allowed cached plans to bypass invalidation. The upgrade refuses to run while an index, stored generated column, partition key, or extended statistics object depends on it. Replace such expressions with ordinary columns maintained by the application or a trigger, index those columns, then upgrade.
- Lindera moves from 0.42 to 6.2. The embedded IPADIC dictionary changes from the 2007-08-01 edition to the 2025-09-20 edition, so its token IDs change; external compiled dictionaries must be rebuilt with Lindera 6. User-dictionary IDs now use `1073741824 + dictionary-local ID` instead of overlapping system-dictionary IDs. Regenerate stored Lindera token arrays and rebuild their indexes; see the [model notes](06-model.md#lindera-model).
- A matching custom-model row with a NULL source now gets a NULL embedding, including a value supplied explicitly by the INSERT; 0.1.x left the target column unchanged. Nonmatching filtered rows retain their embedding.
- Vocabulary writers that add tokens to a custom model serialize on a per-model advisory lock until commit; writers whose tokens already exist do not wait. `REPEATABLE READ` and `SERIALIZABLE` writers can receive SQLSTATE `40001` and must retry.
- `PREPARE TRANSACTION` is rejected after calling a definition-changing API. Read-only use can be prepared.
- `add_preload_model` and `remove_preload_model` are restricted to administrators, and broken preload entries are skipped with a WARNING instead of preventing startup. Before upgrading, remove preload entries for Lindera models that use external dictionaries not yet rebuilt with Lindera 6.
- The built-in `lucene_english`, `nltk_english`, and `iso_english` stopword lists are reserved: `drop_stopwords` and `create_stopwords` refuse these names. If an earlier version replaced one of them, the upgrade prints a WARNING; copy the list to a new name and reference that name instead.
- `pg_dump` now includes the definition tables; see [logical backups](07-limitation.md#logical-backups).
