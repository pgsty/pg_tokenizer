-- pg_tokenizer 1.0.1: bounded analysis and snapshot-aware definition caches.
-- SQL signatures, vocabulary IDs, and stored data are preserved.

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

-- Old definitions did not record the creator's search_path. Preserve standard
-- builtin names, resolve a unique custom dictionary, and refuse ambiguity.
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
            IF pg_catalog.cardinality(candidates) = 1 OR candidates[1] LIKE 'pg_catalog.%' THEN
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
