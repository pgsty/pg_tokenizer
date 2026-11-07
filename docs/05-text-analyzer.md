# Text Analyzer

`text analyzer` has 3 main components, `character filters`, `pre tokenizer`, `token filters`.

- character filter: It is used to filter out some characters before tokenization. e.g. `to_lowercase`, `unicode_normalization`.
- pre tokenizer: It is used to split the text into tokens. e.g. `unicode segmentation` will split texts according to the [`Unicode Standard Annex #29`](https://unicode.org/reports/tr29/)
- token filter: It is used to filter out some tokens after tokenization. e.g. `stopwords`, `stemmer`.

## Character Filter

We support following character filters:

- `to_lowercase`: Convert all characters to lowercase.
- `unicode_normalization`: Normalize the text according to the [Unicode Normalization Forms](https://unicode.org/reports/tr15/) (NFC, NFD, NFKC, NFKD).

## Pre Tokenizer

We support following pre tokenizers:

- `regex`: Generate tokens by matching the regular expression.
- `unicode_segmentation`: Split the text into tokens according to the [`Unicode Standard Annex #29`](https://unicode.org/reports/tr29/).
- `jieba`: Chinese text segmentation using the [Jieba](https://github.com/messense/jieba-rs) library, in `search` (default), `precise`, or `full` mode; see [Options for `jieba`](00-reference.md#options-for-jieba). In 1.0.0 and earlier, `full` and `precise` were swapped; regenerate values produced with either mode.

## Token Filter

We support following token filters:

- `skip_non_alphanumeric`: Skip tokens where all characters are not alphanumeric.
- `stemmer`: Stem tokens using the [Snowball stemmer](https://snowballstem.org/) algorithm.
- `stopwords`: Filter out tokens that are in the stop words list.
- `synonym`: Replace tokens with their synonyms.
- `pg_dict`: Names are resolved and stored with their schema at creation in 1.1.0, and resolved again per batch after dictionary DDL. Process tokens using the [PostgreSQL dictionary](https://www.postgresql.org/docs/current/textsearch-dictionaries.html). You can integrate this with the PostgreSQL dictionary or other extensions that provide dictionaries.

### Supported algorithms for `stemmer`

arabic, armenian, basque, catalan, danish, dutch, english_porter, english_porter2, estonian, finnish, french, german, greek, hindi, hungarian, indonesian, irish, italian, lithuanian, nepali, norwegian, portuguese, romanian, russian, serbian, spanish, swedish, tamil, turkish, yiddish

### Customize dictionary for `stopwords` and `synonym`

We support customize `stopwords` and `synonym` by providing a dictionary.

```sql
-- Create a dictionary for stopwords, each line is a stopword.
SELECT create_stopwords('stop1', $$
it
is
an
$$);

SELECT tokenizer_catalog.create_text_analyzer('test_stopwords', $$
pre_tokenizer = "unicode_segmentation"
[[character_filters]]
to_lowercase = {}
[[token_filters]]
stopwords = "stop1"
$$);

SELECT tokenizer_catalog.apply_text_analyzer('It is an apple.', 'test_stopwords');
----
{apple}
```

```sql
-- Create a dictionary for synonyms, each line is a synonym.
SELECT create_synonym('syn1', $$
pgsql postgres postgresql
index indices
$$);

SELECT tokenizer_catalog.create_text_analyzer('test_synonym', $$
pre_tokenizer = "unicode_segmentation"
[[token_filters]]
synonym = "syn1"
$$);

SELECT tokenizer_catalog.apply_text_analyzer('postgresql indices', 'test_synonym');
----
{pgsql,index}
```
