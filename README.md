# pg_tokenizer

[![Website: vchord.pgsty.com](https://img.shields.io/badge/Website-vchord.pgsty.com-slategray?style=flat&logo=cilium&logoColor=white)](https://vchord.pgsty.com/)
[![Docs: pg_tokenizer](https://img.shields.io/badge/Docs-pg__tokenizer-slategray?style=flat)](https://vchord.pgsty.com/bm25/tokenizer/overview/)
[![PGEXT: pg_tokenizer](https://img.shields.io/badge/PGEXT-pg__tokenizer-%233E668F?style=flat&logo=postgresql&logoColor=white&labelColor=3E668F)](https://pgext.cloud/ext/pg_tokenizer)
[![Pigsty: pg_tokenizer](https://img.shields.io/badge/Pigsty-pg__tokenizer-slategray?style=flat)](https://pigsty.io/ext/e/pg_tokenizer)
[![Version: v0.1.1](https://img.shields.io/badge/Version-v0.1.1-slategray?style=flat)](https://github.com/pgsty/pg_tokenizer/releases)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-slategray?style=flat&logo=opensourceinitiative&logoColor=green)](https://github.com/pgsty/pg_tokenizer/blob/main/LICENSE)
[![GitHub Stars](https://img.shields.io/github/stars/pgsty/pg_tokenizer?style=flat&logo=github&logoColor=black&color=slategray)](https://star-history.com/#pgsty/pg_tokenizer&Date)
[![PostgreSQL: 13-18](https://img.shields.io/badge/PostgreSQL-13--18-%233E668F?style=flat&logo=postgresql&logoColor=white&labelColor=3E668F)](https://pgext.cloud/ext/pg_tokenizer)

**`pg_tokenizer`** is a standalone multilingual text analyzer and vocabulary tokenizer extension for PostgreSQL, designed to power full-text BM25 ranking with [`vchord_bm25`](https://github.com/pgsty/vchord_bm25) and hybrid retrieval with [`vchord`](https://github.com/pgsty/vchord).

[Website](https://vchord.pgsty.com/) | [Tokenizer Docs](https://vchord.pgsty.com/bm25/tokenizer/overview/) | [BM25 Docs](https://vchord.pgsty.com/bm25/) | [vchord](https://github.com/pgsty/vchord) | [vchord_bm25](https://github.com/pgsty/vchord_bm25) | [PGEXT](https://pgext.cloud/ext/pg_tokenizer) | [Pigsty](https://pigsty.io/ext/e/pg_tokenizer) ([中文](https://pigsty.cc/ext/e/pg_tokenizer)) | [Releases](https://github.com/pgsty/pg_tokenizer/releases) | [Issues](https://github.com/pgsty/pg_tokenizer/issues)

---

## Vchord Suite

`pg_tokenizer` is part of the **Vchord** extension family maintained by [PGSTY](https://pgsty.com):

- **[`vchord`](https://github.com/pgsty/vchord)** ([`pgsty/vchord`](https://github.com/pgsty/vchord) · [PGEXT](https://pgext.cloud/ext/vchord) · [Pigsty](https://pigsty.io/ext/e/vchord)): Scalable, high-performance, and disk-friendly vector similarity search (`vchordrq` & `vchordg`).
- **[`vchord_bm25`](https://github.com/pgsty/vchord_bm25)** ([`pgsty/vchord_bm25`](https://github.com/pgsty/vchord_bm25) · [PGEXT](https://pgext.cloud/ext/vchord_bm25) · [Pigsty](https://pigsty.io/ext/e/vchord_bm25)): Native BM25 full-text ranking index (`Block-WeakAnd`).
- **[`pg_tokenizer`](https://github.com/pgsty/pg_tokenizer)** ([`pgsty/pg_tokenizer`](https://github.com/pgsty/pg_tokenizer) · [PGEXT](https://pgext.cloud/ext/pg_tokenizer) · [Pigsty](https://pigsty.io/ext/e/pg_tokenizer)): Standalone multilingual text analyzer and vocabulary tokenizer.

---

## Features

- **Modular Text Analysis Pipeline**: Chain character filters (`to_lowercase`, `unicode_normalization`), pre-tokenizers (`unicode_segmentation`, `regex`, Chinese `jieba`), and token filters (`stopwords`, Snowball `stemmer`, `synonym`, `pg_dict`, `ngram`) via declarative TOML configurations.
- **Built-in & Hugging Face Vocabularies**: Ship with built-in pre-trained models (`bert_base_uncased`, `wiki_tocken`, `gemma2b`, `llmlingua2`) or import any Hugging Face `tokenizer.json` directly into PostgreSQL.
- **Corpus-Trained Custom Models**: Automatically build domain-specific vocabularies from your own table columns and maintain token embeddings via database triggers (`create_custom_model_tokenizer_and_trigger`).
- **Multilingual Support**: Native support for English, European languages (30 Snowball stemmers), Chinese (`jieba`), Japanese/Korean (`lindera`), and multilingual LLM vocabularies.

---

## Quick Start

For full setup instructions (Debian/ZIP packages and building from source), see the [Installation Guide](https://vchord.pgsty.com/bm25/getting-started/installation/) or [docs/01-installation.md](docs/01-installation.md).

### 1. Install via Native Linux Packages ([`pig`](https://pig.pgsty.com/))

On EL 8/9/10, Debian 12/13, and Ubuntu 22.04/24.04 (PostgreSQL 14–18), install pre-built native packages from the [Pigsty repository](https://pigsty.io/ext/e/pg_tokenizer) (see package matrix on [pgext.cloud](https://pgext.cloud/ext/pg_tokenizer)):

```bash
curl -fsSL https://repo.pigsty.io/pig | bash
pig repo add pgsql -u
pig ext add pg_tokenizer
```

Then add `pg_tokenizer.so` to `shared_preload_libraries` and restart PostgreSQL:

```bash
psql -U postgres -c 'ALTER SYSTEM SET shared_preload_libraries = "pg_tokenizer.so"'
sudo systemctl restart postgresql
```

### 2. Run with Docker

You can also start a ready-to-use container with the [`tensorchord/vchord-suite`](https://vchord.pgsty.com/docs/getting-started/vectorchord-suite/) image, which bundles PostgreSQL, [`vchord`](https://github.com/pgsty/vchord), [`vchord_bm25`](https://github.com/pgsty/vchord_bm25), and [`pg_tokenizer`](https://github.com/pgsty/pg_tokenizer):

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

---

## Documentation

Full online documentation is available at **[vchord.pgsty.com/bm25/tokenizer](https://vchord.pgsty.com/bm25/tokenizer/overview/)**, alongside the local docs in [`docs/`](docs/):

- [Tokenizer Overview & Usage](https://vchord.pgsty.com/bm25/tokenizer/overview/) ([docs/04-usage.md](docs/04-usage.md))
- [Models (Built-in, Custom, Hugging Face, Lindera)](https://vchord.pgsty.com/bm25/tokenizer/models/) ([docs/06-model.md](docs/06-model.md))
- [Text Analyzer Pipeline](https://vchord.pgsty.com/bm25/tokenizer/text-analyzer/) ([docs/05-text-analyzer.md](docs/05-text-analyzer.md))
- [Multilingual Segmentation & Examples](https://vchord.pgsty.com/bm25/tokenizer/multilingual/) ([docs/03-examples.md](docs/03-examples.md))
- [SQL & Configuration Reference](https://vchord.pgsty.com/bm25/reference/sql/) ([docs/00-reference.md](docs/00-reference.md))
- [Installation](docs/01-installation.md) · [Development](docs/02-development.md) · [Limitations](docs/07-limitation.md)

---

## License

This software is licensed under the **Apache License 2.0 ([Apache-2.0](LICENSE))**.

- Copyright (c) 2025-2026 TensorChord Inc.
- Copyright (c) 2026 PGSTY PTE. LTD.
- Maintained by [Ruohang Feng](mailto:rh@vonng.com) @ [PGSTY](https://pgsty.com)
