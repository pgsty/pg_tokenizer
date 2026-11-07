# Reference

## Functions

### Text Analyzer

- `tokenizer_catalog.create_text_analyzer(name TEXT, config TEXT)`: Create a text analyzer.
- `tokenizer_catalog.drop_text_analyzer(name TEXT)`: Drop a text analyzer.
- `tokenizer_catalog.apply_text_analyzer(text TEXT, text_analyzer_name TEXT) RETURNS TEXT[]`: Apply a text analyzer to a text.

<br/>

- `tokenizer_catalog.create_stopwords(name TEXT, config TEXT)`: Create a stopwords dictionary.
- `tokenizer_catalog.drop_stopwords(name TEXT)`: Drop a stopwords dictionary. The built-in `lucene_english`, `nltk_english`, and `iso_english` lists cannot be dropped, and `create_stopwords` rejects their names.

<br/>

- `tokenizer_catalog.create_synonym(name TEXT, config TEXT)`: Create a synonym dictionary.
- `tokenizer_catalog.drop_synonym(name TEXT)`: Drop a synonym dictionary.

### Model

- `tokenizer_catalog.create_custom_model(name TEXT, config TEXT)`: Create a custom model.
- `tokenizer_catalog.create_custom_model_tokenizer_and_trigger(tokenizer_name TEXT, model_name TEXT, text_analyzer_name TEXT, table_name TEXT, source_column TEXT, target_column TEXT [, filter TEXT])`: Create a custom model tokenizer and trigger. The optional seventh argument restricts vocabulary and embedding updates; omit it or pass `NULL` to include all rows (1.0.0). Six-argument calls remain supported.
- `tokenizer_catalog.drop_custom_model(name TEXT)`: Drop a custom model. A missing model raises a WARNING, like the other drop functions; tokenizers that reference the model are not dropped.

<br/>

- `tokenizer_catalog.add_preload_model(name TEXT)`: Add a model to the cluster-wide preload list (administrator only; nontransactional).
- `tokenizer_catalog.remove_preload_model(name TEXT)`: Remove a model from the cluster-wide preload list (administrator only; nontransactional).
- `tokenizer_catalog.list_preload_models() RETURNS TEXT[]`: List all preload models.

<br/>

- `tokenizer_catalog.create_lindera_model(name TEXT, config TEXT)`: Create a Lindera model. Lindera 6 dictionary URIs and legacy dictionary tables are supported; see [Lindera model configuration and upgrade notes](06-model.md#lindera-model).
- `tokenizer_catalog.drop_lindera_model(name TEXT)`: Drop a lindera model.

<br/>

- `tokenizer_catalog.create_huggingface_model(name TEXT, config TEXT)`: Create a huggingface model. Token IDs (vocabulary, added tokens, and post-processor special tokens) must not exceed 2147483647.
- `tokenizer_catalog.drop_huggingface_model(name TEXT)`: Drop a huggingface model.

### Tokenizer

- `tokenizer_catalog.create_tokenizer(name TEXT, config TEXT)`: Create a tokenizer.
- `tokenizer_catalog.drop_tokenizer(name TEXT)`: Drop a tokenizer.
- `tokenizer_catalog.tokenize(text TEXT, tokenizer_name TEXT) RETURNS INT[]`: Tokenize a text.
- `tokenizer_catalog.analyze_to_tsvector(text TEXT, text_analyzer_name TEXT) RETURNS TSVECTOR`: Convert the analyzer's string tokens to native lexemes with positions (1.0.0).
- `tokenizer_catalog.tokenize_to_tsvector(text TEXT, tokenizer_name TEXT) RETURNS TSVECTOR`: Convert model token IDs to decimal lexemes with positions (1.0.0). Use the same tokenizer for documents and queries; see [native full-text search](04-usage.md#native-postgresql-full-text-search).


## Configuration

> We utilize `TOML` syntax to express all configurations. Configurations that cannot be parsed or validated, including invalid regular expressions, raise SQLSTATE `22023` (`invalid_parameter_value`) with the parser's message as DETAIL.

### Analysis resource budgets (1.0.1)

These positive session settings reject oversized input or expansion with SQLSTATE `54000`; results are never silently truncated. See [cancellation and resource use](07-limitation.md#cancellation-and-resource-use) for the accounting model and remaining third-party library limits.

| Setting | Default | Scope |
| --- | ---: | --- |
| `pg_tokenizer.max_input_bytes` | 16,777,216 | UTF-8 bytes in one analyzer input |
| `pg_tokenizer.max_output_tokens` | 1,000,000 | Tokens or model IDs in one stage |
| `pg_tokenizer.max_output_bytes` | 67,108,864 | Token bytes plus descriptors, ID bytes, character output or ngram offsets |
| `pg_tokenizer.max_piece_bytes` | 1,048,576 | Input to one regex or opaque library/dictionary call |

### Options for `text analyzer`

| Key               | Type           | Description                                                                            |
| ----------------- | -------------- | -------------------------------------------------------------------------------------- |
| character_filters | Array of Table | Character filters, see [Options for `character_filter`](#options-for-character_filter) |
| pre_tokenizer     | Table          | Pre-tokenizer, see [Options for `pre_tokenizer`](#options-for-pre_tokenizer)           |
| token_filters     | Array of Table | Token filters, see [Options for `token_filter`](#options-for-token_filter)             |

### Options for `character_filter`

| Key                   | Type        | Description                                                                                                                                      |
| --------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| to_lowercase          | Empty Table | Convert all characters to lowercase                                                                                                              |
| unicode_normalization | String      | Unicode normalization form, see [Unicode Normalization Forms](https://unicode.org/reports/tr15/), supported values: `nfkc`, `nfkd`, `nfc`, `nfd` |

You can choose only one of the above options for each character filter.

### Options for `pre_tokenizer`

| Key                  | Type        | Description                                                                                                    |
| -------------------- | ----------- | -------------------------------------------------------------------------------------------------------------- |
| regex                | String      | It will generate all tokens that match the regex pattern                                                       |
| unicode_segmentation | Empty Table | Split the text into tokens based on the Unicode Standard Annex #29                                             |
| jieba                | Table       | Split the text into tokens based on the Jieba Chinese tokenizer, see [Options for `jieba`](#options-for-jieba) |

#### Options for `jieba`

| Key        | Type    | Description                                                                            |
| ---------- | ------- | -------------------------------------------------------------------------------------- |
| mode       | String  | Jieba tokenizer mode, supported values: `full`, `precise`, `search`. default: `search` |
| enable_hmm | Boolean | Whether to use the HMM for unknown words in `precise` and `search` modes, default: `true`; `full` mode ignores it |

- `full`: every dictionary word found in the text, overlapping (Jieba `cut_all`).
- `precise`: the most likely segmentation, without overlaps (Jieba `cut`).
- `search`: precise segmentation plus the dictionary words inside long words, for search indexing (Jieba `cut_for_search`).

In 1.0.0 and earlier, `full` and `precise` were swapped: `full` produced precise segmentation (and honoured `enable_hmm`), while `precise` produced full-mode output. Analyzers that set `mode = "full"` or `mode = "precise"` now produce different tokens; regenerate stored token arrays, custom-model embeddings and `tsvector` values built with them, and rebuild dependent indexes. The default `search` mode is unchanged.

### Options for `token_filter`

| Key                   | Type        | Description                                                                                                                                                                         |
| --------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| skip_non_alphanumeric | Empty Table | Skip tokens that all characters are non-alphanumeric                                                                                                                                |
| stemmer               | String      | Stemmer, see [Supported values for `stemmer`](#supported-values-for-stemmer)                                                                                                        |
| stopwords             | String      | Stopwords name, builtin: `lucene_english`, `nltk_english`, `iso_english`                                                                                                            |
| synonym               | String      | Synonym name                                                                                                                                                                        |
| pg_dict               | String      | Using [postgres text search dictionary](https://www.postgresql.org/docs/current/textsearch-dictionaries.html). Each token is looked up on its own, as `ts_lexize` does; a thesaurus applies only single-word entries. |
| ngram                 | Table       | N-gram tokenizer, see [Options for `ngram`](#options-for-ngram)                                                                                                                     |

You can choose only one of the above options for each token filter.

#### Supported values for `stemmer`

arabic, armenian, basque, catalan, danish, dutch, english_porter, english_porter2, estonian, finnish, french, german, greek, hindi, hungarian, indonesian, irish, italian, lithuanian, nepali, norwegian, portuguese, romanian, russian, serbian, spanish, swedish, tamil, turkish, yiddish

#### Options for `ngram`

| Key               | Type    | Description                                              |
| ----------------- | ------- | -------------------------------------------------------- |
| max_gram          | Integer | Maximum n-gram size, range: `1..=255`, default: `2`      |
| min_gram          | Integer | Minimum n-gram size, range: `1..=255`, default: `1`      |
| preserve_original | Boolean | Whether to preserve the original token, default: `false` |

Lengths count Unicode characters, not bytes. A token shorter than `min_gram` produces no n-grams, unless `preserve_original` keeps the token itself.

### Options for `tokenizer`

| Key           | Type   | Description                                                                       |
| ------------- | ------ | --------------------------------------------------------------------------------- |
| text_analyzer | String | Text analyzer name. If you are using an external model, you can just ignore this. |
| model         | String | Model name. We have some builtin models, see [Builtin models](#builtin-models)    |

#### Builtin models

- [bert-base-uncased](https://huggingface.co/google-bert/bert-base-uncased)
- [wiki_tocken](https://huggingface.co/datasets/iohadrubin/wikitext-103-raw-v1)
- [gemma2b](https://huggingface.co/google/gemma-2b)
- [llmlingua2](https://huggingface.co/microsoft/llmlingua-2-xlm-roberta-large-meetingbank)

### Options for `custom model`

| Key           | Type   | Description         |
| ------------- | ------ | ------------------- |
| table         | String | Table name, optionally schema-qualified. |
| column        | String | Column name.        |
| text_analyzer | String | Text analyzer name. |
| filter        | String | Optional SQL Boolean expression over source columns, without `WHERE`. Default: all rows (1.0.0). |

### Options for `lindera model`

It's totally the same as lindera tokenizer configs, see [Lindera](https://github.com/lindera/lindera).
