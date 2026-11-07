CREATE FUNCTION custom_model_insert_trigger()
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
