// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Arc;

use pgrx::pg_sys::errcodes::PgSqlErrorCode;
use pgrx::pg_sys::panic::ErrorReportable;
use pgrx::pg_sys::{self};
use pgrx::prelude::PgHeapTuple;
use pgrx::spi::{quote_identifier, quote_literal};
use pgrx::{IntoDatum, WhoAllocated};
use serde::{Deserialize, Serialize};

use crate::text_analyzer::get_text_analyzer;
use crate::utils::{invalid_config, raise_error, spi_get_one};

use super::{validate_new_model_name, ModelConfig, TokenizerModel, MODEL_OBJECT_POOL};

#[derive(Debug)]
pub struct CustomModel {
    name: String,
}

impl CustomModel {
    pub fn new(name: &str, _config: &CustomModelConfig) -> Self {
        CustomModel {
            name: name.to_string(),
        }
    }
}

// The new library also supports 1.0.0 SQL until the administrator upgrades it.
// Never adopt a schema that is not owned by this extension.
fn vocabulary_table(name: &str) -> String {
    let schema = unsafe {
        let namespace = pg_sys::get_namespace_oid(c"tokenizer_data".as_ptr(), true);
        let extension = pg_sys::get_extension_oid(c"pg_tokenizer".as_ptr(), false);
        if namespace != pg_sys::InvalidOid
            && pg_sys::getExtensionOfObject(pg_sys::NamespaceRelationId, namespace) == extension
        {
            "tokenizer_data"
        } else {
            "tokenizer_catalog"
        }
    };
    format!("{schema}.{}", quote_identifier(format!("model_{name}")))
}

// Take this lock before choosing the vocabulary schema. Upgrade takes a
// conflicting lock before moving tables, so a concurrent creator cannot commit
// a new vocabulary in the old schema after migration has scanned the catalog.
fn lock_model_catalog() {
    pgrx::Spi::run("LOCK TABLE tokenizer_catalog.model IN ROW EXCLUSIVE MODE").unwrap();
}

impl TokenizerModel for CustomModel {
    fn apply(&self, text: String) -> Vec<u32> {
        let query = format!(
            "SELECT id FROM {} WHERE token = $1",
            vocabulary_table(&self.name)
        );

        // The vocabulary stores long tokens as pieces; look up the same pieces.
        split_long_token(text)
            .into_iter()
            .filter_map(|piece| spi_get_one::<i32>(&query, &[piece.into()]))
            .map(|id| u32::try_from(id).unwrap())
            .collect()
    }

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<u32> {
        let tokens = split_long_tokens(tokens);
        let query = format!(
            "SELECT id, token FROM {} WHERE token = ANY($1)",
            vocabulary_table(&self.name)
        );

        let mut token_map = HashMap::new();
        pgrx::Spi::connect(|client| {
            let tuptable = client
                .select(&query, None, &[tokens.clone().into()])
                .unwrap_or_report();
            for (i, tup) in tuptable.enumerate() {
                crate::utils::check_for_interrupts_every(i);
                let id: i32 = tup.get(1).unwrap_or_report().expect("no id value");
                let id = u32::try_from(id).expect("id is not a valid u32");
                let token: String = tup.get(2).unwrap_or_report().expect("no token value");
                token_map.insert(token, id);
            }
        });

        tokens
            .into_iter()
            .enumerate()
            .filter_map(|(i, token)| {
                crate::utils::check_for_interrupts_every(i);
                token_map.get(&token).copied()
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomModelConfig {
    table: String,
    column: String,
    text_analyzer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filter: Option<String>,
}

// Store a qualified name so triggers and drops do not depend on a later search_path.
fn resolve_table(table: &str) -> String {
    try_resolve_table(table).unwrap_or_else(|| panic!("Table not found: {table}"))
}

fn try_resolve_table(table: &str) -> Option<String> {
    use pgrx::{pg_sys, IntoDatum, PgTryBuilder};

    // Parse identifiers without executing SQL. Only suppress syntax errors from
    // this pure parser, to retain legacy literal names such as O'Reilly docs.
    let identifiers = PgTryBuilder::new(|| unsafe {
        pgrx::direct_function_call::<Vec<String>>(
            pg_sys::parse_ident,
            &[table.into_datum(), true.into_datum()],
        )
    })
    .catch_when(
        pg_sys::errcodes::PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
        |_| None,
    )
    .execute();
    let lookup = match identifiers {
        None => "pg_catalog.to_regclass(pg_catalog.quote_ident($1))",
        Some(parts) if parts.len() > 1 || table.trim_start().starts_with('"') => {
            "coalesce(pg_catalog.to_regclass($1), pg_catalog.to_regclass(pg_catalog.quote_ident($1)))"
        }
        Some(_) => {
            "coalesce(pg_catalog.to_regclass(pg_catalog.quote_ident($1)), pg_catalog.to_regclass($1))"
        }
    };
    spi_get_one(
        &format!(
            "SELECT pg_catalog.format('%I.%I', n.nspname, c.relname) FROM pg_catalog.pg_class c \
         JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.oid = {lookup}"
        ),
        &[table.into()],
    )
}

fn update_event(config: &CustomModelConfig) -> String {
    if config.filter.is_some() {
        // A predicate column can change without the source text changing.
        "UPDATE".to_string()
    } else {
        format!("UPDATE OF {}", quote_identifier(&config.column))
    }
}

// 1. validate the model name and config
// 2. validate if text analyzer exists
// 3. create word table && create trigger && insert existing tokens
// 4. insert model into model table
// 5. insert model into model object pool
// do 3 4 5 in a transaction, if any step fails, rollback
#[pgrx::pg_extern(volatile, parallel_unsafe)]
fn create_custom_model(name: &str, config: &str) {
    validate_new_model_name(name).unwrap();
    lock_model_catalog();
    let vocabulary = vocabulary_table(name);

    let mut config: CustomModelConfig =
        toml::from_str(config).unwrap_or_else(|e| invalid_config("custom model", name, e));
    config.table = resolve_table(&config.table);
    let table = &config.table;
    let column = quote_identifier(&config.column);

    get_text_analyzer(&config.text_analyzer);

    let create_word_table = format!(
        r#"
        CREATE TABLE {vocabulary} (
            id int GENERATED BY DEFAULT AS IDENTITY,
            token TEXT NOT NULL UNIQUE
        );
        "#,
    );
    let filter = config.filter.as_deref().unwrap_or("true");
    let filter_arg = config
        .filter
        .as_deref()
        .map(|filter| format!(", {}", quote_literal(filter)))
        .unwrap_or_default();
    let analyzer = quote_literal(&config.text_analyzer);
    let create_trigger = format!(
        r#"
        CREATE TRIGGER "model_{name}_trigger"
        BEFORE INSERT OR {event} ON {table}
        FOR EACH ROW
        EXECUTE FUNCTION tokenizer_catalog.custom_model_insert_trigger({model}, {column_arg}, {analyzer}{filter_arg});
        "#,
        event = update_event(&config),
        model = quote_literal(name),
        column_arg = quote_literal(&config.column),
    );
    let insert_existing_tokens = format!(
        r#"
        WITH tokens AS (
            SELECT pg_catalog.unnest(tokenizer_catalog.apply_text_analyzer_for_custom_model(
                ({column})::pg_catalog.text, {analyzer}::pg_catalog.text)) AS token
            FROM {table} AS document WHERE ({filter})
        )
        INSERT INTO {vocabulary} (token)
        SELECT DISTINCT token COLLATE "C" FROM tokens
        ORDER BY token COLLATE "C"
        ON CONFLICT (token) DO NOTHING;
        "#,
    );
    let grant_model = format!(
        r#"
        GRANT SELECT, INSERT, UPDATE, DELETE ON {vocabulary} TO pg_database_owner;
        "#,
    );
    let insert_model = r#"
        INSERT INTO tokenizer_catalog.model (name, config) VALUES ($1, $2)
        ON CONFLICT (name) DO NOTHING RETURNING 1
        "#;
    let custom_model = CustomModel::new(name, &config);
    let config_str = serde_json::to_string(&ModelConfig::Custom(config.clone())).unwrap();

    pgrx::Spi::connect_mut(|client| {
        // Use the trigger's composite-row scope even when the corpus is empty.
        // System columns/table-qualified names cannot work against NEW later.
        client
            .select(
                &format!(
                    "SELECT ({filter}) IS TRUE FROM (SELECT (NULL::{table}).*) AS document LIMIT 0"
                ),
                None,
                &[],
            )
            .unwrap();
        client.update(&create_word_table, None, &[]).unwrap();
        client.update(&grant_model, None, &[]).unwrap();
        // PostgreSQL may suffix the identity sequence name after a collision.
        let sequence: String = client
            .select(
                "SELECT pg_catalog.pg_get_serial_sequence($1, 'id')",
                None,
                &[vocabulary.as_str().into()],
            )
            .unwrap()
            .first()
            .get_one()
            .unwrap()
            .expect("model identity sequence is missing");
        client
            .update(
                &format!("GRANT USAGE, SELECT ON SEQUENCE {sequence} TO pg_database_owner"),
                None,
                &[],
            )
            .unwrap();
        client.update(&create_trigger, None, &[]).unwrap();
        client.update(&insert_existing_tokens, None, &[]).unwrap();

        let tuptable = client
            .update(insert_model, None, &[name.into(), config_str.into()])
            .unwrap();
        if tuptable.is_empty() {
            panic!("Model already exists: {}", name);
        }

        crate::cache::mark_catalog_changed();
        MODEL_OBJECT_POOL.insert(name.to_string(), Arc::new(custom_model));
    });
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
fn drop_custom_model(name: &str) {
    if let Err(e) = validate_new_model_name(name) {
        pgrx::warning!("Invalid model name: {}, Details: {}", name, e);
        return;
    }

    lock_model_catalog();
    let select_config = r#"SELECT config FROM tokenizer_catalog.model WHERE name = $1"#;
    // Like the other drop functions, a missing model is a WARNING. Tokenizers
    // that reference the name are not dropped.
    let Some(config_bytes) = spi_get_one::<&str>(select_config, &[name.into()]) else {
        pgrx::warning!("Model not found: {}", name);
        return;
    };
    let config: ModelConfig =
        serde_json::from_str(config_bytes).unwrap_or_else(|e| invalid_config("model", name, e));
    let ModelConfig::Custom(config) = &config else {
        panic!("Model is not a custom model: {}", name);
    };
    // A dropped source table has already taken the triggers with it.
    let table_name = try_resolve_table(&config.table);
    let drop_triggers = table_name.map(|table| {
        format!(
            r#"DROP TRIGGER IF EXISTS "model_{name}_trigger" ON {table};
            DROP TRIGGER IF EXISTS "model_{name}_trigger_insert" ON {table}"#
        )
    });
    let drop_table = format!("DROP TABLE IF EXISTS {}", vocabulary_table(name));
    let delete_model = r#"DELETE FROM tokenizer_catalog.model WHERE name = $1 RETURNING 1"#;

    pgrx::Spi::connect_mut(|client| {
        if let Some(drop_triggers) = &drop_triggers {
            client.update(drop_triggers, None, &[]).unwrap();
        }
        client.update(&drop_table, None, &[]).unwrap();
        let tuptable = client.update(delete_model, None, &[name.into()]).unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("Model not found: {}", name);
        }
    });

    crate::cache::mark_catalog_changed();
    MODEL_OBJECT_POOL.remove(name);
}

// Vocabulary tokens are keys of a btree index, whose entries must fit in a
// third of a page. Longer tokens are stored and looked up as pieces.
const MAX_TOKEN_LENGTH: usize = 2600;

// Split a token into pieces of at most MAX_TOKEN_LENGTH bytes, cut at UTF-8
// character boundaries, in order. Shorter tokens are returned unchanged.
fn split_long_token(token: String) -> Vec<String> {
    if token.len() <= MAX_TOKEN_LENGTH {
        return vec![token];
    }
    let mut budget = crate::budget::Output::default();
    let capacity = token.len().div_ceil(MAX_TOKEN_LENGTH - 3);
    budget.workspace(capacity.saturating_mul(std::mem::size_of::<String>()));
    let mut pieces = Vec::with_capacity(capacity);
    let mut rest = token.as_str();
    while rest.len() > MAX_TOKEN_LENGTH {
        // A character is at most 4 bytes, so a boundary exists in the last 4.
        let end = (MAX_TOKEN_LENGTH - 3..=MAX_TOKEN_LENGTH)
            .rev()
            .find(|&end| rest.is_char_boundary(end))
            .expect("a UTF-8 character is at most 4 bytes");
        let (piece, tail) = rest.split_at(end);
        budget.text(piece.len());
        crate::utils::check_for_interrupts_every(pieces.len());
        pieces.push(piece.to_string());
        rest = tail;
    }
    budget.text(rest.len());
    pieces.push(rest.to_string());
    pieces
}

// Split every long token in place, keeping the order of the token sequence.
fn split_long_tokens(tokens: Vec<String>) -> Vec<String> {
    if tokens.iter().all(|token| token.len() <= MAX_TOKEN_LENGTH) {
        return tokens;
    }
    let mut budget = crate::budget::Output::default();
    let mut pieces = Vec::new();
    for token in tokens {
        for piece in split_long_token(token) {
            budget.text(piece.len());
            pieces.push(piece);
        }
    }
    pieces
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn apply_text_analyzer_for_custom_model(text: &str, text_analyzer_name: &str) -> Vec<String> {
    let text_analyzer = get_text_analyzer(text_analyzer_name);
    let tokens = text_analyzer.apply(text);
    if tokens.iter().any(|token| token.len() > MAX_TOKEN_LENGTH) {
        pgrx::warning!(
            "custom model tokens longer than {MAX_TOKEN_LENGTH} bytes are split into pieces of at most {MAX_TOKEN_LENGTH} bytes"
        );
    }
    split_long_tokens(tokens)
}

pgrx::extension_sql_file!("../sql/custom_model.sql", name = "custom_model_triggers");

#[pgrx::pg_extern(volatile, parallel_unsafe)]
fn create_custom_model_tokenizer_and_trigger(
    tokenizer_name: &str,
    model_name: &str,
    text_analyzer_name: &str,
    table_name: &str,
    source_column: &str,
    target_column: &str,
    filter: Option<&str>,
) {
    let config = CustomModelConfig {
        table: resolve_table(table_name),
        column: source_column.to_string(),
        text_analyzer: text_analyzer_name.to_string(),
        filter: filter.map(str::to_string),
    };
    create_custom_model(model_name, &toml::to_string(&config).unwrap());
    let tokenizer_config = toml::to_string(&std::collections::BTreeMap::from([
        ("text_analyzer", text_analyzer_name),
        ("model", model_name),
    ]))
    .unwrap();
    let predicate = filter.unwrap_or("true");
    pgrx::Spi::connect_mut(|client| {
        client
            .update(
                "SELECT tokenizer_catalog.create_tokenizer($1, $2)",
                None,
                &[tokenizer_name.into(), tokenizer_config.into()],
            )
            .unwrap();
        client
            .update(
                &format!(
                    "UPDATE {} AS document SET {} = tokenizer_catalog.tokenize(({})::pg_catalog.text, {}::pg_catalog.text) WHERE ({predicate})",
                    config.table,
                    quote_identifier(target_column),
                    quote_identifier(source_column),
                    quote_literal(tokenizer_name)
                ),
                None,
                &[],
            )
            .unwrap();
        client.update(
            &format!(r#"CREATE TRIGGER "model_{model_name}_trigger_insert"
                BEFORE INSERT OR {} ON {} FOR EACH ROW
                EXECUTE FUNCTION tokenizer_catalog.custom_model_tokenizer_set_target_column_trigger({}, {}, {}{})"#,
                update_event(&config), config.table, quote_literal(tokenizer_name), quote_literal(source_column),
                quote_literal(target_column),
                filter.map(|filter| format!(", {}", quote_literal(filter))).unwrap_or_default()),
            None, &[],
        ).unwrap();
    });
}

#[pgrx::pg_trigger]
fn custom_model_tokenizer_set_target_column_trigger<'a>(
    trigger: &'a pgrx::PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, impl WhoAllocated>>, ()> {
    let mut new = trigger.new().expect("new tuple is missing").into_owned();
    let tg_argv = trigger.extra_args().expect("trigger arguments are missing");
    if !(3..=4).contains(&tg_argv.len()) {
        panic!("Invalid trigger arguments");
    }
    let tokenizer_name = &tg_argv[0];
    let source_column = &tg_argv[1];
    let target_column = &tg_argv[2];
    let row = || {
        let row = trigger.new().expect("new tuple is missing");
        let oid = row.composite_type_oid().expect("row type is missing");
        // SAFETY: the OID comes from this exact composite tuple's descriptor.
        unsafe { pgrx::datum::DatumWithOid::new(row, oid) }
    };

    if let Some(filter) = tg_argv.get(3) {
        let matches = spi_get_one::<bool>(
            &format!("SELECT ({filter}) IS TRUE FROM (SELECT ($1).*) AS document"),
            &[row()],
        )
        .unwrap();
        if !matches {
            // Leave nonmatching rows untouched, including any existing embedding.
            return Ok(Some(new));
        }
    }

    // A preceding vocabulary trigger may have waited for another transaction.
    // Use a read-write SPI call so READ COMMITTED gets a fresh snapshot that can
    // see that writer's committed IDs as well as our own newly inserted tokens.
    // Read the source like model creation and the vocabulary trigger do, with
    // a ::text cast, so non-text source columns such as char(n) work too. A
    // NULL source gives a NULL result (tokenize is STRICT).
    let query = format!(
        "SELECT tokenizer_catalog.tokenize((($1).{})::pg_catalog.text, $2)",
        quote_identifier(source_column)
    );
    let target: Option<Vec<i32>> = pgrx::Spi::connect_mut(|client| {
        client
            .update(&query, Some(1), &[row(), tokenizer_name.as_str().into()])
            .unwrap_or_report()
            .first()
            .get_one()
            .unwrap_or_report()
    });
    let (idx, att) = new
        .get_attribute_by_name(target_column)
        .expect("target column is missing");
    let value = if att.atttypid == pg_sys::INT4ARRAYOID {
        // Assignment coercion of int4[] to int4[] is the identity.
        target.and_then(IntoDatum::into_datum)
    } else {
        assign_target(target, att.atttypid, att.atttypmod)
    };
    // SAFETY: the value has the type of the target column, as coerced above.
    unsafe { new.set_by_index_unchecked(idx, value) };
    Ok(Some(new))
}

// Convert the tokenizer output to the target column the way INSERT and UPDATE
// assign values: PostgreSQL assignment coercion to the column's type and
// typmod, so domain NOT NULL/CHECK constraints (also for NULL) and length
// limits apply. PostgreSQL does not repeat this for a BEFORE trigger's tuple.
fn assign_target(
    target: Option<Vec<i32>>,
    target_type: pg_sys::Oid,
    target_typmod: i32,
) -> Option<pg_sys::Datum> {
    unsafe {
        let isnull = target.is_none();
        let datum = target
            .and_then(IntoDatum::into_datum)
            .unwrap_or(pg_sys::Datum::from(0));
        let source = pg_sys::makeConst(
            pg_sys::INT4ARRAYOID,
            -1,
            pg_sys::InvalidOid,
            -1,
            datum,
            isnull,
            false,
        );
        let expr = pg_sys::coerce_to_target_type(
            std::ptr::null_mut(),
            source.cast(),
            pg_sys::INT4ARRAYOID,
            target_type,
            target_typmod,
            pg_sys::CoercionContext::COERCION_ASSIGNMENT,
            pg_sys::CoercionForm::COERCE_IMPLICIT_CAST,
            -1,
        );
        if expr.is_null() {
            raise_error(
                PgSqlErrorCode::ERRCODE_DATATYPE_MISMATCH,
                format!(
                    "tokenizer output of type integer[] cannot be assigned to a target column of type {}",
                    format_type(target_type, target_typmod)
                ),
                None,
                Some("Use an integer[] target column or a type with an assignment cast from integer[]."),
            );
        }
        pg_sys::assign_expr_collations(std::ptr::null_mut(), expr);
        let estate = ExecutorState::new();
        let state = pg_sys::ExecPrepareExpr(expr.cast(), estate.0);
        let econtext = pg_sys::MakePerTupleExprContext(estate.0);
        let mut isnull = false;
        let result = pg_sys::ExecEvalExprSwitchContext(state, econtext, &mut isnull);
        if isnull {
            return None;
        }
        // Copy into the trigger's memory context before the executor state,
        // which owns the result, is freed.
        let mut typlen = 0;
        let mut typbyval = false;
        pg_sys::get_typlenbyval(target_type, &mut typlen, &mut typbyval);
        Some(pg_sys::datumCopy(result, typbyval, typlen.into()))
    }
}

// Owns an EState and frees it on every exit path.
struct ExecutorState(*mut pg_sys::EState);

impl ExecutorState {
    fn new() -> Self {
        Self(unsafe { pg_sys::CreateExecutorState() })
    }
}

impl Drop for ExecutorState {
    fn drop(&mut self) {
        unsafe {
            if std::thread::panicking() {
                // Unwinding from an ERROR: like an aborting transaction, free
                // the memory without running expression shutdown callbacks.
                while !(*self.0).es_exprcontexts.is_null() {
                    let first = (*(*(*self.0).es_exprcontexts).elements).ptr_value;
                    pg_sys::FreeExprContext(first.cast(), false);
                }
                pg_sys::MemoryContextDelete((*self.0).es_query_cxt);
            } else {
                pg_sys::FreeExecutorState(self.0);
            }
        }
    }
}

fn format_type(oid: pg_sys::Oid, typmod: i32) -> String {
    unsafe {
        let name = pg_sys::format_type_with_typemod(oid, typmod);
        let result = CStr::from_ptr(name).to_string_lossy().into_owned();
        pg_sys::pfree(name.cast());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{split_long_token, split_long_tokens, MAX_TOKEN_LENGTH};

    #[test]
    fn short_tokens_are_unchanged() {
        assert_eq!(split_long_token(String::new()), [""]);
        assert_eq!(split_long_token("a".repeat(2600)), ["a".repeat(2600)]);
        let tokens = vec!["x".to_string(), "ア".repeat(866)];
        assert_eq!(split_long_tokens(tokens.clone()), tokens);
    }

    #[test]
    fn ascii_tokens_are_split_in_order() {
        let token = format!("{}{}", "a".repeat(2600), "b");
        assert_eq!(split_long_token(token), ["a".repeat(2600), "b".into()]);
        let token = format!("{}{}", "c".repeat(2600), "d".repeat(2600));
        assert_eq!(
            split_long_token(token),
            ["c".repeat(2600), "d".repeat(2600)]
        );
        let token = format!("{}{}{}", "e".repeat(2600), "f".repeat(2600), "g");
        assert_eq!(
            split_long_token(token),
            ["e".repeat(2600), "f".repeat(2600), "g".into()]
        );
    }

    #[test]
    fn multibyte_characters_are_not_cut() {
        // 2598 bytes fit; the 867th character would end at byte 2601.
        let pieces = split_long_token("ア".repeat(1000));
        assert_eq!(pieces, ["ア".repeat(866), "ア".repeat(134)]);
        // A 4-byte character crossing the limit moves to the next piece.
        let token = format!("{}{}{}", "a".repeat(2598), "🦀", "z");
        assert_eq!(
            split_long_token(token),
            ["a".repeat(2598), "🦀z".to_string()]
        );
        // A 2-byte character ending exactly at the limit stays.
        let token = format!("{}{}{}", "a".repeat(2598), "é", "z");
        assert_eq!(
            split_long_token(token),
            [format!("{}é", "a".repeat(2598)), "z".to_string()]
        );
        for len in [2601, 3000, 7801, 100_000] {
            let token: String = "aé中🦀".chars().cycle().take(len).collect();
            let pieces = split_long_token(token.clone());
            assert_eq!(pieces.concat(), token);
            assert!(pieces
                .iter()
                .all(|p| !p.is_empty() && p.len() <= MAX_TOKEN_LENGTH));
            assert!(pieces[..pieces.len() - 1]
                .iter()
                .all(|p| p.len() > MAX_TOKEN_LENGTH - 4));
        }
    }

    #[test]
    fn mixed_tokens_keep_their_positions() {
        let tokens = vec![
            "x".to_string(),
            "b".repeat(2601),
            "y".to_string(),
            "c".repeat(5200),
            "z".to_string(),
        ];
        assert_eq!(
            split_long_tokens(tokens),
            [
                "x".to_string(),
                "b".repeat(2600),
                "b".into(),
                "y".into(),
                "c".repeat(2600),
                "c".repeat(2600),
                "z".into()
            ]
        );
    }
}
