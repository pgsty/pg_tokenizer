// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::Arc;

use anyhow::Context;
use lindera::dictionary::{DictionaryKind, WordId};
use lindera_analysis::tokenizer::{Tokenizer, TokenizerConfig};
use pgrx::pg_sys::{self, errcodes::PgSqlErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{validate_new_model_name, ModelConfig, TokenizerModel, MODEL_OBJECT_POOL};
use crate::utils::raise_error;

#[derive(Debug, Serialize, Deserialize)]
pub struct LinderaConfig {
    #[serde(flatten)]
    inner: TokenizerConfig,
}

impl LinderaConfig {
    fn tokenizer_config(&self) -> anyhow::Result<TokenizerConfig> {
        let mut config = self.inner.clone();
        let segmenter = config
            .get_mut("segmenter")
            .and_then(Value::as_object_mut)
            .context("missing segmenter config")?;

        // Catalog rows and preload files from Lindera 0.x use dictionary tables.
        // Convert a copy at load time so existing models continue to load.
        if let Some(dictionary) = segmenter.get("dictionary").and_then(Value::as_object) {
            let uri = if let Some(kind) = dictionary.get("kind") {
                let kind: DictionaryKind = serde_json::from_value(kind.clone())?;
                format!("embedded://{}", kind.as_str())
            } else {
                dictionary
                    .get("path")
                    .and_then(Value::as_str)
                    .context("dictionary kind or path must be specified")?
                    .to_owned()
            };
            let use_mmap = dictionary
                .get("memmap")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            segmenter.insert("dictionary".into(), Value::String(uri));
            segmenter.entry("use_mmap").or_insert(Value::Bool(use_mmap));
        }
        if let Some(dictionary) = segmenter.get("user_dictionary").and_then(Value::as_object) {
            let path = dictionary
                .get("path")
                .and_then(Value::as_str)
                .context("user dictionary path must be specified")?
                .to_owned();
            segmenter.insert("user_dictionary".into(), Value::String(path));
        }

        Ok(config)
    }
}

// The values Lindera opens when it builds a tokenizer from a normalized
// configuration: the system dictionary and the optional user dictionary.
// Lindera ignores a non-string value for either key, and so does this check.
fn dictionary_sources(config: &TokenizerConfig) -> impl Iterator<Item = &str> {
    let segmenter = config.get("segmenter");
    ["dictionary", "user_dictionary"]
        .into_iter()
        .filter_map(move |key| segmenter?.get(key)?.as_str())
}

// Only an `embedded://` URI is served from the library itself. Lindera treats
// every other value (a plain path, a `file://` URI, or anything it cannot
// parse) as a file to open, so this deliberately does not try to parse URIs.
fn reads_server_files(config: &TokenizerConfig) -> bool {
    dictionary_sources(config).any(|source| !source.starts_with("embedded://"))
}

// PostgreSQL reserves reading arbitrary server files for superusers and roles
// with the privileges of pg_read_server_files (role inheritance honoured).
// Preload runs in the postmaster from files only an administrator can write.
fn may_read_server_files() -> bool {
    unsafe {
        pg_sys::process_shared_preload_libraries_in_progress
            || pg_sys::superuser()
            || pg_sys::has_privs_of_role(
                pg_sys::GetUserId(),
                pg_sys::Oid::from(pg_sys::ROLE_PG_READ_SERVER_FILES),
            )
    }
}

pub struct LinderaModel {
    tokenizer: Tokenizer,
}

impl LinderaModel {
    // Every load goes through here: creation, the first use in a backend (from
    // the catalog, which also covers rows written directly) and preload.
    // Authorization applies to loading; an already loaded object is reused.
    pub fn new(config: &LinderaConfig) -> Self {
        let config = config.tokenizer_config().unwrap_or_else(|e| {
            raise_error(
                PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                "invalid Lindera configuration",
                Some(e.to_string()),
                None,
            )
        });
        let privileged = may_read_server_files();
        if !privileged && reads_server_files(&config) {
            raise_error(
                PgSqlErrorCode::ERRCODE_INSUFFICIENT_PRIVILEGE,
                "permission denied to load a Lindera dictionary from a file",
                Some(
                    "Only roles with privileges of the \"pg_read_server_files\" role may use \
                     Lindera dictionary or user dictionary paths."
                        .into(),
                ),
                Some("Use an embedded:// dictionary."),
            );
        }
        // Lindera errors can quote file names and file contents. Only roles
        // that may read server files anyway get the details.
        Self::build(&config).unwrap_or_else(|e| {
            raise_error(
                PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                "failed to load Lindera dictionary",
                privileged.then(|| e.to_string()),
                None,
            )
        })
    }

    // Builds without the authorization check; callers must perform it first.
    fn build(config: &TokenizerConfig) -> lindera::LinderaResult<Self> {
        let tokenizer = Tokenizer::from_config(config)?;
        Ok(Self { tokenizer })
    }
}

impl TokenizerModel for LinderaModel {
    fn apply(&self, token: String) -> Vec<u32> {
        self.tokenizer
            .tokenize(&token)
            .unwrap()
            .into_iter()
            .filter_map(|t| token_id(t.word_id))
            .collect()
    }
}

// Lindera IDs are local to each dictionary. Preserve existing system IDs and
// reserve bit 30 for user entries, keeping both namespaces within SQL INT.
const USER_DICTIONARY_ID_BASE: u32 = 1 << 30;

fn token_id(word: WordId) -> Option<u32> {
    if word.is_unknown() || word.id() == u32::MAX {
        return None;
    }
    assert!(
        word.id() < USER_DICTIONARY_ID_BASE,
        "Lindera dictionary has too many entries for pg_tokenizer token IDs"
    );
    Some(
        word.id()
            + if word.is_system() {
                0
            } else {
                USER_DICTIONARY_ID_BASE
            },
    )
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn create_lindera_model(name: &str, config: &str) {
    validate_new_model_name(name).unwrap();
    let config: LinderaConfig = toml::from_str(config).unwrap();

    let insert_model = r#"
        INSERT INTO tokenizer_catalog.model (name, config) VALUES ($1, $2)
        ON CONFLICT (name) DO NOTHING RETURNING 1
    "#;

    let lindera_model = LinderaModel::new(&config);
    let config_str = serde_json::to_string(&ModelConfig::Lindera(config)).unwrap();

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(insert_model, Some(1), &[name.into(), config_str.into()])
            .unwrap();

        if tuptable.is_empty() {
            panic!("Model already exists: {}", name);
        }

        crate::cache::mark_catalog_changed();
        MODEL_OBJECT_POOL.insert(name.to_string(), Arc::new(lindera_model));
    });
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn drop_lindera_model(name: &str) {
    validate_new_model_name(name).unwrap();

    let delete_model = r#"
        DELETE FROM tokenizer_catalog.model WHERE name = $1 RETURNING 1
    "#;

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(delete_model, Some(1), &[name.into()])
            .unwrap();

        if tuptable.is_empty() {
            pgrx::warning!("Model not found: {}", name);
        }
    });

    crate::cache::mark_catalog_changed();
    MODEL_OBJECT_POOL.remove(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Unit tests run without a PostgreSQL backend, so they build models
    // without the backend authorization check.
    fn build(config: &LinderaConfig) -> LinderaModel {
        LinderaModel::build(&config.tokenizer_config().unwrap()).unwrap()
    }

    fn reads_files(segmenter: Value) -> bool {
        let config = LinderaConfig {
            inner: json!({ "segmenter": segmenter }),
        };
        reads_server_files(&config.tokenizer_config().unwrap())
    }

    #[test]
    fn embedded_dictionaries_do_not_read_server_files() {
        assert!(!reads_files(json!({"dictionary": "embedded://ipadic"})));
        assert!(!reads_files(json!({"dictionary": {"kind": "ipadic"}})));
        assert!(!reads_files(json!({
            "dictionary": "embedded://ipadic",
            "user_dictionary": "embedded://ipadic"
        })));
        // Lindera ignores non-string values, so they open nothing.
        assert!(!reads_files(
            json!({"dictionary": 1, "user_dictionary": []})
        ));
        assert!(!reads_files(json!({})));
    }

    #[test]
    fn dictionary_paths_read_server_files() {
        for segmenter in [
            json!({"dictionary": "/etc"}),
            json!({"dictionary": "relative/dictionary"}),
            json!({"dictionary": "file:///etc"}),
            json!({"dictionary": {"path": "/etc"}}),
            json!({"dictionary": {"path": "/etc", "memmap": true}}),
            json!({"dictionary": "embedded://ipadic", "user_dictionary": "/data/user.csv"}),
            json!({"dictionary": "embedded://ipadic", "user_dictionary": "file:///data/user.csv"}),
            json!({"dictionary": {"kind": "ipadic"}, "user_dictionary": {"path": "/data/user.csv"}}),
            // Not parsed as embedded URIs: refuse rather than guess.
            json!({"dictionary": "EMBEDDED://ipadic"}),
            json!({"dictionary": " embedded://ipadic"}),
            json!({"dictionary": "embedded:/ipadic"}),
        ] {
            assert!(reads_files(segmenter.clone()), "{segmenter}");
        }
    }

    #[cfg(feature = "lindera-ipadic")]
    #[test]
    fn user_dictionary_ids_do_not_collide_with_system_dictionary() {
        let config = LinderaConfig {
            inner: json!({"segmenter": {
                "dictionary": "embedded://ipadic",
                "user_dictionary": concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ipadic-user.csv")
            }}),
        };
        let model = build(&config);
        let words = model.tokenizer.tokenize("東京スカイツリー").unwrap();
        assert_eq!(words.len(), 1);
        let user = words[0].word_id;
        assert!(!user.is_system() && !user.is_unknown());
        let system = WordId::new(Default::default(), user.id());
        assert_ne!(token_id(user), token_id(system));
        assert_eq!(
            model.apply("東京スカイツリー".into()),
            [USER_DICTIONARY_ID_BASE]
        );
        assert_eq!(model.apply("関西国際空港限定".into()), [372982, 374178]);
    }

    #[cfg(feature = "lindera-ipadic")]
    #[test]
    fn legacy_dictionary_config_preserves_filters() {
        let config: LinderaConfig = toml::from_str(
            r#"
            [segmenter]
            mode = "normal"
            [segmenter.dictionary]
            kind = "ipadic"
            [[character_filters]]
            kind = "unicode_normalize"
            [character_filters.args]
            kind = "nfkc"
            "#,
        )
        .unwrap();
        let normalized = config.tokenizer_config().unwrap();
        assert_eq!(normalized["segmenter"]["dictionary"], "embedded://ipadic");
        assert_eq!(normalized["segmenter"]["mode"], "normal");
        assert_eq!(
            normalized["character_filters"],
            config.inner["character_filters"]
        );
        assert_eq!(config.inner["segmenter"]["dictionary"]["kind"], "ipadic");
    }

    #[test]
    fn legacy_file_and_user_dictionary_config() {
        let config = LinderaConfig {
            inner: json!({"segmenter": {
                "dictionary": {"path": "/data/ipadic", "memmap": true},
                "user_dictionary": {"kind": "ipadic", "path": "/data/user.csv"}
            }}),
        };
        assert_eq!(
            config.tokenizer_config().unwrap(),
            json!({"segmenter": {
                "dictionary": "/data/ipadic",
                "user_dictionary": "/data/user.csv",
                "use_mmap": true
            }})
        );
    }

    #[test]
    fn native_dictionary_config_is_unchanged() {
        let config = LinderaConfig {
            inner: json!({"segmenter": {
                "dictionary": "embedded://ipadic",
                "user_dictionary": "file:///data/user.csv",
                "keep_whitespace": true
            }}),
        };
        assert_eq!(config.tokenizer_config().unwrap(), config.inner);
    }

    #[test]
    fn invalid_legacy_dictionary_config_is_rejected() {
        for dictionary in [json!({}), json!({"kind": 1}), json!({"kind": "invalid"})] {
            let config = LinderaConfig {
                inner: json!({"segmenter": {"dictionary": dictionary}}),
            };
            assert!(config.tokenizer_config().is_err());
        }
    }

    #[cfg(feature = "lindera-ipadic")]
    #[test]
    fn legacy_catalog_config_and_native_config_produce_same_ids() {
        let legacy: LinderaConfig = serde_json::from_value(json!({
            "segmenter": {"mode": "normal", "dictionary": {"kind": "ipadic"}}
        }))
        .unwrap();
        let native: LinderaConfig =
            toml::from_str("[segmenter]\nmode = 'normal'\ndictionary = 'embedded://ipadic'")
                .unwrap();
        for config in [legacy, native] {
            let model = build(&config);
            assert_eq!(model.apply("関西国際空港限定".into()), [372982, 374178]);
        }
    }

    #[cfg(feature = "lindera-ipadic")]
    #[test]
    fn unknown_words_do_not_leak_unknown_dictionary_ids() {
        let config: LinderaConfig =
            toml::from_str("[segmenter]\ndictionary = 'embedded://ipadic'").unwrap();
        let model = build(&config);
        assert!(model.apply("トートバッグ".into()).is_empty());
        assert_eq!(
            model.apply("関西国際空港限定トートバッグ".into()),
            [372982, 374178]
        );
    }
}
