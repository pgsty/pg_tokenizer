# Development

## Set up development environment


1. Install Rust

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. Clone the Repository

```sh
git clone https://github.com/pgsty/pg_tokenizer
cd pg_tokenizer
```

3. Install [`cargo-pgrx`](https://github.com/pgcentralfoundation/pgrx)

```sh
cargo install cargo-pgrx --version $(grep -o 'pgrx = "=[^"]*' Cargo.toml | cut -d = -f 3)
cargo pgrx init
```

## Debug

Debug information is enabled in `debug` and `dev-opt` profiles. You can build the extension with debug information by specifying the profile. And then you can use `gdb` to debug the extension.

```sh
cargo pgrx package --debug --features pg18 # no compiler optimization
cargo pgrx package --profile dev-opt --features pg18 # with compiler optimization
```

When setting environment variable `RUST_BACKTRACE=1`, you can get a backtrace when the program panics.

```sh
RUST_BACKTRACE=1 cargo pgrx run --profile dev-opt
```

## CI and release packages

The Check workflow tests PostgreSQL 14–18 on x86_64 and aarch64. It runs Rust
checks and unit tests, then installs the extension with `lindera-ipadic` and runs
the SQL, concurrency, permission, and upgrade suites. Matrix failures do not
cancel the remaining platforms.

Do not add `--no-gc-sections` to global `RUSTFLAGS` or Cargo configuration. It
prevents unit-test executables from discarding unused PostgreSQL references and
causes undefined-symbol linker failures. With cargo-pgrx 0.19.3, the schema,
install, and package commands handle schema-section retention for the extension
library separately. `tools/package.sh` generates the schema and library from the
same locked release build, with the same IPADIC feature as CI.

The Release workflow is manual. Commit the changes and version-specific install
and upgrade SQL, create a `v<version>` tag, and wait for that commit's Check run to
succeed. Run Release with either `v<version>` or `<version>` as its input. It
resolves the existing tag to a commit, checks the Cargo version and install SQL,
and builds all ten PostgreSQL/architecture combinations on Ubuntu 24.04.

Only after every build succeeds does it upload the DEB and ZIP files to a draft
GitHub Release. Review the test results, package compatibility, and release notes
before publishing the draft. The upload job has `contents: write`; build jobs
have read-only repository access. A moved tag or an already published release
stops the upload. Do not move an existing release tag to include later fixes;
prepare a new version and tag instead.
