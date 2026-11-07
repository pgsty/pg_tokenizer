# Release and PGXN publication

The next release is **1.1.0**, upgrading directly from **1.0.0**. The unpublished
1.0.1 candidate is not a supported installed version. Historical review reports
retain their original version names and measurements; they are excluded from
the PGXN source ZIP.

## Release gates

1. Review the final commit and run the Check workflow on PostgreSQL 14–18,
   Linux x86_64 and aarch64. Local macOS/PG18 results do not replace that matrix.
2. Verify fresh installation, the `1.0.0 → 1.1.0` upgrade, older upgrade chains,
   custom vocabulary IDs and ACLs, logical backups, standby replay, promotion,
   and coexistence with `vchord` and `vchord_bm25`.
3. Check Cargo.toml, Cargo.lock, generated install SQL and rendered control/META
   versions. Do not ship a 1.0.1 installation or migration script. The source
   control file keeps `@CARGO_VERSION@`; packaging substitutes it.
4. Tag the accepted commit as `v1.1.0` when publishing. Dispatch Release with
   that tag. It requires a successful Check run on the same commit, builds the
   ten Linux packages plus the PGXN source ZIP/checksum, and uploads to a
   **draft** GitHub release. Review its assets and notes before publication.
5. Publish the GitHub release and upload the identical source ZIP to PGXN.
   Verify the indexed metadata, download checksum and installation afterward.

Keep the [upgrade instructions](01-installation.md#upgrading-to-110) in the
release notes. They cover the runtime schema move, dictionary ambiguity,
Jieba output changes, replica restarts and resource budgets.
[Issue #13](https://github.com/pgsty/pg_tokenizer/issues/13) remains open:
third-party library allocations and cancellation latency are not hard-bounded.
The USERSET budgets are not a resource isolation boundary.

## PGXN account and distribution

[PGXN Manager](https://manager.pgxn.org/about) requires an approved account.
Log in and verify permission to publish the `pg_tokenizer` distribution and
provided extension. On 2026-11-08 the public API did not list `pg_tokenizer` or
`pg_tokenizer.rs`; an absent public entry does not establish account status or
guarantee that a name is available. Resolve any ownership conflict with PGXN
administrators before uploading.

The [PGXN upload guide](https://manager.pgxn.org/howto) describes source archives
and META.json. This repository provides `META.json.in`, with Apache-2.0 licensing,
PGSTY maintainer details, PostgreSQL 14–18 requirements, repository links and the
installed extension's version. `make dist` renders META.json and the control
file into a ZIP rooted at `pg_tokenizer-<version>/`, using a committed Git ref.
It includes source, embedded assets, Cargo.lock, historical SQL and migrations;
it excludes `.github` and review reports. SQL symlinks are stored as regular
files for portable ZIP extraction. Repeated builds of the same ref produce
the same archive in the same Python/zlib environment.

```bash
make dist REF=v1.1.0
unzip build/dist/pg_tokenizer-1.1.0.zip -d /tmp/tokenizer-source
cd /tmp/tokenizer-source/pg_tokenizer-1.1.0
validate_pgxn_meta META.json
```

Install the official validator with
`cpanm PGXN::Meta::Validator` (a private Perl library is also supported).
See the [metadata specification](https://pgxn.org/spec/) and
[validator documentation](https://metacpan.org/pod/validate_pgxn_meta).
`make meta` renders metadata from the working tree for inspection; `make dist`
always reads the selected commit and refuses to overwrite an existing ZIP.

## Building through PGXN

PGXN distributes source; it does not supply the Rust toolchain or binary
packages. The [PGXN client](https://pgxn.github.io/pgxnclient/usage.html) runs
`make all` and `make install`. The repository's Makefile supplies those targets
by calling `cargo pgrx package` and installing its staged output, including the
upgrade scripts.

Before building, install Python 3.11+, GNU Make, PostgreSQL server/development
files and `pg_config`, a Rust toolchain supported by the locked dependencies,
Clang/libclang and the platform C/C++ build tools. The local acceptance uses
Rust 1.99.1 and cargo-pgrx 0.19.3. Cargo needs network access or a populated
cache for crates, the locked Snowball Git dependency and dictionary assets.

```bash
cargo install cargo-pgrx --version 0.19.3 --locked
cargo pgrx init --pg18=/path/to/pg_config
make PG_CONFIG=/path/to/pg_config
# Inspect a private installation before installing into PostgreSQL:
make install PG_CONFIG=/path/to/pg_config DESTDIR=/tmp/tokenizer-stage
# For a real installation, omit DESTDIR and use the required filesystem rights.
```

`FEATURES` defaults to `lindera-ipadic`; select other embedded dictionaries
explicitly when needed. The PostgreSQL feature comes from `PG_CONFIG`.
`make install` verifies the staged version, PostgreSQL paths and file digests
before copying files. `PGXN_BUILD_DIR` selects the staging directory.

After publication, an appropriately provisioned machine can run:

```bash
pgxn install --pg_config /path/to/pg_config pg_tokenizer=1.1.0
```

The client also accepts a local source ZIP, so the same build/install path can
be checked before publication. Preload the matching library and restart
PostgreSQL before `CREATE EXTENSION` or `ALTER EXTENSION ... UPDATE`; installing
files alone does not reload a preloaded library.

## Upload and verification

Upload the validated ZIP through PGXN Manager's web form or its documented
upload API. For a manual API upload, curl prompts for the password:

```bash
curl --fail-with-body --user YOUR_PGXN_USER \
  --form dist=@build/dist/pg_tokenizer-1.1.0.zip \
  https://manager.pgxn.org/upload
```

For future unattended uploads, provision a PGXN username and repository secret
such as `PGXN_PASSWORD` through secure settings, then add a separate manually
dispatched publication job. The current workflow creates reviewable assets;
it does not upload to PGXN or require that secret. Do not put credentials in
Git, release notes or command-line password arguments.

After uploading, inspect the distribution and extension records in the public
PGXN API, compare the downloaded ZIP against its SHA-256 file, and install it
on a clean supported PostgreSQL host. Test both fresh creation and upgrade
from an existing 1.0.0 database, including a custom vocabulary. Record the
release commit, artifact hashes and results in the release tracker.
