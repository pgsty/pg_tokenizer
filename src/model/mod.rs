// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

mod builtin;
mod custom;
mod huggingface;
mod lindera;

use std::sync::{Arc, LazyLock};

use builtin::{get_builtin_model, is_builtin_model};
use custom::{CustomModel, CustomModelConfig};
use dashmap::DashMap;
use huggingface::{HuggingFaceConfig, HuggingFaceModel};
use lindera::{LinderaConfig, LinderaModel};
use serde::{Deserialize, Serialize};

use crate::utils::{catalog_one, invalid_config};

pub trait TokenizerModel {
    fn apply(&self, token: String) -> Vec<u32>;

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<u32> {
        let mut budget = crate::budget::Output::default();
        let mut ids = Vec::new();
        for token in tokens {
            crate::utils::check_for_interrupts();
            let output = self.apply(token);
            budget.ids(output.len());
            ids.extend(output);
        }
        ids
    }
}
pub type TokenizerModelPtr = Arc<dyn TokenizerModel + Send + Sync>;

impl TokenizerModel for tokenizers::Tokenizer {
    fn apply(&self, token: String) -> Vec<u32> {
        crate::budget::piece(token.len());
        if let Some(padding) = self.get_padding() {
            if let tokenizers::PaddingStrategy::Fixed(length) = padding.strategy {
                crate::budget::tokens(length);
                crate::budget::bytes(length.saturating_mul(96 + padding.pad_token.len()));
            }
            if let Some(multiple) = padding.pad_to_multiple_of {
                crate::budget::tokens(multiple);
                crate::budget::bytes(multiple.saturating_mul(96 + padding.pad_token.len()));
            }
        }
        let encoded = self.encode_fast(token, false).unwrap();
        crate::budget::Output::default().ids(encoded.get_ids().len());
        encoded.get_ids().to_vec()
    }
}

impl TokenizerModel for tocken::tokenizer::Tokenizer {
    fn apply(&self, token: String) -> Vec<u32> {
        crate::budget::piece(token.len());
        let ids = self.tokenize(&token);
        crate::budget::tokens(ids.len());
        ids
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
enum ModelConfig {
    Custom(CustomModelConfig),
    Lindera(LinderaConfig),
    HuggingFace(HuggingFaceConfig),
}

type ModelObjectPool = DashMap<String, TokenizerModelPtr>;
pub(super) static MODEL_OBJECT_POOL: LazyLock<ModelObjectPool> =
    LazyLock::new(ModelObjectPool::default);
// Built by the postmaster from preload files and inherited by every backend.
// Reused only when the current database's catalog has the same definition.
static PRELOADED_MODELS: LazyLock<DashMap<String, (String, TokenizerModelPtr)>> =
    LazyLock::new(DashMap::default);

pgrx::extension_sql!(
    r#"
CREATE TABLE tokenizer_catalog.model (
    name TEXT NOT NULL UNIQUE PRIMARY KEY,
    config TEXT NOT NULL
);
"#,
    name = "model_table"
);

pub fn get_model(name: &str) -> TokenizerModelPtr {
    validate_model_name(name).unwrap();
    crate::cache::check();
    if let Some(model) = MODEL_OBJECT_POOL.get(name) {
        return model.clone();
    }

    let object = match get_builtin_model(name) {
        Some(object) => object,
        None => {
            get_model_from_database(name).unwrap_or_else(|| panic!("Model not found: {}", name))
        }
    };
    MODEL_OBJECT_POOL.insert(name.to_string(), object.clone());
    object
}

pub(super) fn clear_cache() {
    MODEL_OBJECT_POOL.clear();
}

fn get_model_from_database(name: &str) -> Option<TokenizerModelPtr> {
    let config: String = catalog_one(
        "SELECT config FROM tokenizer_catalog.model WHERE name = $1",
        &[name.into()],
    )?;
    if let Some(preloaded) = PRELOADED_MODELS.get(name) {
        if preloaded.0 == config {
            return Some(preloaded.1.clone());
        }
    }
    let config = serde_json::from_str(&config).unwrap_or_else(|e| invalid_config("model", name, e));
    Some(build_model(name, &config))
}

fn get_model_config(name: &str) -> Option<ModelConfig> {
    let config_bytes: String = catalog_one(
        "SELECT config FROM tokenizer_catalog.model WHERE name = $1",
        &[name.into()],
    )?;

    Some(serde_json::from_str(&config_bytes).unwrap_or_else(|e| invalid_config("model", name, e)))
}

fn build_model(name: &str, config: &ModelConfig) -> TokenizerModelPtr {
    match config {
        ModelConfig::Custom(config) => Arc::new(CustomModel::new(name, config)),
        ModelConfig::Lindera(config) => Arc::new(LinderaModel::new(config)),
        ModelConfig::HuggingFace(config) => Arc::new(HuggingFaceModel::new(name, config)),
    }
}

// 1. It only contains ascii letters, numbers, and underscores.
// 2. It starts with a letter.
// 3. Its length cannot exceed 20 characters.
pub fn validate_model_name(name: &str) -> Result<(), String> {
    let name_bytes = name.as_bytes();
    for &b in name_bytes {
        if !b.is_ascii_alphanumeric() && b != b'_' {
            return Err(format!("Invalid character: {}", b as char));
        }
    }
    if !(1..=20).contains(&name_bytes.len()) {
        return Err(format!("Name length must be between 1 and {}", 20));
    }
    if !name_bytes[0].is_ascii_alphabetic() {
        return Err("Name must start with a letter".to_string());
    }

    Ok(())
}

pub fn validate_new_model_name(name: &str) -> Result<(), String> {
    validate_model_name(name)?;
    if is_builtin_model(name) {
        return Err("The name is reserved, please choose another name".to_string());
    }

    Ok(())
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
pub fn add_preload_model(name: &str) {
    validate_model_name(name).unwrap();
    get_model(name);

    let dir_path = std::path::Path::new("pg_tokenizer/preload_models");
    if let Err(e) = std::fs::create_dir_all(dir_path) {
        pgrx::error!("Failed to create preload directory: {}", e);
    }
    let path = dir_path.join(name);
    let content = if is_builtin_model(name) {
        String::new()
    } else {
        let config = get_model_config(name).unwrap();
        serde_json::to_string(&config).unwrap()
    };
    if let Err(e) = std::fs::write(&path, content) {
        pgrx::error!("Failed to save preload model: {}", e);
    }
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
pub fn remove_preload_model(name: &str) {
    validate_model_name(name).unwrap();
    let path = std::path::Path::new("pg_tokenizer/preload_models").join(name);
    match std::fs::remove_file(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            pgrx::warning!("Preload model not found: {}", name);
        }
        Err(e) => {
            pgrx::error!("Failed to remove file: {}", e);
        }
    }
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
pub fn list_preload_models() -> Vec<String> {
    let dir_path = std::path::Path::new("pg_tokenizer/preload_models");
    let entries = match std::fs::read_dir(dir_path) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => pgrx::error!("Failed to read preload directory: {}", e),
    };
    let mut models = Vec::new();
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| pgrx::error!("Failed to read preload entry: {}", e));
        let name = entry.file_name().into_string().unwrap();
        models.push(name);
    }

    models
}

pub fn init() {
    let dir_path = std::path::Path::new("pg_tokenizer/preload_models");
    if !dir_path.exists() {
        match std::fs::create_dir_all(dir_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                pgrx::warning!("pg_tokenizer: cannot create preload directory: {}", e);
                return;
            }
        }
        for name in builtin::PRELOAD_MODELS {
            let path = dir_path.join(name);
            if let Err(e) = std::fs::write(&path, "") {
                pgrx::warning!("pg_tokenizer: cannot save default preload {}: {}", name, e);
            }
        }
    }

    let entries = match std::fs::read_dir(dir_path) {
        Ok(entries) => entries,
        Err(e) => {
            pgrx::warning!("pg_tokenizer: cannot read preload directory: {}", e);
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                pgrx::warning!("pg_tokenizer: cannot read preload entry: {}", e);
                continue;
            }
        };
        // All per-entry validation belongs inside the recovery boundary.
        // Preloading is an optimization; a stale entry must not stop startup.
        let loaded = std::panic::catch_unwind(|| {
            let name = entry
                .file_name()
                .into_string()
                .expect("invalid preload name");
            validate_model_name(&name).unwrap();
            if get_builtin_model(&name).is_some() {
                return;
            }
            let content = std::fs::read_to_string(entry.path()).unwrap();
            let config: ModelConfig = serde_json::from_str(&content).unwrap();
            let object = build_model(&name, &config);
            PRELOADED_MODELS.insert(name, (content, object));
        });
        if loaded.is_err() {
            pgrx::warning!(
                "pg_tokenizer: skipping preload model that failed to load: {}",
                entry.file_name().to_string_lossy()
            );
        }
    }
}
