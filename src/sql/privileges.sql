-- The current database owner can manage local tokenizer definitions even when
-- a different administrator installed the extension. No PUBLIC write access.
CREATE SCHEMA tokenizer_data;
REVOKE CREATE ON SCHEMA tokenizer_catalog FROM PUBLIC, pg_database_owner;
GRANT USAGE ON SCHEMA tokenizer_catalog TO pg_database_owner;
REVOKE ALL ON SCHEMA tokenizer_data FROM PUBLIC;
GRANT USAGE, CREATE ON SCHEMA tokenizer_data TO pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_data
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_data
    GRANT USAGE, SELECT ON SEQUENCES TO pg_database_owner;
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

-- One transactional signal for definition tables, replicated with their WAL.
-- Vocabulary inserts do not change definitions and do not update this row.
CREATE TABLE tokenizer_catalog.definition_version (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    version bigint NOT NULL
);
INSERT INTO tokenizer_catalog.definition_version VALUES (true, 0);

CREATE FUNCTION tokenizer_catalog._bump_definition_version()
RETURNS trigger LANGUAGE plpgsql VOLATILE PARALLEL UNSAFE
SECURITY DEFINER SET search_path = pg_catalog AS $body$
BEGIN
    UPDATE tokenizer_catalog.definition_version SET version = version + 1 WHERE singleton;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'pg_tokenizer definition version is missing';
    END IF;
    RETURN NULL;
END;
$body$;

CREATE TRIGGER definition_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.text_analyzer FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._bump_definition_version();
CREATE TRIGGER definition_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.tokenizer FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._bump_definition_version();
CREATE TRIGGER definition_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.model FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._bump_definition_version();
CREATE TRIGGER definition_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.stopwords FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._bump_definition_version();
CREATE TRIGGER definition_changed AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.synonym FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._bump_definition_version();

-- Readers need only this harmless counter, in addition to their existing ACLs.
-- Exclude it from pg_dump config data: restored definitions bump a fresh row.
REVOKE ALL ON tokenizer_catalog.definition_version FROM PUBLIC, pg_database_owner;
GRANT SELECT ON tokenizer_catalog.definition_version TO PUBLIC;
REVOKE EXECUTE ON FUNCTION tokenizer_catalog._bump_definition_version() FROM PUBLIC, pg_database_owner;
