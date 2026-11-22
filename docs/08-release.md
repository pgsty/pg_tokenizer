# Release and PGXN Packaging

Release **`1.2.0`** targets PostgreSQL 15–19 on `x86_64` and `aarch64`. CI and packaging use Rust `1.100.0` from `rust-toolchain.toml` and `pgrx 0.20.0`.

## Maintainer Release Checklist

1. Verify `Cargo.toml`, `Cargo.lock`, `sql/install/pg_tokenizer--1.2.0.sql`, `sql/upgrade/pg_tokenizer--1.1.0--1.2.0.sql`, and `docs/release-notes/1.2.0.md` are up to date (`pg_tokenizer.control` keeps `default_version = '@CARGO_VERSION@'` in Git).
2. Push to `main` and wait for the `Check` workflow (`.github/workflows/check.yml`) to pass across all style and PostgreSQL 15–19 × `x86_64`/`aarch64` jobs.
3. Tag the green commit as `v1.2.0`, push the tag, and manually dispatch the `Release` workflow (`.github/workflows/release.yml`) with `tag = v1.2.0`.
4. `Release` verifies that `Check` passed on the tagged commit, builds and tests `.deb` and `.zip` packages for PostgreSQL 15–19 (`amd64` and `arm64`, Ubuntu 24.04) plus the PGXN source archive (`pg_tokenizer-1.2.0.zip`), and uploads them with `SHA256SUMS` to a draft GitHub Release.
5. Inspect the draft release assets, checksums, and release notes before publishing on GitHub.

## Building and Validating the PGXN Source Archive

`META.json.in` defines the PGXN distribution metadata (`apache_2_0`, PostgreSQL `>= 15.0.0, < 20.0.0`). `make dist` exports the selected Git commit into `build/dist/pg_tokenizer-<version>.zip` (resolving `sql/` symlinks into regular files, substituting `@CARGO_VERSION@` in `pg_tokenizer.control`, rendering `META.json`, and excluding `.github` and `reports` via `.gitattributes`):

```bash
make meta
make dist REF=HEAD
unzip -t build/dist/pg_tokenizer-1.2.0.zip
(cd build/dist && sha256sum --check pg_tokenizer-1.2.0.zip.sha256)
```

To validate `META.json` with the official PGXN validator (`cpanm PGXN::Meta::Validator`):

```bash
validate_pgxn_meta META.json
```

## Building and Installing from a PGXN Archive

PGXN distributes source archives; the target host must provide Python 3.11+, GNU Make, Rust 1.99.1+, `cargo-pgrx 0.20.0`, Clang/libclang, and PostgreSQL 15–19 development headers:

```bash
cargo install cargo-pgrx --version 0.20.0 --locked
make PG_CONFIG=/path/to/pg_config
sudo make install PG_CONFIG=/path/to/pg_config
```

Or with `pgxnclient`:

```bash
pgxn install --pg_config /path/to/pg_config build/dist/pg_tokenizer-1.2.0.zip
```

`FEATURES` defaults to `lindera-ipadic`. `make install` verifies the staged manifest digests and atomically renames files into place so a running PostgreSQL server keeps its mapped shared library until restart.

## Manual PGXN Upload

GitHub workflows never upload to PGXN automatically. After publishing a release, maintainers can upload `build/dist/pg_tokenizer-<version>.zip` via [PGXN Manager](https://manager.pgxn.org/howto):

```bash
curl --fail-with-body --user YOUR_PGXN_USER \
  --form dist=@build/dist/pg_tokenizer-1.2.0.zip \
  https://manager.pgxn.org/upload
```
