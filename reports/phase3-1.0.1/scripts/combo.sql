\set ON_ERROR_STOP 1
SET search_path = public, tokenizer_catalog;
SET enable_seqscan = off;
SET max_parallel_workers_per_gather = 0;
SET statement_timeout = '60s';
SET vchordrq.probes = '4';

CREATE EXTENSION IF NOT EXISTS vchord CASCADE;
CREATE EXTENSION IF NOT EXISTS vchord_bm25;
CREATE EXTENSION IF NOT EXISTS pg_tokenizer;
DO $$ BEGIN
    IF (SELECT extversion FROM pg_extension WHERE extname = 'pg_tokenizer') <> '1.0.1'
       OR (SELECT extversion FROM pg_extension WHERE extname = 'vchord') <> '1.2.0'
       OR (SELECT extversion FROM pg_extension WHERE extname = 'vchord_bm25') <> '1.0.0'
    THEN RAISE EXCEPTION 'wrong staged SQL versions'; END IF;
END $$;

SELECT create_text_analyzer('combo_words', $$
pre_tokenizer = "unicode_segmentation"
character_filters = ["to_lowercase"]
$$);
SELECT create_tokenizer('combo_bert', 'model = "bert_base_uncased"');
CREATE TABLE combo_docs (id int PRIMARY KEY, body text, terms tsvector, ids tsvector);
INSERT INTO combo_docs
SELECT i, body, analyze_to_tsvector(body, 'combo_words'), tokenize_to_tsvector(body, 'combo_bert')
FROM (SELECT i, 'Postgres search ranking document ' || i AS body FROM generate_series(1, 2000) i) s;
CREATE INDEX combo_terms ON combo_docs USING bm25 (terms bm25_ops);
CREATE INDEX combo_ids ON combo_docs USING bm25 (ids bm25_ops);
ANALYZE combo_docs;
EXPLAIN (COSTS OFF) SELECT id FROM combo_docs ORDER BY terms <&>
    to_bm25query(analyze_to_tsvector('postgres ranking', 'combo_words'), 'combo_terms') LIMIT 10;
EXPLAIN (COSTS OFF) SELECT id FROM combo_docs ORDER BY ids <&>
    to_bm25query(tokenize_to_tsvector('postgres ranking', 'combo_bert'), 'combo_ids') LIMIT 10;
DO $$ DECLARE n int; BEGIN
    SELECT count(*) INTO n FROM (SELECT id FROM combo_docs ORDER BY terms <&>
        to_bm25query(analyze_to_tsvector('postgres ranking', 'combo_words'), 'combo_terms') LIMIT 10) s;
    IF n <> 10 THEN RAISE EXCEPTION 'analyzer BM25 returned % rows', n; END IF;
    SELECT count(*) INTO n FROM (SELECT id FROM combo_docs ORDER BY ids <&>
        to_bm25query(tokenize_to_tsvector('postgres ranking', 'combo_bert'), 'combo_ids') LIMIT 10) s;
    IF n <> 10 THEN RAISE EXCEPTION 'token ID BM25 returned % rows', n; END IF;
END $$;
DELETE FROM combo_docs WHERE id % 3 = 0;
VACUUM combo_docs;
SELECT bm25_maintain('combo_terms'), bm25_maintain('combo_ids');

CREATE TABLE combo_vectors AS
SELECT i AS id, ARRAY[i::real / 4000, (i % 7)::real / 7, (i % 11)::real / 11, 0.5]::vector(4) AS e
FROM generate_series(1, 4000) i;
CREATE INDEX combo_rq ON combo_vectors USING vchordrq (e vector_l2_ops)
WITH (options = $$[build.internal]
lists = [16]
spherical_centroids = false
$$);
ANALYZE combo_vectors;
EXPLAIN (COSTS OFF) SELECT id FROM combo_vectors ORDER BY e <-> '[0.5,0.5,0.5,0.5]' LIMIT 10;
DO $$ DECLARE n int; BEGIN
    SELECT count(*) INTO n FROM (SELECT id FROM combo_vectors ORDER BY e <-> '[0.5,0.5,0.5,0.5]' LIMIT 10) s;
    IF n <> 10 THEN RAISE EXCEPTION 'rq returned % rows', n; END IF;
END $$;
DROP INDEX combo_rq;
CREATE INDEX combo_g ON combo_vectors USING vchordg (e vector_l2_ops);
EXPLAIN (COSTS OFF) SELECT id FROM combo_vectors ORDER BY e <-> '[0.5,0.5,0.5,0.5]' LIMIT 10;
DO $$ DECLARE n int; BEGIN
    SELECT count(*) INTO n FROM (SELECT id FROM combo_vectors ORDER BY e <-> '[0.5,0.5,0.5,0.5]' LIMIT 10) s;
    IF n <> 10 THEN RAISE EXCEPTION 'g returned % rows', n; END IF;
END $$;
DROP TABLE combo_vectors, combo_docs;
SELECT drop_tokenizer('combo_bert');
SELECT drop_text_analyzer('combo_words');
SELECT 'combo passed' AS result;
