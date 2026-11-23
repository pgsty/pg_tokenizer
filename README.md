# pg_tokenizer

[![Website: vchord.pgsty.com](https://img.shields.io/badge/Website-vchord.pgsty.com-slategray?style=flat&logo=cilium&logoColor=white)](https://vchord.pgsty.com/)
[![Docs: tokenizer](https://img.shields.io/badge/Docs-tokenizer-slategray?style=flat)](https://vchord.pgsty.com/docs/tokenizer/)
[![Catalog: tokenizer](https://img.shields.io/badge/Catalog-tokenizer-%233E668F?style=flat&logo=postgresql&logoColor=white&labelColor=3E668F)](https://pgext.cloud/ext/pg_tokenizer)
[![Version: 1.2.0](https://img.shields.io/badge/Version-1.2.0-slategray?style=flat)](docs/release-notes/1.2.0.md)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-slategray?style=flat&logo=opensourceinitiative&logoColor=green)](https://github.com/pgsty/pg_tokenizer/blob/main/LICENSE)
[![GitHub Stars](https://img.shields.io/github/stars/pgsty/pg_tokenizer?style=flat&logo=github&logoColor=black&color=slategray)](https://star-history.com/#pgsty/pg_tokenizer&Date)
[![PostgreSQL: 15-19](https://img.shields.io/badge/PostgreSQL-15--19-%233E668F?style=flat&logo=postgresql&logoColor=white&labelColor=3E668F)](#build-from-source)

pg_tokenizer 1.2.0 provides multilingual text analysis and vocabulary tokenization for PostgreSQL 15–19 on `x86_64` and `aarch64`.

[Documentation](https://vchord.pgsty.com/docs/tokenizer/) · [Releases](https://github.com/pgsty/pg_tokenizer/releases) · [Issues](https://github.com/pgsty/pg_tokenizer/issues)

## Features

- Configure character filters, pre-tokenizers, and token filters through TOML.
- Analyze European languages with Snowball stemmers, Chinese with Jieba, and Japanese or Korean with Lindera.
- Use built-in vocabularies, Hugging Face models, or custom vocabularies trained on table data.
- Return text tokens, integer token IDs, or positioned `tsvector` values for [vchord_bm25](https://github.com/pgsty/vchord_bm25) and PostgreSQL GIN/GiST indexes.
- `to_lowercase` reuses ASCII input that has no uppercase letters and allocates an output string when it converts uppercase ASCII. The `stopwords`, `skip_non_alphanumeric`, and `synonym` batch filters reuse the input token vector. A longer synonym can still grow an individual token's string buffer.

## Installation

Install native Linux packages from the [Pigsty repository](https://pigsty.io/ext/e/pg_tokenizer):

```bash
pig repo add pgsql -u
pig ext add pg_tokenizer
```

The GitHub release workflow builds Linux `.deb` and `.zip` packages on Ubuntu 24.04 for PostgreSQL 15–19 on both architectures, plus a source archive. The binaries target `glibc >= 2.39` and embed the IPADIC dictionary. On older Linux distributions, install a native package or build from source. See [GitHub Releases](https://github.com/pgsty/pg_tokenizer/releases) and the [installation guide](docs/01-installation.md).

Add `pg_tokenizer` to `shared_preload_libraries`, restart PostgreSQL, then enable the extension:

```sql
CREATE EXTENSION pg_tokenizer;
```

## Usage

Create a text analyzer and convert text into tokens or a `tsvector`:

```sql
SELECT tokenizer_catalog.create_text_analyzer('words', $$
pre_tokenizer = "unicode_segmentation"
[[character_filters]]
to_lowercase = {}
$$);

SELECT tokenizer_catalog.apply_text_analyzer('PostgreSQL Full Text Search', 'words');
SELECT tokenizer_catalog.analyze_to_tsvector('PostgreSQL Full Text Search', 'words');
```

Use a vocabulary model to produce integer token IDs:

```sql
SELECT tokenizer_catalog.create_tokenizer('bert', $$
model = "bert_base_uncased"
$$);

SELECT tokenizer_catalog.tokenize('PostgreSQL Full Text Search', 'bert');
SELECT tokenizer_catalog.tokenize_to_tsvector('PostgreSQL Full Text Search', 'bert');
```

Store analyzed vectors in a column and build a BM25 index with `vchord_bm25`. Use the same analyzer or tokenizer for documents and queries. See [examples](docs/03-examples.md), [pipeline configuration](docs/05-text-analyzer.md), and the [SQL reference](docs/00-reference.md).

## Build from Source

The repository pins Rust 1.100.0 for development, CI, and release builds. Source builds support Rust 1.99.1 or later and require Python 3.11+, GNU Make, PostgreSQL 15–19 development headers, matching Clang, libclang, and LLVM tools, and `cargo-pgrx` 0.20.0 to match pgrx 0.20.0.

```bash
git clone --branch v1.2.0 --depth 1 https://github.com/pgsty/pg_tokenizer.git
cd pg_tokenizer
cargo install cargo-pgrx --version 0.20.0 --locked
PG_CONFIG=/path/to/pg_config make build
sudo make PG_CONFIG=/path/to/pg_config install
```

The Makefile embeds IPADIC. Other dictionaries are optional Cargo features. External system dictionaries must use Lindera 7 format v3. Set `CLANG_PATH` and `LIBCLANG_PATH` if multiple LLVM versions are installed, and `CLANG_NO_DEFAULT_CONFIG=1` if Clang loads a default configuration file.

## Upgrading

Install the new library and SQL files, restart PostgreSQL on the primary and all replicas, then run in each database:

```sql
ALTER EXTENSION pg_tokenizer UPDATE;
```

Upgrading from `1.1.0` to `1.2.0` preserves catalog definitions, custom vocabulary IDs, and the preload file format. Replace external Lindera system dictionaries on every node and migrate preload symlinks to real directories and regular files before restart. If Lindera segmentation, regex matching, or updated Unicode tables change your tokens, regenerate stored output from the source text before switching queries to the new pipeline. PostgreSQL 14 installations must first move to a supported server major. See the [1.2.0 release notes](docs/release-notes/1.2.0.md) and the [upgrade guide](docs/01-installation.md#upgrading).

## Testing

```bash
cargo test --locked --features 'pg19 lindera-ipadic'
cargo clippy --locked --features 'pg19 lindera-ipadic'
sqllogictest --db "$USER" --user "$USER" './tests/**/*.slt'
```

SQL tests require a running PostgreSQL instance with the extension preloaded and installed. See [development](docs/02-development.md) for the integration, preload, and standby test suites.

## License

[Apache License 2.0](LICENSE).

Copyright (c) 2025 TensorChord Inc. and 2026 PGSTY PTE. LTD. Maintained by [Ruohang Feng](mailto:rh@vonng.com) at [PGSTY](https://pgsty.com).
