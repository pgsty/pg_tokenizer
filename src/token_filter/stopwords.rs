// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.

use std::{
    collections::HashSet,
    sync::{Arc, LazyLock},
};

use dashmap::{DashMap, Entry};
use serde::{Deserialize, Serialize};

use crate::utils::spi_get_one;

use super::TokenFilter;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopwordsTokenFilter {
    stopwords: HashSet<String>,
}
pub type StopwordsTokenFilterPtr = Arc<StopwordsTokenFilter>;

impl StopwordsTokenFilter {
    // config is a string with multiple lines, each line represents a stopword
    pub fn build(config: &str) -> Self {
        let mut stopwords = HashSet::new();

        for line in config.lines() {
            stopwords.insert(line.to_string());
        }

        StopwordsTokenFilter { stopwords }
    }
}

impl TokenFilter for StopwordsTokenFilter {
    fn apply(&self, token: String) -> Vec<String> {
        if self.stopwords.contains(token.as_str()) {
            vec![]
        } else {
            vec![token]
        }
    }
}

pgrx::extension_sql!(
    r#"
CREATE TABLE tokenizer_catalog.stopwords (
    name TEXT NOT NULL UNIQUE PRIMARY KEY,
    config TEXT NOT NULL
);
"#,
    name = "stopwords_table"
);

type StopwordsObjectPool = DashMap<String, StopwordsTokenFilterPtr>;
static STOPWORDS_OBJECT_POOL: LazyLock<StopwordsObjectPool> =
    LazyLock::new(StopwordsObjectPool::default);

pub fn get_stopwords_token_filter(name: &str) -> StopwordsTokenFilterPtr {
    if let Some(model) = STOPWORDS_OBJECT_POOL.get(name) {
        return model.clone();
    }

    match STOPWORDS_OBJECT_POOL.entry(name.to_string()) {
        Entry::Occupied(entry) => entry.get().clone(),
        Entry::Vacant(entry) => {
            if let Some(object) = get_stopwords_token_filter_from_database(name) {
                entry.insert(object.clone());
                return object;
            }

            panic!("Stopwords not found: {}", name);
        }
    }
}

fn get_stopwords_token_filter_from_database(name: &str) -> Option<StopwordsTokenFilterPtr> {
    let config: &str = spi_get_one(
        "SELECT config FROM tokenizer_catalog.stopwords WHERE name = $1",
        &[name.into()],
    )?;

    let stopwords = StopwordsTokenFilter::build(config);
    Some(Arc::new(stopwords))
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn create_stopwords(name: &str, config: &str) {
    let stopwords = StopwordsTokenFilter::build(config);

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                r#"
                INSERT INTO tokenizer_catalog.stopwords (name, config) VALUES ($1, $2)
                ON CONFLICT (name) DO NOTHING RETURNING 1
                "#,
                Some(1),
                &[name.into(), config.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            panic!("Stopwords already exists: {}", name);
        }

        if STOPWORDS_OBJECT_POOL
            .insert(name.to_string(), Arc::new(stopwords))
            .is_some()
        {
            panic!("Stopwords already exists: {}", name);
        }
    });
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn drop_stopwords(name: &str) {
    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(
                "DELETE FROM tokenizer_catalog.Stopwords WHERE name = $1 RETURNING 1",
                Some(1),
                &[name.into()],
            )
            .unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("Stopwords not found: {}", name);
        }
    });

    STOPWORDS_OBJECT_POOL.remove(name);
}

macro_rules! STOPWORDS_DIR {
    () => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/stopwords")
    };
}

static LUCENE_ENGLISH_STOPWORDS: &str = include_str!(concat!(STOPWORDS_DIR!(), "/lucene_english"));
static NLTK_ENGLISH_STOPWORDS: &str = include_str!(concat!(STOPWORDS_DIR!(), "/nltk_english"));
static ISO_ENGLISH_STOPWORDS: &str = include_str!(concat!(STOPWORDS_DIR!(), "/iso_english"));

fn create_stopwords_when_init(name: &str, config: &str) {
    pgrx::Spi::connect_mut(|client| {
        client
            .update(
                r#"
                INSERT INTO tokenizer_catalog.stopwords (name, config) VALUES ($1, $2)
                ON CONFLICT (name) DO NOTHING
                "#,
                Some(1),
                &[name.into(), config.into()],
            )
            .unwrap();
    });
}

#[pgrx::pg_extern]
pub fn _pg_tokenizer_stopwords_init() {
    create_stopwords_when_init("lucene_english", LUCENE_ENGLISH_STOPWORDS);
    create_stopwords_when_init("nltk_english", NLTK_ENGLISH_STOPWORDS);
    create_stopwords_when_init("iso_english", ISO_ENGLISH_STOPWORDS);
}

pgrx::extension_sql!(
    r#"
    SELECT tokenizer_catalog._pg_tokenizer_stopwords_init();
    "#,
    name = "stopwords_init",
    requires = ["stopwords_table", _pg_tokenizer_stopwords_init]
);
