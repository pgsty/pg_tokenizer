-- Upgrade pg_tokenizer from 1.2.0 to 1.3.0.
-- Replaces SQL tsvector wrappers and PL/pgSQL custom-model vocabulary trigger
-- with native C implementations, and links definition-change triggers to the
-- primary cache invalidation epoch and XID barrier.

CREATE FUNCTION tokenizer_catalog._notify_catalog_changed()
RETURNS void
STRICT VOLATILE PARALLEL UNSAFE
LANGUAGE c
AS 'MODULE_PATHNAME', '_notify_catalog_changed_wrapper';

CREATE FUNCTION tokenizer_catalog._notify_definition_version_changed()
RETURNS TRIGGER
LANGUAGE c
AS 'MODULE_PATHNAME', '_notify_definition_version_changed_wrapper';

REVOKE EXECUTE ON FUNCTION tokenizer_catalog._notify_catalog_changed(),
    tokenizer_catalog._notify_definition_version_changed() FROM PUBLIC, pg_database_owner;

CREATE OR REPLACE FUNCTION tokenizer_catalog._bump_definition_version()
RETURNS trigger LANGUAGE plpgsql VOLATILE PARALLEL UNSAFE
SECURITY DEFINER SET search_path = pg_catalog AS $body$
BEGIN
    UPDATE tokenizer_catalog.definition_version SET version = version + 1 WHERE singleton;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'pg_tokenizer definition version is missing';
    END IF;
    PERFORM tokenizer_catalog._notify_catalog_changed();
    RETURN NULL;
END;
$body$;

-- Row validators can load either image before the version update runs.
CREATE TRIGGER definition_changing BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.text_analyzer FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._notify_definition_version_changed();
CREATE TRIGGER definition_changing BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.tokenizer FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._notify_definition_version_changed();
CREATE TRIGGER definition_changing BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.model FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._notify_definition_version_changed();
CREATE TRIGGER definition_changing BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.stopwords FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._notify_definition_version_changed();
CREATE TRIGGER definition_changing BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON tokenizer_catalog.synonym FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._notify_definition_version_changed();
CREATE TRIGGER definition_version_changed AFTER DELETE OR TRUNCATE
    ON tokenizer_catalog.definition_version FOR EACH STATEMENT
    EXECUTE FUNCTION tokenizer_catalog._notify_definition_version_changed();

CREATE OR REPLACE FUNCTION tokenizer_catalog.custom_model_insert_trigger()
RETURNS TRIGGER
LANGUAGE c
AS 'MODULE_PATHNAME', 'custom_model_insert_trigger_wrapper';

CREATE OR REPLACE FUNCTION tokenizer_catalog.analyze_to_tsvector(
    "text" TEXT,
    "text_analyzer_name" TEXT
) RETURNS pg_catalog.tsvector
STRICT STABLE PARALLEL SAFE
LANGUAGE c
AS 'MODULE_PATHNAME', 'analyze_to_tsvector_wrapper';

CREATE OR REPLACE FUNCTION tokenizer_catalog.tokenize_to_tsvector(
    "text" TEXT,
    "tokenizer_name" TEXT
) RETURNS pg_catalog.tsvector
STRICT STABLE PARALLEL SAFE
LANGUAGE c
AS 'MODULE_PATHNAME', 'tokenize_to_tsvector_wrapper';

SELECT tokenizer_catalog._notify_catalog_changed();
