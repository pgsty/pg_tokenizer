# Development

Building `pg_tokenizer` requires:
- Rust `1.100.0` for development, CI, and release builds, selected by `rust-toolchain.toml`. The minimum supported compiler is `1.99.1`, as required by `pgrx 0.20.0`.
- Python `3.11` or later
- PostgreSQL `15–19` server and development headers (`pg_config`)
- Matching Clang, libclang, and LLVM tools (set `CC`, `CLANG_PATH`, and `LIBCLANG_PATH` when multiple LLVM versions are installed; set `CLANG_NO_DEFAULT_CONFIG=1` if Clang loads a default configuration file)

## Set up Development Environment

1. Install Rust via [rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. Clone the repository and install the matching `cargo-pgrx`:

```sh
git clone https://github.com/pgsty/pg_tokenizer.git
cd pg_tokenizer
cargo install cargo-pgrx --version 0.20.0 --locked
cargo pgrx init
```

3. Build and stage the extension (defaults to `FEATURES=lindera-ipadic`):

```sh
PG_CONFIG=/path/to/pg_config make build
sudo make PG_CONFIG=/path/to/pg_config install
```

To build with the minimum supported Rust version under rustup, set `RUSTUP_TOOLCHAIN=1.99.1` for the build command.

> Do not set `--no-gc-sections` in global `RUSTFLAGS` or Cargo config: `cargo-pgrx 0.20.0` handles schema-section retention for the `cdylib` target directly, while global `--no-gc-sections` breaks standalone Rust unit-test linking.

## Debugging

`debug` and `dev-opt` profiles retain debug symbols:

```sh
cargo pgrx package --debug --features pg19
cargo pgrx package --profile dev-opt --features pg19
RUST_BACKTRACE=1 cargo pgrx run --profile dev-opt
```

## Running Tests

1. **Format, static checks, and unit tests** (no running PostgreSQL server required):

```sh
cargo fmt --check
cargo clippy --locked --features 'pg19 lindera-ipadic'
cargo test --locked --no-fail-fast --features 'pg19 lindera-ipadic'
python3 tests/test_pgxn.py
```

2. **Installed binary & SQL verification**:

```sh
python3 tools/check_install.py --pg-config /path/to/pg_config
```

3. **SQL and integration suites** (against a disposable PostgreSQL cluster with `shared_preload_libraries = 'pg_tokenizer'` and `max_prepared_transactions = 10`):

```sh
sqllogictest --db "$USER" --user "$USER" './tests/**/*.slt'
PG_TOKENIZER_TEST_DSN="host=localhost user=postgres dbname=postgres" python3 tests/test_integration.py
python3 tests/test_preload.py
python3 tests/test_standby.py
```

## CI and Packaging

- **Check (`.github/workflows/check.yml`)**: Runs style/PGXN archive checks plus the `x86_64` and `aarch64` matrix across PostgreSQL 15–19 on Ubuntu 24.04 (Clippy, Rust unit tests, symbol/schema parity check, SQLLogicTest, concurrency/permission/upgrade integration tests, preload recovery, physical standby replay, and PostgreSQL server log checks).
- **Release (`.github/workflows/release.yml`)**: Manually dispatched for an existing `v<version>` tag whose commit has passed `Check`. Builds and tests `.deb` and `.zip` packages for PostgreSQL 15–19 on `x86_64` and `aarch64` plus the PGXN source archive, then uploads them with SHA-256 checksums to a draft GitHub Release.
