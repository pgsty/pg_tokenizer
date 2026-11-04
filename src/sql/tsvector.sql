CREATE FUNCTION analyze_to_tsvector(text TEXT, text_analyzer_name TEXT)
RETURNS pg_catalog.tsvector
LANGUAGE sql STRICT STABLE PARALLEL SAFE
AS $$ SELECT tokenizer_catalog._analyze_to_tsvector_text($1, $2)::pg_catalog.tsvector $$;

CREATE FUNCTION tokenize_to_tsvector(text TEXT, tokenizer_name TEXT)
RETURNS pg_catalog.tsvector
LANGUAGE sql STRICT STABLE PARALLEL SAFE
AS $$ SELECT tokenizer_catalog._tokens_to_tsvector_text(
    tokenizer_catalog.tokenize($1, $2)::text[])::pg_catalog.tsvector $$;
