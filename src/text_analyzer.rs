// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::character_filter::{get_character_filter, CharacterFilterConfig, CharacterFilterPtr};
use crate::pre_tokenizer::{get_pre_tokenizer, PreTokenizerConfig, PreTokenizerPtr};
use crate::token_filter::{get_token_filter, TokenFilterConfig, TokenFilterPtr};
use crate::utils::{catalog_one, check_for_interrupts, invalid_config};

#[derive(Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct TextAnalyzerConfig {
    #[serde(default)]
    pub character_filters: Vec<CharacterFilterConfig>,
    #[serde(default)]
    pub pre_tokenizer: Option<PreTokenizerConfig>,
    #[serde(default)]
    pub token_filters: Vec<TokenFilterConfig>,
}

pub struct TextAnalyzer {
    pub character_filters: Vec<CharacterFilterPtr>,
    pub pre_tokenizer: Option<PreTokenizerPtr>,
    pub token_filters: Vec<TokenFilterPtr>,
}
pub type TextAnalyzerPtr = Arc<TextAnalyzer>;

impl TextAnalyzer {
    pub fn build(config: TextAnalyzerConfig) -> Self {
        let character_filters = config
            .character_filters
            .into_iter()
            .map(get_character_filter)
            .collect();
        let pre_tokenizer = config.pre_tokenizer.map(get_pre_tokenizer);
        let token_filters = config
            .token_filters
            .into_iter()
            .map(get_token_filter)
            .collect();

        TextAnalyzer {
            character_filters,
            pre_tokenizer,
            token_filters,
        }
    }

    pub fn apply(&self, text: &str) -> Vec<String> {
        crate::budget::input(text.len());
        // Let a statement timeout or cancel stop the pipeline between stages;
        // token filters also check per token.
        let mut text = Cow::Borrowed(text);
        for filter in &self.character_filters {
            check_for_interrupts();
            filter.apply(&mut text);
        }

        check_for_interrupts();
        let text = text.as_ref();
        let pre_tokenized = match &self.pre_tokenizer {
            Some(pre_tokenizer) => pre_tokenizer.pre_tokenize(text),
            None => vec![text],
        };
        check_for_interrupts();
        let mut budget = crate::budget::Output::default();
        let mut tokens = pre_tokenized
            .into_iter()
            .map(|s| {
                budget.text(s.len());
                s.to_string()
            })
            .collect::<Vec<_>>();

        for filter in &self.token_filters {
            check_for_interrupts();
            tokens = filter.apply_batch(tokens);
            crate::budget::text(&tokens);
        }

        tokens
    }
}

pgrx::extension_sql!(
    r#"
CREATE TABLE tokenizer_catalog.text_analyzer (
    name TEXT NOT NULL UNIQUE PRIMARY KEY,
    config TEXT NOT NULL
);
"#,
    name = "text_analyzer_table"
);

type TextAnalyzerObjectPool = DashMap<String, TextAnalyzerPtr>;
static TEXT_ANALYZER_OBJECT_POOL: LazyLock<TextAnalyzerObjectPool> =
    LazyLock::new(TextAnalyzerObjectPool::default);

pub fn get_text_analyzer(name: &str) -> TextAnalyzerPtr {
    crate::cache::check();
    if let Some(model) = TEXT_ANALYZER_OBJECT_POOL.get(name) {
        return model.clone();
    }

    // Do not hold a map shard while loading: dependencies can call
    // cache::check(), which may clear this map.
    let Some(object) = get_text_analyzer_from_database(name) else {
        panic!("TextAnalyzer not found: {}", name);
    };
    TEXT_ANALYZER_OBJECT_POOL.insert(name.to_string(), object.clone());
    object
}

pub(crate) fn clear_cache() {
    TEXT_ANALYZER_OBJECT_POOL.clear();
}

fn get_text_analyzer_from_database(name: &str) -> Option<TextAnalyzerPtr> {
    let config: String = catalog_one(
        "SELECT config FROM tokenizer_catalog.text_analyzer WHERE name = $1",
        &[name.into()],
    )?;

    let config: TextAnalyzerConfig =
        serde_json::from_str(&config).unwrap_or_else(|e| invalid_config("text analyzer", name, e));
    Some(Arc::new(TextAnalyzer::build(config)))
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
fn create_text_analyzer(name: &str, config: &str) {
    let mut config: TextAnalyzerConfig =
        toml::from_str(config).unwrap_or_else(|e| invalid_config("text analyzer", name, e));
    if let Err(e) = config.validate() {
        invalid_config("text analyzer", name, e);
    }

    crate::token_filter::qualify_dictionaries(&mut config.token_filters);
    let config_str = serde_json::to_string(&config).unwrap();
    let text_analyzer = TextAnalyzer::build(config);

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                r#"
                INSERT INTO tokenizer_catalog.text_analyzer (name, config) VALUES ($1, $2)
                ON CONFLICT (name) DO NOTHING RETURNING 1
                "#,
                None,
                &[name.into(), config_str.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            panic!("Text analyzer already exists: {}", name);
        }

        crate::cache::mark_catalog_changed();
        TEXT_ANALYZER_OBJECT_POOL.insert(name.to_string(), Arc::new(text_analyzer));
    });
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
fn drop_text_analyzer(name: &str) {
    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                "DELETE FROM tokenizer_catalog.text_analyzer WHERE name = $1 RETURNING 1",
                None,
                &[name.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("TextAnalyzer not found: {}", name);
        }
    });

    crate::cache::mark_catalog_changed();
    TEXT_ANALYZER_OBJECT_POOL.remove(name);
}

#[pgrx::pg_extern(stable, parallel_safe)]
fn apply_text_analyzer(text: &str, text_analyzer_name: &str) -> Vec<String> {
    let text_analyzer = get_text_analyzer(text_analyzer_name);
    text_analyzer.apply(text)
}
