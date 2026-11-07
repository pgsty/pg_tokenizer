-- pg_tokenizer 1.0.1: bounded analysis and snapshot-aware definition caches.
-- SQL signatures, vocabulary IDs, and object OIDs are preserved.
LOCK TABLE tokenizer_catalog.model IN SHARE ROW EXCLUSIVE MODE;

ALTER FUNCTION tokenizer_catalog.create_huggingface_model(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_huggingface_model(text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.create_lindera_model(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_lindera_model(text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.create_stopwords(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_stopwords(text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.create_synonym(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_synonym(text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.create_text_analyzer(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_text_analyzer(text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.create_tokenizer(text, text) PARALLEL UNSAFE;
ALTER FUNCTION tokenizer_catalog.drop_tokenizer(text) PARALLEL UNSAFE;

-- Old definitions did not record the creator's search_path. Resolve exactly
-- one dictionary and refuse ambiguity, including a custom dictionary that
-- shadows a pg_catalog builtin: the original search_path cannot be recovered.
DO $dictionaries$
DECLARE
    definition record;
    item record;
    config jsonb;
    identifiers text[];
    candidates text[];
    qualified text;
BEGIN
    FOR definition IN
        SELECT 'text_analyzer' AS relation, a.name, a.config FROM tokenizer_catalog.text_analyzer a
        UNION ALL SELECT 'tokenizer', t.name, t.config FROM tokenizer_catalog.tokenizer t
    LOOP
        config := definition.config::jsonb;
        FOR item IN SELECT value, ordinality FROM pg_catalog.jsonb_array_elements(
            COALESCE(config->'token_filters', '[]'::jsonb)) WITH ORDINALITY
        LOOP
            IF NOT item.value ? 'pg_dict' THEN CONTINUE; END IF;
            identifiers := pg_catalog.parse_ident(item.value->>'pg_dict', true);
            IF pg_catalog.cardinality(identifiers) <> 1 THEN CONTINUE; END IF;
            SELECT pg_catalog.array_agg(pg_catalog.format('%I.%I', n.nspname, d.dictname)
                       ORDER BY n.nspname <> 'pg_catalog', n.nspname)
              INTO candidates FROM pg_catalog.pg_ts_dict d
              JOIN pg_catalog.pg_namespace n ON n.oid = d.dictnamespace
              WHERE d.dictname = identifiers[1];
            IF pg_catalog.cardinality(candidates) = 1 THEN
                qualified := candidates[1];
            ELSE
                RAISE EXCEPTION 'pg_dict name % in % % is missing or ambiguous',
                    identifiers[1], definition.relation, definition.name
                    USING HINT = 'Set a schema-qualified pg_dict name in the definition before upgrading.';
            END IF;
            config := pg_catalog.jsonb_set(config,
                ARRAY['token_filters', (item.ordinality - 1)::text, 'pg_dict'],
                pg_catalog.to_jsonb(qualified));
        END LOOP;
        IF config IS DISTINCT FROM definition.config::jsonb THEN
            EXECUTE pg_catalog.format('UPDATE tokenizer_catalog.%I SET config = $1 WHERE name = $2',
                                      definition.relation) USING config::text, definition.name;
        END IF;
    END LOOP;
END;
$dictionaries$;

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

-- Keep extension definitions separate from caller-owned runtime vocabularies.
-- An unrelated schema of the same name is an error, never silently adopted.
CREATE SCHEMA tokenizer_data;
REVOKE ALL ON SCHEMA tokenizer_data FROM PUBLIC;
DO $runtime$
DECLARE
    acl record;
    model record;
    grantee text;
BEGIN
    -- Carry existing application schema access to the data schema, including
    -- grant options. Explicit CREATE grants move there as well. The extension
    -- installer, as schema owner, retains PostgreSQL's inherent owner rights.
    FOR acl IN
        SELECT a.* FROM pg_catalog.pg_namespace n,
            LATERAL pg_catalog.aclexplode(COALESCE(n.nspacl, pg_catalog.acldefault('n', n.nspowner))) a
        WHERE n.nspname = 'tokenizer_catalog' AND a.grantee <> n.nspowner
    LOOP
        grantee := CASE WHEN acl.grantee = 0 THEN 'PUBLIC'
                   ELSE pg_catalog.quote_ident(pg_catalog.pg_get_userbyid(acl.grantee)) END;
        EXECUTE pg_catalog.format('GRANT %s ON SCHEMA tokenizer_data TO %s%s',
            acl.privilege_type, grantee, CASE WHEN acl.is_grantable THEN ' WITH GRANT OPTION' ELSE '' END);
        IF acl.privilege_type = 'CREATE' THEN
            EXECUTE pg_catalog.format('REVOKE CREATE ON SCHEMA tokenizer_catalog FROM %s CASCADE', grantee);
        END IF;
    END LOOP;
    FOR model IN
        SELECT name FROM tokenizer_catalog.model WHERE config::jsonb ? 'custom' ORDER BY name
    LOOP
        -- SET SCHEMA preserves table/index/sequence identities, owners, ACLs,
        -- token IDs and dependencies, and also moves owned identity sequences.
        EXECUTE pg_catalog.format('ALTER TABLE tokenizer_catalog.%I SET SCHEMA tokenizer_data',
                                   'model_' || model.name);
    END LOOP;
END;
$runtime$;
GRANT USAGE, CREATE ON SCHEMA tokenizer_data TO pg_database_owner;
REVOKE CREATE ON SCHEMA tokenizer_catalog FROM PUBLIC, pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_data
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO pg_database_owner;
ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_data
    GRANT USAGE, SELECT ON SEQUENCES TO pg_database_owner;

CREATE OR REPLACE FUNCTION tokenizer_catalog.custom_model_insert_trigger()
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
        SELECT 1 FROM tokenizer_data.%I AS vocabulary WHERE vocabulary.token = t.token
    )', target_column, text_analyzer, model_table) INTO missing USING NEW;
    IF missing IS NULL THEN
        RETURN NEW;
    END IF;

    -- Sorting one row cannot prevent opposite lock orders across rows/statements.
    -- Serialize vocabulary inserts for this model until commit; only writers
    -- that add tokens take this lock.
    PERFORM pg_catalog.pg_advisory_xact_lock(
        1885828203, pg_catalog.to_regclass(pg_catalog.format('tokenizer_data.%I', model_table))::oid::int);
    EXECUTE pg_catalog.format('
    INSERT INTO tokenizer_data.%I (token)
    SELECT t.token FROM pg_catalog.unnest($1) AS t(token)
    WHERE NOT EXISTS (
        SELECT 1 FROM tokenizer_data.%I AS vocabulary WHERE vocabulary.token = t.token
    )
    ORDER BY t.token COLLATE "C"
    ON CONFLICT (token) DO NOTHING', model_table, model_table) USING missing;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
