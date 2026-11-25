-- Preserve the original six-argument function's OID and dependent callers on upgrade.
CREATE FUNCTION create_custom_model_tokenizer_and_trigger(
    tokenizer_name TEXT, model_name TEXT, text_analyzer_name TEXT,
    table_name TEXT, source_column TEXT, target_column TEXT)
RETURNS VOID AS $body$
BEGIN
    PERFORM tokenizer_catalog.create_custom_model_tokenizer_and_trigger(
        tokenizer_name, model_name, text_analyzer_name, table_name, source_column, target_column,
        NULL::pg_catalog.text);
END;
$body$ LANGUAGE plpgsql;
