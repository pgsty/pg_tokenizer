// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::Arc;

use tokenizers::Tokenizer;

use super::{validate_new_model_name, ModelConfig, TokenizerModel, MODEL_OBJECT_POOL};

#[derive(Debug)]
pub struct HuggingFaceModel {
    tokenizer: Tokenizer,
}

impl HuggingFaceModel {
    pub fn new(_name: &str, config: &HuggingFaceConfig) -> Self {
        let tokenizer = Tokenizer::from_bytes(config.as_bytes()).expect("Failed to load tokenizer");
        HuggingFaceModel { tokenizer }
    }
}

impl TokenizerModel for HuggingFaceModel {
    fn apply(&self, text: String) -> Vec<u32> {
        self.tokenizer.apply(text)
    }

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<u32> {
        self.tokenizer.apply_batch(tokens)
    }
}

pub type HuggingFaceConfig = String;

#[pgrx::pg_extern(volatile, parallel_safe)]
fn create_huggingface_model(name: &str, config: &str) {
    validate_new_model_name(name).unwrap();

    let insert_model = r#"
        INSERT INTO tokenizer_catalog.model (name, config) VALUES ($1, $2)
        ON CONFLICT (name) DO NOTHING RETURNING 1
    "#;

    let config = config.to_string();
    let model = HuggingFaceModel::new(name, &config);
    let config_str = serde_json::to_string(&ModelConfig::HuggingFace(config)).unwrap();

    pgrx::Spi::connect_mut(|client| {
        let tuptable = client
            .update(insert_model, Some(1), &[name.into(), config_str.into()])
            .unwrap();

        if tuptable.is_empty() {
            panic!("Model already exists: {}", name);
        }

        crate::cache::mark_catalog_changed();
        if MODEL_OBJECT_POOL
            .insert(name.to_string(), Arc::new(model))
            .is_some()
        {
            panic!("Model already exists: {}", name);
        }
    });
}

#[pgrx::pg_extern(volatile, parallel_safe)]
fn drop_huggingface_model(name: &str) {
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
