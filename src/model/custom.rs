// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::{collections::HashMap, ffi::CStr, sync::Arc};

use pgrx::{
    pg_sys::panic::ErrorReportable,
    prelude::PgHeapTuple,
    spi::{quote_identifier, quote_literal},
    WhoAllocated,
};
use serde::{Deserialize, Serialize};

use crate::{text_analyzer::get_text_analyzer, utils::spi_get_one};

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

impl TokenizerModel for CustomModel {
    fn apply(&self, text: String) -> Vec<u32> {
        let query = format!(
            r#"SELECT id FROM tokenizer_catalog."model_{}" WHERE token = $1"#,
            self.name
        );

        let id = spi_get_one::<i32>(&query, &[text.into()]);

        if let Some(id) = id {
            vec![u32::try_from(id).unwrap()]
        } else {
            vec![]
        }
    }

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<u32> {
        let query = format!(
            r#"SELECT id, token FROM tokenizer_catalog."model_{}" WHERE token = ANY($1)"#,
            self.name
        );

        let mut token_map = HashMap::new();
        pgrx::Spi::connect(|client| {
            let tuptable = client
                .select(&query, None, &[tokens.clone().into()])
                .unwrap_or_report();
            for tup in tuptable {
                let id: i32 = tup.get(1).unwrap_or_report().expect("no id value");
                let id = u32::try_from(id).expect("id is not a valid u32");
                let token: String = tup.get(2).unwrap_or_report().expect("no token value");
                token_map.insert(token, id);
            }
        });

        tokens
            .into_iter()
            .filter_map(|token| token_map.get(&token).copied())
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

    let mut config: CustomModelConfig = toml::from_str(config).unwrap();
    config.table = resolve_table(&config.table);
    let table = &config.table;
    let column = quote_identifier(&config.column);

    get_text_analyzer(&config.text_analyzer);

    let create_word_table = format!(
        r#"
        CREATE TABLE tokenizer_catalog."model_{}" (
            id int GENERATED BY DEFAULT AS IDENTITY,
            token TEXT NOT NULL UNIQUE
        );
        "#,
        name
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
        INSERT INTO tokenizer_catalog."model_{name}" (token)
        SELECT DISTINCT token COLLATE "C" FROM tokens
        ORDER BY token COLLATE "C"
        ON CONFLICT (token) DO NOTHING;
        "#,
    );
    let grant_model = format!(
        r#"
        GRANT SELECT, INSERT, UPDATE, DELETE ON tokenizer_catalog."model_{name}" TO pg_database_owner;
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
                Some(1),
                &[format!("tokenizer_catalog.\"model_{name}\"").into()],
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

    let select_config = r#"SELECT config FROM tokenizer_catalog.model WHERE name = $1"#;
    let config_bytes: &str = spi_get_one(select_config, &[name.into()]).unwrap();
    let config: ModelConfig = serde_json::from_str(config_bytes).unwrap();
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
    let drop_table = format!(r#"DROP TABLE IF EXISTS tokenizer_catalog."model_{}""#, name);
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

const MAX_TOKEN_LENGTH: usize = 2600;

#[pgrx::pg_extern(volatile, parallel_safe)]
fn apply_text_analyzer_for_custom_model(text: &str, text_analyzer_name: &str) -> Vec<String> {
    let text_analyzer = get_text_analyzer(text_analyzer_name);
    let mut results = text_analyzer.apply(text);

    // split all tokens that are longer than 2600 characters
    let len = results.len();
    for i in 0..len {
        let token_len = results[i].len();
        if token_len > MAX_TOKEN_LENGTH {
            pgrx::warning!("There is a custom table token whose length has exceeded MAX_TOKEN_LENGTH({MAX_TOKEN_LENGTH}). It will be cut off to multiple tokens. If you need to support long token, welcome to submit an issue to \"https://github.com/pgsty/pg_tokenizer/issues\".");

            let replace_token = results[i][..MAX_TOKEN_LENGTH].to_string();
            let token = std::mem::replace(&mut results[i], replace_token);
            for j in 1..(token.len().div_ceil(MAX_TOKEN_LENGTH)) {
                results.push(token[j * MAX_TOKEN_LENGTH..][..MAX_TOKEN_LENGTH].to_string());
            }
        }
    }
    results
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
    use pgrx::IntoDatum;

    let mut new = trigger.new().expect("new tuple is missing").into_owned();
    let tg_argv = trigger.extra_args().expect("trigger arguments are missing");
    if !(3..=4).contains(&tg_argv.len()) {
        panic!("Invalid trigger arguments");
    }
    let tokenizer_name = &tg_argv[0];
    let source_column = &tg_argv[1];
    let target_column = &tg_argv[2];

    if let Some(filter) = tg_argv.get(3) {
        let row = trigger.new().expect("new tuple is missing");
        let oid = row.composite_type_oid().expect("row type is missing");
        // SAFETY: the OID comes from this exact composite tuple's descriptor.
        let row = unsafe { pgrx::datum::DatumWithOid::new(row, oid) };
        let matches = spi_get_one::<bool>(
            &format!("SELECT ({filter}) IS TRUE FROM (SELECT ($1).*) AS document"),
            &[row],
        )
        .unwrap();
        if !matches {
            // Leave nonmatching rows untouched, including any existing embedding.
            return Ok(Some(new));
        }
    }

    let source = new
        .get_by_name::<&str>(source_column)
        .expect("source column is missing");
    let Some(source) = source else {
        unsafe {
            new.set_by_index_unchecked(
                new.get_attribute_by_name(target_column)
                    .expect("target column is missing")
                    .0,
                None,
            )
        };
        return Ok(Some(new));
    };

    // A preceding vocabulary trigger may have waited for another transaction.
    // Use a read-write SPI call so READ COMMITTED gets a fresh snapshot that can
    // see that writer's committed IDs as well as our own newly inserted tokens.
    let target: Vec<i32> = pgrx::Spi::connect_mut(|client| {
        client
            .update(
                "SELECT tokenizer_catalog.tokenize($1, $2)",
                Some(1),
                &[source.into(), tokenizer_name.as_str().into()],
            )
            .unwrap_or_report()
            .first()
            .get_one()
            .unwrap_or_report()
            .expect("tokenizer result is missing")
    });
    let (idx, att) = new
        .get_attribute_by_name(target_column)
        .expect("get target column failed");
    let attoid = att.type_oid().value();
    if Vec::<i32>::is_compatible_with(attoid) {
        new.set_by_index(idx, target)
            .expect("set target column failed");
    } else {
        let target_casted = pgrx::Spi::connect(|client| {
            client
                .select(
                    &format!("SELECT $1::{}", lookup_type_name(attoid)),
                    Some(1),
                    &[target.into()],
                )
                .unwrap_or_report();

            unsafe {
                let table = pgrx::pg_sys::SPI_tuptable.as_mut().unwrap();
                if table.numvals != 1 {
                    panic!("unexpected number of tuples returned");
                }
                let heap_tuple = *(table.vals);
                let heap_tuple = pgrx::pg_sys::SPI_copytuple(heap_tuple);

                let mut is_null = false;
                let datum = pgrx::pg_sys::SPI_getbinval(heap_tuple, table.tupdesc, 1, &mut is_null);

                if is_null {
                    panic!("unexpected null value");
                }
                datum
            }
        });
        unsafe { new.set_by_index_unchecked(idx, Some(target_casted)) };
    }

    Ok(Some(new))
}

fn lookup_type_name(oid: pgrx::pg_sys::Oid) -> String {
    unsafe {
        // SAFETY: nothing to concern ourselves with other than just calling into Postgres FFI
        // and Postgres will raise an ERROR if we pass it an invalid Oid, so it'll never return a null
        let cstr_name = pgrx::pg_sys::format_type_extended(oid, -1, 0);
        let cstr = CStr::from_ptr(cstr_name);
        let typname = cstr.to_string_lossy().to_string();
        pgrx::pg_sys::pfree(cstr_name as _); // don't leak the palloc'd cstr_name
        typname
    }
}
