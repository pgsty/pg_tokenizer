# pg_tokenizer

[![Website: vchord.pgsty.com](https://img.shields.io/badge/Website-vchord.pgsty.com-slategray?style=flat&logo=cilium&logoColor=white)](https://vchord.pgsty.com/)
[![Docs: pg_tokenizer](https://img.shields.io/badge/Docs-pg__tokenizer-slategray?style=flat)](https://vchord.pgsty.com/docs/tokenizer/)
[![Catalog: pg_tokenizer](https://img.shields.io/badge/Catalog-pg__tokenizer-%233E668F?style=flat&logo=postgresql&logoColor=white&labelColor=3E668F)](https://pgext.cloud/ext/pg_tokenizer)
[![Version: v1.0.0](https://img.shields.io/badge/Version-v1.0.0-slategray?style=flat)](https://github.com/pgsty/pg_tokenizer/releases)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-slategray?style=flat&logo=opensourceinitiative&logoColor=green)](https://github.com/pgsty/pg_tokenizer/blob/main/LICENSE)
[![GitHub Stars](https://img.shields.io/github/stars/pgsty/pg_tokenizer?style=flat&logo=github&logoColor=black&color=slategray)](https://star-history.com/#pgsty/pg_tokenizer&Date)
[![PostgreSQL: 14-18](https://img.shields.io/badge/PostgreSQL-14--18-%233E668F?style=flat&logo=postgresql&logoColor=white&labelColor=3E668F)](https://pgext.cloud/ext/pg_tokenizer)

**`pg_tokenizer`** is a standalone multilingual text analyzer and vocabulary tokenizer extension for PostgreSQL, designed to power full-text BM25 ranking with [`vchord_bm25`](https://github.com/pgsty/vchord_bm25) and hybrid retrieval with [`vchord`](https://github.com/pgsty/vchord).

[Website](https://vchord.pgsty.com/) | [Documentation](https://vchord.pgsty.com/docs/tokenizer/) | [vchord](https://github.com/pgsty/vchord) | [vchord_bm25](https://github.com/pgsty/vchord_bm25) | [Blog](https://vchord.pgsty.com/blog/) | [PGEXT](https://pgext.cloud/ext/pg_tokenizer) | [Releases](https://github.com/pgsty/pg_tokenizer/releases) | [Issues](https://github.com/pgsty/pg_tokenizer/issues) | [Upstream](https://github.com/supervc-stack/pg_tokenizer.rs)

---

## Vchord Suite

`pg_tokenizer` is part of the **Vchord** extension family maintained by [PGSTY](https://pgsty.com):

- **[`vchord`](https://github.com/pgsty/vchord)** ([Docs](https://vchord.pgsty.com/docs/vchord/) · [PGEXT](https://pgext.cloud/ext/vchord) · [Pigsty](https://pigsty.io/ext/e/vchord)): Scalable, high-performance, and disk-friendly vector similarity search (`vchordrq` & `vchordg`).
- **[`vchord_bm25`](https://github.com/pgsty/vchord_bm25)** ([Docs](https://vchord.pgsty.com/docs/bm25/) · [PGEXT](https://pgext.cloud/ext/vchord_bm25) · [Pigsty](https://pigsty.io/ext/e/vchord_bm25)): Native BM25 full-text ranking index (`Block-WeakAnd`).
- **[`pg_tokenizer`](https://github.com/pgsty/pg_tokenizer)** ([Docs](https://vchord.pgsty.com/docs/tokenizer/) · [PGEXT](https://pgext.cloud/ext/pg_tokenizer) · [Pigsty](https://pigsty.io/ext/e/pg_tokenizer)): Standalone multilingual text analyzer and vocabulary tokenizer.

---

## Features

- **Modular Text Analysis Pipeline**: Chain character filters (`to_lowercase`, `unicode_normalization`), pre-tokenizers (`unicode_segmentation`, `regex`, Chinese `jieba`), and token filters (`stopwords`, Snowball `stemmer`, `synonym`, `pg_dict`, `ngram`) via declarative TOML configurations.
- **Built-in & Hugging Face Vocabularies**: Ship with built-in pre-trained models (`bert_base_uncased`, `wiki_tocken`, `gemma2b`, `llmlingua2`) or import any Hugging Face `tokenizer.json` directly into PostgreSQL.
- **Corpus-Trained Custom Models**: Automatically build domain-specific vocabularies from your own table columns and maintain token embeddings via database triggers (`create_custom_model_tokenizer_and_trigger`).
- **Multilingual Support**: Native support for English, European languages (30 Snowball stemmers), Chinese (`jieba`), Japanese/Korean (`lindera`), and multilingual LLM vocabularies.

---

## Version 1.0.0

Version 1.0.0 is the first PGSTY release. It builds with pgrx 0.19.3 for PostgreSQL 14–18 and moves to Lindera 6.2, Jieba 0.11, and Hugging Face tokenizers 0.23. It adds native PostgreSQL `tsvector` output, filtered custom vocabularies with schema-qualified source tables, safe concurrent vocabulary writes, database-owner privileges, transaction-aware definition caches, preload models that survive cache invalidation and broken entries, and definition tables included in `pg_dump`. It fixes `ngram` for non-ASCII and short tokens, calls `pg_dict` dictionaries the way `ts_lexize` does, reserves the built-in stopword lists, and takes its Snowball stemmers from [`pgsty/rust-stemmers`](https://github.com/pgsty/rust-stemmers) 1.2.1 with unchanged output. Existing `tokenize()` calls still return `INT[]`.

See [custom models](docs/06-model.md#custom-model), [native full-text search](docs/04-usage.md#native-postgresql-full-text-search), and [upgrade instructions](docs/01-installation.md#upgrading-to-100).

## Quick Start

For full setup instructions (Debian/ZIP packages and building from source), see the [Installation Guide](https://vchord.pgsty.com/docs/tokenizer/installation/) or [docs/01-installation.md](docs/01-installation.md).

### 1. Install via Native Linux Packages ([`pig`](https://pig.pgsty.com/))

On EL 8/9/10, Debian 12/13, and Ubuntu 22.04/24.04/26.04 (PostgreSQL 14–18), install the native package built for your distribution and architecture from the [Pigsty repository](https://pigsty.io/ext/e/pg_tokenizer) (see the package matrix on [pgext.cloud](https://pgext.cloud/ext/pg_tokenizer)):

```bash
curl -fsSL https://repo.pigsty.io/pig | bash
pig repo add pgsql -u
pig ext add pg_tokenizer
```

Native RPM/DEB builds do not embed Lindera dictionaries; supply a dictionary built with Lindera 6. GitHub release packages enable `lindera-ipadic` and include IPADIC. Other embedded dictionaries require their corresponding build features.

Then add `pg_tokenizer.so` to `shared_preload_libraries` and restart PostgreSQL:

```bash
psql -U postgres -c "ALTER SYSTEM SET shared_preload_libraries = 'pg_tokenizer.so';"
sudo systemctl restart postgresql
```

### 2. Run with Docker

You can also start a ready-to-use container with the [`tensorchord/vchord-suite`](https://vchord.pgsty.com/docs/vchord/getting-started/vectorchord-suite/) image, which bundles PostgreSQL, [`vchord`](https://github.com/pgsty/vchord), [`vchord_bm25`](https://github.com/pgsty/vchord_bm25), and [`pg_tokenizer`](https://github.com/pgsty/pg_tokenizer):

```bash
docker run \
  --name vchord-suite \
  -e POSTGRES_PASSWORD=postgres \
  -p 5432:5432 \
  -d tensorchord/vchord-suite:pg18-latest
  # Or from GHCR: ghcr.io/tensorchord/vchord-suite:pg18-latest
```

Connect to the database using `psql`:

```bash
psql -h localhost -p 5432 -U postgres
```

### 3. Basic SQL Usage

Enable the extension and add `tokenizer_catalog` to `search_path`:

```sql
CREATE EXTENSION IF NOT EXISTS pg_tokenizer;
SET search_path TO "$user", public, tokenizer_catalog;
```

Create a tokenizer backed by the built-in `llmlingua2` model and tokenize text into an integer array of token IDs:

```sql
SELECT create_tokenizer('tokenizer1', $$
model = "llmlingua2"
$$);

SELECT tokenize(
    'PostgreSQL is a powerful, open-source object-relational database system. It has over 15 years of active development.',
    'tokenizer1'
);
```

More SQL examples (custom corpus models, Chinese `jieba`, Japanese `lindera`) can be found in [docs/03-examples.md](docs/03-examples.md).

When upgrading from Lindera 0.42 to 6.2, regenerate stored Lindera token arrays and rebuild their search indexes: the embedded IPADIC dictionary now assigns different token IDs. Existing model configurations remain accepted; see the [Lindera upgrade notes](docs/06-model.md#lindera-model).

---

## Documentation

Full online documentation is available at **[vchord.pgsty.com/docs/tokenizer](https://vchord.pgsty.com/docs/tokenizer/)**, alongside the local docs in [`docs/`](docs/):

- [Installation Guide](https://vchord.pgsty.com/docs/tokenizer/installation/) ([docs/01-installation.md](docs/01-installation.md))
- [Tokenizer Overview & Usage](https://vchord.pgsty.com/docs/tokenizer/overview/) ([docs/04-usage.md](docs/04-usage.md))
- [Text Analyzer Pipeline](https://vchord.pgsty.com/docs/tokenizer/text-analyzer/) ([docs/05-text-analyzer.md](docs/05-text-analyzer.md))
- [Models (Built-in, Custom, Hugging Face, Lindera)](https://vchord.pgsty.com/docs/tokenizer/models/) ([docs/06-model.md](docs/06-model.md))
- [Multilingual Segmentation & Examples](https://vchord.pgsty.com/docs/tokenizer/multilingual/) ([docs/03-examples.md](docs/03-examples.md))
- [SQL & Configuration Reference](https://vchord.pgsty.com/docs/tokenizer/reference/) ([docs/00-reference.md](docs/00-reference.md))
- [Development](docs/02-development.md) · [Limitations](docs/07-limitation.md)

---

## License

This software is licensed under the **Apache License 2.0 ([Apache-2.0](LICENSE))**.

- Copyright (c) 2025 TensorChord Inc.
- Copyright (c) 2026 PGSTY PTE. LTD.
- Maintained by [Ruohang Feng](mailto:rh@vonng.com) @ [PGSTY](https://pgsty.com)
