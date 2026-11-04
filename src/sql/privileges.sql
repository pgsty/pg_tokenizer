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
