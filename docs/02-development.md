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
cargo pgrx build # no compiler optimization
cargo pgrx build --profile dev-opt # with compiler optimization
```

When setting environment variable `RUST_BACKTRACE=1`, you can get a backtrace when the program panics.

```sh
RUST_BACKTRACE=1 cargo pgrx run --profile dev-opt
```
