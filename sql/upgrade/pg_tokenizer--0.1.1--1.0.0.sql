-- pg_tokenizer 1.0.0: native tsvector output, filtered custom models, database-owner
-- privileges, and hardened vocabulary triggers. Catalog rows, vocabulary IDs, and
-- the original six-argument helper (including its OID) are preserved.

-- 1.0.0 marks apply_text_analyzer STABLE: analyzer definitions are catalog data,
-- never plan-time constants. Refuse to upgrade while objects that require an
-- IMMUTABLE function depend on it; they would fail to restore from a dump.
DO $guard$
DECLARE
    dependents TEXT;
BEGIN
    SELECT pg_catalog.string_agg(pg_catalog.pg_describe_object(d.classid, d.objid, d.objsubid), ', ')
    INTO dependents
    FROM pg_catalog.pg_depend AS d
    LEFT JOIN pg_catalog.pg_attrdef AS ad
        ON d.classid = 'pg_catalog.pg_attrdef'::pg_catalog.regclass AND ad.oid = d.objid
    LEFT JOIN pg_catalog.pg_attribute AS a
        ON a.attrelid = ad.adrelid AND a.attnum = ad.adnum
    WHERE d.refclassid = 'pg_catalog.pg_proc'::pg_catalog.regclass
      AND d.refobjid = 'tokenizer_catalog.apply_text_analyzer(text, text)'::pg_catalog.regprocedure
      AND (d.classid IN ('pg_catalog.pg_class'::pg_catalog.regclass,
                         'pg_catalog.pg_statistic_ext'::pg_catalog.regclass)
           OR a.attgenerated <> '');
    IF dependents IS NOT NULL THEN
        RAISE EXCEPTION 'tokenizer_catalog.apply_text_analyzer is used by %', dependents
            USING HINT = 'pg_tokenizer 1.0.0 marks it STABLE. Replace these indexes, generated columns, '
                         'partition keys, or statistics with stored values, then upgrade.';
    END IF;
END
$guard$;

/* <begin connected objects> */
-- src/tsvector.rs:34
-- pg_tokenizer::tsvector::_analyze_to_tsvector_text
CREATE  FUNCTION "_analyze_to_tsvector_text"(
	"text" TEXT, /* & str */
	"text_analyzer_name" TEXT /* & str */
) RETURNS TEXT /* String */
STRICT STABLE PARALLEL SAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', '_analyze_to_tsvector_text_wrapper';
/* </end connected objects> */


/* <begin connected objects> */
-- src/tsvector.rs:13
-- pg_tokenizer::tsvector::_tokens_to_tsvector_text
CREATE  FUNCTION "_tokens_to_tsvector_text"(
	"tokens" TEXT[] /* Vec < String > */
) RETURNS TEXT /* String */
IMMUTABLE STRICT PARALLEL SAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', '_tokens_to_tsvector_text_wrapper';
/* </end connected objects> */


/* <begin connected objects> */
-- src/model/custom.rs:309
-- pg_tokenizer::model::custom::create_custom_model_tokenizer_and_trigger
CREATE  FUNCTION "create_custom_model_tokenizer_and_trigger"(
	"tokenizer_name" TEXT, /* & str */
	"model_name" TEXT, /* & str */
	"text_analyzer_name" TEXT, /* & str */
	"table_name" TEXT, /* & str */
	"source_column" TEXT, /* & str */
	"target_column" TEXT, /* & str */
	"filter" TEXT /* Option < & str > */
) RETURNS void
VOLATILE PARALLEL UNSAFE
LANGUAGE c /* Rust */
AS 'MODULE_PATHNAME', 'create_custom_model_tokenizer_and_trigger_wrapper';
/* </end connected objects> */

CREATE OR REPLACE FUNCTION custom_model_insert_trigger()
RETURNS TRIGGER AS $$
DECLARE
    model_table TEXT := 'model_' || TG_ARGV[0];
    target_column TEXT := TG_ARGV[1];
    text_analyzer TEXT := TG_ARGV[2];
    matches BOOLEAN;
    missing TEXT[];
BEGIN
    IF TG_NARGS > 3 THEN
        EXECUTE pg_catalog.format('SELECT (%s) IS TRUE FROM (SELECT ($1).*) AS document', TG_ARGV[3])
            INTO matches USING NEW;
        IF NOT matches THEN
            RETURN NEW;
        END IF;
    END IF;

    -- Exact argument types keep overloads added to tokenizer_catalog from
    -- capturing this call. Rows whose tokens are all known need no lock.
    EXECUTE pg_catalog.format('
    SELECT pg_catalog.array_agg(DISTINCT t.token COLLATE "C")
    FROM pg_catalog.unnest(tokenizer_catalog.apply_text_analyzer_for_custom_model(
        $1.%I::pg_catalog.text, %L::pg_catalog.text)) AS t(token)
    WHERE NOT EXISTS (
        SELECT 1 FROM tokenizer_catalog.%I AS vocabulary WHERE vocabulary.token = t.token
    )', target_column, text_analyzer, model_table) INTO missing USING NEW;
    IF missing IS NULL THEN
        RETURN NEW;
    END IF;

    -- Sorting one row cannot prevent opposite lock orders across rows/statements.
    -- Serialize vocabulary inserts for this model until commit; only writers
    -- that add tokens take this lock.
    PERFORM pg_catalog.pg_advisory_xact_lock(
        1885828203, pg_catalog.to_regclass(pg_catalog.format('tokenizer_catalog.%I', model_table))::oid::int);
    EXECUTE pg_catalog.format('
    INSERT INTO tokenizer_catalog.%I (token)
    SELECT t.token FROM pg_catalog.unnest($1) AS t(token)
    WHERE NOT EXISTS (
        SELECT 1 FROM tokenizer_catalog.%I AS vocabulary WHERE vocabulary.token = t.token
    )
    ORDER BY t.token COLLATE "C"
    ON CONFLICT (token) DO NOTHING', model_table, model_table) USING missing;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Preserve the original six-argument function's OID and dependent callers on upgrade.
CREATE OR REPLACE FUNCTION create_custom_model_tokenizer_and_trigger(
    tokenizer_name TEXT, model_name TEXT, text_analyzer_name TEXT,
    table_name TEXT, source_column TEXT, target_column TEXT)
RETURNS VOID AS $body$
BEGIN
    PERFORM tokenizer_catalog.create_custom_model_tokenizer_and_trigger(
        tokenizer_name, model_name, text_analyzer_name, table_name, source_column, target_column,
        NULL::pg_catalog.text);
END;
$body$ LANGUAGE plpgsql;

ALTER FUNCTION tokenizer_catalog.create_custom_model(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_custom_model(text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.apply_text_analyzer(text, text) STABLE;

CREATE FUNCTION analyze_to_tsvector(text TEXT, text_analyzer_name TEXT)
RETURNS pg_catalog.tsvector
LANGUAGE sql STRICT STABLE PARALLEL SAFE
AS $$ SELECT tokenizer_catalog._analyze_to_tsvector_text($1, $2)::pg_catalog.tsvector $$;

CREATE FUNCTION tokenize_to_tsvector(text TEXT, tokenizer_name TEXT)
RETURNS pg_catalog.tsvector
LANGUAGE sql STRICT STABLE PARALLEL SAFE
AS $$ SELECT tokenizer_catalog._tokens_to_tsvector_text(
    tokenizer_catalog.tokenize($1, $2)::text[])::pg_catalog.tsvector $$;

-- The current database owner can manage local tokenizer definitions even when
-- a different administrator installed the extension. No PUBLIC write access.
GRANT USAGE, CREATE ON SCHEMA tokenizer_catalog TO pg_database_owner;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA tokenizer_catalog TO pg_database_owner;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA tokenizer_catalog TO pg_database_owner;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA tokenizer_catalog TO pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_catalog
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_catalog
    GRANT USAGE, SELECT ON SEQUENCES TO pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_catalog
    GRANT EXECUTE ON FUNCTIONS TO pg_database_owner;
-- The preload list is cluster-wide state in the data directory.
REVOKE EXECUTE ON FUNCTION tokenizer_catalog.add_preload_model(text),
    tokenizer_catalog.remove_preload_model(text) FROM PUBLIC, pg_database_owner;

-- Include user definitions in pg_dump. CREATE EXTENSION recreates the shipped
-- stopword lists, so dumping those rows would conflict on restore.
SELECT pg_catalog.pg_extension_config_dump('tokenizer_catalog.text_analyzer', '');
SELECT pg_catalog.pg_extension_config_dump('tokenizer_catalog.tokenizer', '');
SELECT pg_catalog.pg_extension_config_dump('tokenizer_catalog.model', '');
SELECT pg_catalog.pg_extension_config_dump('tokenizer_catalog.synonym', '');
SELECT pg_catalog.pg_extension_config_dump('tokenizer_catalog.stopwords',
    $$WHERE name NOT IN ('lucene_english', 'nltk_english', 'iso_english')$$);

-- 1.0.0 reserves the shipped stopword names. Older versions could drop one and
-- create a different list under its name, which a logical restore would revert.
DO $stopwords$
DECLARE
    replaced TEXT;
BEGIN
    SELECT pg_catalog.string_agg(s.name, ', ' ORDER BY s.name)
    INTO replaced
    FROM tokenizer_catalog.stopwords AS s
    JOIN (VALUES
        ('iso_english', '883fc88a14aa980677c80d485e6c16863fa40ff87743b1f46c6970845ed8f0a5'),
        ('lucene_english', '2f66c0e3dde5d31c7e919e2ed4d9d91390696480be361bfa143ca9ae0cb7ca13'),
        ('nltk_english', '2faef318155a30d7e91c39de3f51fde2eb44a3fe24e0adb0c504fd35c7add888')
    ) AS shipped(name, sha256) ON shipped.name = s.name
    WHERE pg_catalog.encode(pg_catalog.sha256(pg_catalog.convert_to(s.config, 'UTF8')), 'hex')
        <> shipped.sha256;
    IF replaced IS NOT NULL THEN
        RAISE WARNING 'stopwords % differ from the shipped lists', replaced
            USING DETAIL = 'pg_dump skips the shipped stopword names; a logical restore recreates their shipped contents.',
                  HINT = 'Copy the list with create_stopwords() under a new name and use that name in text analyzers.';
    END IF;
END
$stopwords$;
