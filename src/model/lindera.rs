// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::Arc;

use anyhow::Context;
use lindera::dictionary::DictionaryKind;
use lindera_analysis::tokenizer::{Tokenizer, TokenizerConfig};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{validate_new_model_name, ModelConfig, TokenizerModel, MODEL_OBJECT_POOL};

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

pub struct LinderaModel {
    tokenizer: Tokenizer,
}

impl LinderaModel {
    pub fn new(config: &LinderaConfig) -> Self {
        let tokenizer = Tokenizer::from_config(&config.tokenizer_config().unwrap()).unwrap();
        Self { tokenizer }
    }
}

impl TokenizerModel for LinderaModel {
    fn apply(&self, token: String) -> Vec<u32> {
        self.tokenizer
            .tokenize(&token)
            .unwrap()
            .into_iter()
            .filter(|t| !t.word_id.is_unknown())
            .map(|t| t.word_id.id())
            .filter(|id| *id != u32::MAX)
            .collect()
    }
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

    MODEL_OBJECT_POOL.remove(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            let model = LinderaModel::new(&config);
            assert_eq!(model.apply("関西国際空港限定".into()), [372982, 374178]);
        }
    }

    #[cfg(feature = "lindera-ipadic")]
    #[test]
    fn unknown_words_do_not_leak_unknown_dictionary_ids() {
        let config: LinderaConfig =
            toml::from_str("[segmenter]\ndictionary = 'embedded://ipadic'").unwrap();
        let model = LinderaModel::new(&config);
        assert!(model.apply("トートバッグ".into()).is_empty());
        assert_eq!(
            model.apply("関西国際空港限定トートバッグ".into()),
            [372982, 374178]
        );
    }
}
