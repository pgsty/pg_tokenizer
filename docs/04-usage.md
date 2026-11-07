# Usage

- using pre-trained model

```sql
-- 1. create a tokenizer
SELECT create_tokenizer('tokenizer1', $$
model = "llmlingua2"
$$);

-- 2. tokenize using defined tokenizer
SELECT tokenize('PostgreSQL is a powerful, open-source object-relational database system. It has over 15 years of active development.', 'tokenizer1');

-- For setting up a table
UPDATE table_name SET target_column_name = tokenize(source_column_name, 'tokenizer1');
```

- using custom model

```sql
-- 1. setup the document table
CREATE TABLE documents (
    id SERIAL PRIMARY KEY,
    passage TEXT,
    embedding INT[]
);

-- 2. create a text analyzer to generate tokens that can be used to train the model
SELECT create_text_analyzer('text_analyzer1', $$
pre_tokenizer = "unicode_segmentation"  # split texts according to the Unicode Standard Annex #29
[[character_filters]]
to_lowercase = {}                       # convert all characters to lowercase
[[character_filters]]
unicode_normalization = "nfkd"          # normalize the text to Unicode Normalization Form KD
[[token_filters]]
skip_non_alphanumeric = {}              # skip tokens that all characters are not alphanumeric
[[token_filters]]
stopwords = "nltk_english"              # remove stopwords using the nltk dictionary
[[token_filters]]
stemmer = "english_porter2"             # stem tokens using the English Porter2 stemmer
$$);

-- 3. create a model to generate embeddings from original passage
-- It'll train a model from passage column and store the embeddings in the embedding column
SELECT create_custom_model_tokenizer_and_trigger(
    tokenizer_name => 'tokenizer1',
    model_name => 'model1',
    text_analyzer_name => 'text_analyzer1',
    table_name => 'documents',
    source_column => 'passage',
    target_column => 'embedding'
);

-- 4. now you can insert some data and embeddings will be generated automatically
INSERT INTO documents (passage) VALUES 
('PostgreSQL is a powerful, open-source object-relational database system. It has over 15 years of active development.'),
('Full-text search is a technique for searching in plain-text documents or textual database fields. PostgreSQL supports this with tsvector.'),
('BM25 is a ranking function used by search engines to estimate the relevance of documents to a given search query.'),
('PostgreSQL provides many advanced features like full-text search, window functions, and more.'),
('Search and ranking in databases are important in building effective information retrieval systems.'),
('The BM25 ranking algorithm is derived from the probabilistic retrieval framework.'),
('Full-text search indexes documents to allow fast text queries. PostgreSQL supports this through its GIN and GiST indexes.'),
('The PostgreSQL community is active and regularly improves the database system.'),
('Relational databases such as PostgreSQL can handle both structured and unstructured data.'),
('Effective search ranking algorithms, such as BM25, improve search results by understanding relevance.');
```

## Tokenizer configuration

The tokenizer is mainly composed by 2 parts, `text analyzer` and `model`. `text analyzer` is used to parse the text and generate token arrays, which has similar functionality as `tsvector`. While `model` is used to generate token embeddings(token id array, can be casted to `bm25vector`), which is used for similarity search.

### Text Analyzer

`text analyzer` has 3 main components, `character filters`, `pre tokenizer`, `token filters`.

- character filter: It is used to filter out some characters before tokenization. e.g. `to_lowercase`, `unicode_normalization`.
- pre tokenizer: It is used to split the text into tokens. e.g. `unicode segmentation` will split texts according to the [`Unicode Standard Annex #29`](https://unicode.org/reports/tr29/)
- token filter: It is used to filter out some tokens after tokenization. e.g. `stopwords`, `stemmer`.

Details of `text analyzer` configuration can be found in the [text analyzer](05-text-analyzer.md) document.

### Model

`model` has 2 main types, `pre-trained`, `custom`.
- `pre-trained` models have pre-trained vocab lists and some pre-defined tokenization rules. e.g. [`bert_base_uncased`](https://huggingface.co/google-bert/bert-base-uncased), [`wiki_tocken`](https://huggingface.co/datasets/iohadrubin/wikitext-103-raw-v1).
- `custom` models will maintain their own vocab mapping. You can build a custom model based on your own corpus easily.

> Note that some models may have similar processes as `text analyzer`, so you can skip the `text analyzer` configuration for these models.

Details of `model` configuration can be found in the [model](06-model.md) document.

### Config Examples

We utilize the `TOML` format to define the configurations. Here are some examples:

- define a text analyzer and a model first, then create a tokenizer

```sql
-- define a text analyzer
SELECT create_text_analyzer('text_analyzer1', $$
pre_tokenizer = "unicode_segmentation"
$$);

-- define a model, or skip this step if you want to use builtin models
SELECT create_huggingface_model('model1', $$
...
$$);

-- create a tokenizer
SELECT create_tokenizer('tokenizer1', $$
text_analyzer = "text_analyzer1"  # optional, you can skip this if your model can tokenize text directly
model = "model1"
$$);
```

- inline text analyzer

```sql
-- define a model
SELECT create_huggingface_model('model1', $$
...
$$);

-- create a tokenizer
SELECT create_tokenizer('tokenizer1', $$
# model configuration (top-level keys come before any [[table]])
model = "model1"
# inlined text analyzer configuration
pre_tokenizer = "unicode_segmentation"
[[character_filters]]
to_lowercase = {}
[[token_filters]]
stopwords = "nltk_english"
$$);
```

## Native PostgreSQL full-text search

Version 1.0.0 provides two functions returning the native `tsvector` type. `analyze_to_tsvector` uses the text analyzer's string tokens as lexemes, making it useful for Jieba and custom stemmer/filter pipelines. `tokenize_to_tsvector` instead uses the model's token IDs as decimal lexemes, supporting built-in, custom, Hugging Face, and Lindera models. No model decoding or second PostgreSQL text normalization is performed.

```sql
SELECT create_text_analyzer('fts_words', $$
pre_tokenizer = "unicode_segmentation"
[[character_filters]]
to_lowercase = {}
$$);
SELECT analyze_to_tsvector('Cat dog cat', 'fts_words');
-- 'cat':1,3 'dog':2

CREATE TABLE fts_documents (id int PRIMARY KEY, passage text, terms tsvector);
INSERT INTO fts_documents VALUES
    (1, 'Cat dog cat', analyze_to_tsvector('Cat dog cat', 'fts_words'));
CREATE INDEX ON fts_documents USING gin (terms);
SELECT id FROM fts_documents WHERE terms @@ 'cat <-> dog'::tsquery;

SELECT create_tokenizer('fts_bert', 'model = "bert_base_uncased"');
SELECT tokenize_to_tsvector('hello hello', 'fts_bert');
-- '7592':1,2
```

Both functions preserve repeated output tokens and assign positions from the final non-empty output sequence (default weight D). Positions are **not source-text offsets**: filters can remove, expand, or reorder tokens, and model output can contain subwords. Quotes, backslashes, whitespace, and punctuation in lexemes are preserved. Empty tokens are omitted and do not create position gaps; an empty output produces an empty vector, and NULL input returns NULL.

PostgreSQL's native limits apply: tokens longer than 2046 bytes are omitted, without leaving position gaps; each lexeme stores at most 256 positions, and positions above 16,383 are clamped. Long input can therefore lose term-frequency/proximity detail. Repeated tokens are merged into one lexeme entry before PostgreSQL parses the vector, so the 1 MB tsvector limit applies to the distinct lexemes, as with `to_tsvector`; in 1.0.0 a long document with many repeated tokens could fail with `string is too long for tsvector` although the resulting vector was small. See [PostgreSQL text search limits](https://www.postgresql.org/docs/current/textsearch-limitations.html).

Use the same analyzer/model and lexeme representation for documents and queries. `plainto_tsquery('english', ...)` does not generate model token IDs or reproduce a custom Chinese analyzer. These functions do not add a custom text search parser or a `tsquery` builder. They are `STABLE` because definitions live in catalogs: store/update the resulting vector in a column and index that column with GIN/GiST. When an installed BM25 implementation accepts native `tsvector`, it can consume these values too; check that implementation's SQL API before migrating an existing `bm25vector` index.

### Definition cache invalidation

Since 1.1.0, catalog-backed definitions are validated against a one-row version counter committed and replicated with their changes. Each statement reads the counter and definitions under its SQL snapshot. Long-lived primary and standby sessions refresh when their snapshot can see the new version, including after promotion; a REPEATABLE READ transaction keeps its old view until it ends. Wait for WAL replay and begin a new snapshot when coordinating a primary-side definition change with standby queries.

Dependent tokenizers refresh with their analyzers and models. Stored vectors and indexes still need explicit regeneration when output changes. Definition changes serialize on the version row until commit; vocabulary inserts do not. See [cache semantics and resource limits](07-limitation.md) for rollback, permissions, parallel queries and the four configurable budgets.
