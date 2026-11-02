# Example

## Using pre-trained model (bert-base-uncased)

```sql
-- create a tokenizer with pre-trained model
SELECT create_tokenizer('tokenizer1', $$
model = "llmlingua2"
$$);

SELECT tokenize('PostgreSQL is a powerful, open-source object-relational database system. It has over 15 years of active development.', 'tokenizer1');
```

Update table:
```sql
CREATE TABLE documents (
    id SERIAL PRIMARY KEY,
    passage TEXT,
    embedding INT[]
);

SELECT create_tokenizer('tokenizer1', $$
model = "llmlingua2"
$$);

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

UPDATE documents SET embedding = tokenize(passage, 'tokenizer1');
```

## Using custom model

```sql
CREATE TABLE documents (
    id SERIAL PRIMARY KEY,
    passage TEXT,
    embedding INT[]
);

-- create a text analyzer to generate tokens that can be used to train the model
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

-- create a model to generate embeddings from original passage
-- It'll train a model from passage column and store the embeddings in the embedding column
SELECT create_custom_model_tokenizer_and_trigger(
    tokenizer_name => 'tokenizer1',
    model_name => 'model1',
    text_analyzer_name => 'text_analyzer1',
    table_name => 'documents',
    source_column => 'passage',
    target_column => 'embedding'
);

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

## Using jieba for Chinese text

```sql
CREATE TABLE documents (
    id SERIAL PRIMARY KEY,
    passage TEXT,
    embedding INT[]
);

-- create a text analyzer which uses jieba pre-tokenizer
SELECT create_text_analyzer('text_analyzer1', $$
[pre_tokenizer.jieba]
$$);

SELECT create_custom_model_tokenizer_and_trigger(
    tokenizer_name => 'tokenizer1',
    model_name => 'model1',
    text_analyzer_name => 'text_analyzer1',
    table_name => 'documents',
    source_column => 'passage',
    target_column => 'embedding'
);

INSERT INTO documents (passage) VALUES 
('红海早过了，船在印度洋面上开驶着，但是太阳依然不饶人地迟落早起，侵占去大部分的夜。'),
('夜仿佛纸浸了油变成半透明体；它给太阳拥抱住了，分不出身来，也许是给太阳陶醉了，所以夕照晚霞褪后的夜色也带着酡红。'),
('到红消醉醒，船舱里的睡人也一身腻汗地醒来，洗了澡赶到甲板上吹海风，又是一天开始。'),
('这是七月下旬，合中国旧历的三伏，一年最热的时候。在中国热得更比常年利害，事后大家都说是兵戈之象，因为这就是民国二十六年【一九三七年】。'),
('这条法国邮船白拉日隆子爵号（VicomtedeBragelonne）正向中国开来。'),
('早晨八点多钟，冲洗过的三等舱甲板湿意未干，但已坐满了人，法国人、德国流亡出来的犹太人、印度人、安南人，不用说还有中国人。'),
('海风里早含着燥热，胖人身体给炎风吹干了，上一层汗结的盐霜，仿佛刚在巴勒斯坦的死海里洗过澡。'),
('毕竟是清晨，人的兴致还没给太阳晒萎，烘懒，说话做事都很起劲。'),
('那几个新派到安南或中国租界当警察的法国人，正围了那年轻善撒娇的犹太女人在调情。'),
('俾斯麦曾说过，法国公使大使的特点，就是一句外国话不会讲；这几位警察并不懂德文，居然传情达意，引得犹太女人格格地笑，比他们的外交官强多了。'),
('这女人的漂亮丈夫，在旁顾而乐之，因为他几天来，香烟、啤酒、柠檬水沾光了不少。'),
('红海已过，不怕热极引火，所以等一会甲板上零星果皮、纸片、瓶塞之外，香烟头定又遍处皆是。'),
('法国人的思想是有名的清楚，他的文章也明白干净，但是他的做事，无不混乱、肮脏、喧哗，但看这船上的乱糟糟。'),
('这船，倚仗人的机巧，载满人的扰攘，寄满人的希望，热闹地行着，每分钟把沾污了人气的一小方小面，还给那无情、无尽、无际的大海。');
```

## Using lindera for Japanese text

```sql
CREATE TABLE documents (
    id SERIAL PRIMARY KEY,
    passage TEXT,
    embedding INT[]
);

-- using lindera config to customize the tokenizer, see https://github.com/lindera/lindera
SELECT create_lindera_model('lindera_ipadic', $$
[segmenter]
mode = "normal"
dictionary = "embedded://ipadic"
[[character_filters]]
kind = "unicode_normalize"
  [character_filters.args]
  kind = "nfkc"
[[character_filters]]
kind = "japanese_iteration_mark"
  [character_filters.args]
  normalize_kanji = true
  normalize_kana = true
[[character_filters]]
kind = "mapping"
[character_filters.args.mapping]
"リンデラ" = "Lindera"
[[token_filters]]
kind = "japanese_compound_word"
  [token_filters.args]
  kind = "ipadic"
  tags = [ "名詞,数", "名詞,接尾,助数詞" ]
  new_tag = "名詞,数"
[[token_filters]]
kind = "japanese_number"
  [token_filters.args]
  tags = [ "名詞,数" ]
[[token_filters]]
kind = "japanese_stop_tags"
  [token_filters.args]
  tags = [
  "接続詞",
  "助詞",
  "助詞,格助詞",
  "助詞,格助詞,一般",
  "助詞,格助詞,引用",
  "助詞,格助詞,連語",
  "助詞,係助詞",
  "助詞,副助詞",
  "助詞,間投助詞",
  "助詞,並立助詞",
  "助詞,終助詞",
  "助詞,副助詞／並立助詞／終助詞",
  "助詞,連体化",
  "助詞,副詞化",
  "助詞,特殊",
  "助動詞",
  "記号",
  "記号,一般",
  "記号,読点",
  "記号,句点",
  "記号,空白",
  "記号,括弧閉",
  "その他,間投",
  "フィラー",
  "非言語音"
]
[[token_filters]]
kind = "japanese_katakana_stem"
  [token_filters.args]
  min = 3
[[token_filters]]
kind = "remove_diacritical_mark"
  [token_filters.args]
  japanese = false
$$);

SELECT tokenizer_catalog.create_tokenizer('lindera_ipadic', $$
model = "lindera_ipadic"
$$);

INSERT INTO documents (passage) VALUES 
('どこで生れたかとんと見当けんとうがつかぬ。'),
('何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。'),
('吾輩はここで始めて人間というものを見た。'),
('しかもあとで聞くとそれは書生という人間中で一番獰悪どうあくな種族であったそうだ。'),
('この書生というのは時々我々を捕つかまえて煮にて食うという話である。'),
('しかしその当時は何という考もなかったから別段恐しいとも思わなかった。'),
('ただ彼の掌てのひらに載せられてスーと持ち上げられた時何だかフワフワした感じがあったばかりである。'),
('掌の上で少し落ちついて書生の顔を見たのがいわゆる人間というものの見始みはじめであろう。'),
('この時妙なものだと思った感じが今でも残っている。'),
('第一毛をもって装飾されべきはずの顔がつるつるしてまるで薬缶やかんだ。'),
('その後ご猫にもだいぶ逢あったがこんな片輪かたわには一度も出会でくわした事がない。'),
('のみならず顔の真中があまりに突起している。'),
('そうしてその穴の中から時々ぷうぷうと煙けむりを吹く。'),
('どうも咽むせぽくて実に弱った。'),
('これが人間の飲む煙草たばこというものである事はようやくこの頃知った。');

UPDATE documents SET embedding = tokenizer_catalog.tokenize(passage, 'lindera_ipadic');
```
