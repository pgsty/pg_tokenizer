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

use std::io::{self, Read, Write};
use std::sync::{Arc, LazyLock};

use anyhow::{ensure, Context};
use builtin::{get_builtin_model, is_builtin_model};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use custom::{CustomModel, CustomModelConfig};
use dashmap::DashMap;
use huggingface::{HuggingFaceConfig, HuggingFaceModel};
use lindera::{LinderaConfig, LinderaModel};
use serde::{Deserialize, Serialize};

use crate::utils::{catalog_one, invalid_config};

#[inline]
pub(crate) fn u32_to_i32_token_id(id: u32, tokenizer_name: &str) -> i32 {
    i32::try_from(id).unwrap_or_else(|_| {
        crate::utils::raise_error(
            pgrx::pg_sys::errcodes::PgSqlErrorCode::ERRCODE_NUMERIC_VALUE_OUT_OF_RANGE,
            format!(
                "token ID {id} of tokenizer \"{tokenizer_name}\" is out of range for type integer"
            ),
            None,
            Some("Recreate the model with token IDs not greater than 2147483647."),
        )
    })
}

pub(crate) fn push_u32_ids_as_i32(
    ids: &[u32],
    tokenizer_name: &str,
    out: &mut Vec<i32>,
    budget: &mut crate::budget::Output,
) {
    budget.ids(ids.len());
    out.reserve(ids.len());
    for (i, &id) in ids.iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        out.push(u32_to_i32_token_id(id, tokenizer_name));
    }
}

pub trait TokenizerModel {
    fn apply(&self, token: String) -> Vec<u32>;

    fn check_load_privileges(&self) {}

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

    fn apply_str_to_i32(
        &self,
        text: &str,
        tokenizer_name: &str,
        _limits: crate::budget::Limits,
        out: &mut Vec<i32>,
        budget: &mut crate::budget::Output,
    ) {
        let output = self.apply(text.to_owned());
        push_u32_ids_as_i32(&output, tokenizer_name, out, budget);
    }

    fn apply_batch_to_i32(
        &self,
        tokens: Vec<String>,
        tokenizer_name: &str,
        limits: crate::budget::Limits,
        out: &mut Vec<i32>,
    ) {
        let mut budget = crate::budget::Output::new(limits);
        for token in tokens {
            crate::utils::check_for_interrupts();
            self.apply_str_to_i32(&token, tokenizer_name, limits, out, &mut budget);
        }
    }
}
pub type TokenizerModelPtr = Arc<dyn TokenizerModel + Send + Sync>;

fn check_hf_padding_budget(tokenizer: &tokenizers::Tokenizer, limits: crate::budget::Limits) {
    if let Some(padding) = tokenizer.get_padding() {
        if let tokenizers::PaddingStrategy::Fixed(length) = padding.strategy {
            limits.check_tokens(length);
            limits.check_bytes(length.saturating_mul(96 + padding.pad_token.len()));
        }
        if let Some(multiple) = padding.pad_to_multiple_of {
            limits.check_tokens(multiple);
            limits.check_bytes(multiple.saturating_mul(96 + padding.pad_token.len()));
        }
    }
}

impl TokenizerModel for tokenizers::Tokenizer {
    fn apply(&self, token: String) -> Vec<u32> {
        let limits = crate::budget::Limits::current();
        limits.check_piece(token.len());
        check_hf_padding_budget(self, limits);
        let encoded = self.encode_fast(token, false).unwrap();
        crate::budget::Output::new(limits).ids(encoded.get_ids().len());
        encoded.get_ids().to_vec()
    }

    fn apply_str_to_i32(
        &self,
        text: &str,
        tokenizer_name: &str,
        limits: crate::budget::Limits,
        out: &mut Vec<i32>,
        budget: &mut crate::budget::Output,
    ) {
        limits.check_piece(text.len());
        check_hf_padding_budget(self, limits);
        let encoded = self.encode_fast(text, false).unwrap();
        push_u32_ids_as_i32(encoded.get_ids(), tokenizer_name, out, budget);
    }
}

impl TokenizerModel for tocken::tokenizer::Tokenizer {
    fn apply(&self, token: String) -> Vec<u32> {
        crate::budget::piece(token.len());
        let ids = self.tokenize(&token);
        crate::budget::tokens(ids.len());
        ids
    }

    fn apply_str_to_i32(
        &self,
        text: &str,
        tokenizer_name: &str,
        limits: crate::budget::Limits,
        out: &mut Vec<i32>,
        budget: &mut crate::budget::Output,
    ) {
        limits.check_piece(text.len());
        let ids = self.tokenize(text);
        push_u32_ids_as_i32(&ids, tokenizer_name, out, budget);
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
    custom::clear_cache();
}

fn get_model_from_database(name: &str) -> Option<TokenizerModelPtr> {
    let config = get_model_config(name)?;
    if let Some(preloaded) = PRELOADED_MODELS.get(name) {
        if preloaded.0 == config {
            return Some(preloaded.1.clone());
        }
    }
    let config = serde_json::from_str(&config).unwrap_or_else(|e| invalid_config("model", name, e));
    Some(build_model(name, &config))
}

fn get_model_config(name: &str) -> Option<String> {
    catalog_one(
        "SELECT config FROM tokenizer_catalog.model WHERE name = $1",
        &[name.into()],
    )
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

fn sync_directory(_dir: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    _dir.open(".")?.sync_all()?;
    Ok(())
}

fn preload_directory(create: bool) -> io::Result<(Dir, bool)> {
    let mut parent = Dir::open_ambient_dir(".", ambient_authority())?;
    let mut created = false;
    for component in ["pg_tokenizer", "preload_models"] {
        created = false;
        parent = match parent.open_dir_nofollow(component) {
            Err(err) if create && err.kind() == io::ErrorKind::NotFound => {
                match parent.create_dir(component) {
                    Ok(()) => {
                        sync_directory(&parent)?;
                        created = true;
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(err) => return Err(err),
                }
                parent.open_dir_nofollow(component)?
            }
            result => result?,
        };
    }
    Ok((parent, created))
}

fn write_preload(dir: &Dir, name: &str, content: &str) -> io::Result<()> {
    let temporary = format!(".preload-{}", uuid::Uuid::new_v4());
    let mut file = dir.open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
    let written = file
        .write_all(content.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(err) = written.and_then(|()| dir.rename(&temporary, dir, name)) {
        let _ = dir.remove_file(&temporary);
        return Err(err);
    }
    sync_directory(dir)
}

fn preload_names(dir: &Dir, startup: bool) -> io::Result<Vec<String>> {
    let mut models = Vec::new();
    for entry in dir.entries()? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) if startup => {
                pgrx::warning!("pg_tokenizer: cannot read preload directory entry: {}", err);
                continue;
            }
            Err(err) => return Err(err),
        };
        let filename = entry.file_name();
        if filename.as_encoded_bytes().starts_with(b".preload-") {
            continue;
        }
        match (filename.to_str(), entry.file_type()) {
            (Some(name), Ok(kind)) if kind.is_file() && validate_model_name(name).is_ok() => {
                models.push(name.to_owned());
            }
            (_, Err(err)) if err.kind() == io::ErrorKind::NotFound => {}
            (_, Err(err)) if !startup => return Err(err),
            _ if startup => pgrx::warning!("pg_tokenizer: skipping preload model {:?}", filename),
            _ => {}
        }
    }
    models.sort_unstable();
    models.dedup();
    Ok(models)
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
pub fn add_preload_model(name: &str) {
    validate_model_name(name).unwrap();
    get_model(name);
    let content = if is_builtin_model(name) {
        String::new()
    } else {
        get_model_config(name).unwrap_or_else(|| panic!("Model not found: {name}"))
    };
    preload_directory(true)
        .and_then(|(dir, _)| write_preload(&dir, name, &content))
        .unwrap_or_else(|err| pgrx::error!("Failed to save preload model {}: {}", name, err));
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
pub fn remove_preload_model(name: &str) {
    validate_model_name(name).unwrap();
    match preload_directory(false).and_then(|(dir, _)| {
        dir.remove_file(name)?;
        sync_directory(&dir)
    }) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            pgrx::warning!("Preload model not found: {}", name);
        }
        Err(err) => pgrx::error!("Failed to remove preload model {}: {}", name, err),
    }
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
pub fn list_preload_models() -> Vec<String> {
    match preload_directory(false).and_then(|(dir, _)| preload_names(&dir, false)) {
        Ok(models) => models,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => pgrx::error!("Failed to read preload directory: {}", err),
    }
}

fn load_preload(dir: &Dir, name: &str) -> anyhow::Result<()> {
    let file = dir.open_with(
        name,
        OpenOptions::new()
            .read(true)
            .nonblock(true)
            .follow(FollowSymlinks::No),
    )?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "expected a regular file");
    if is_builtin_model(name) {
        ensure!(metadata.len() == 0, "expected an empty built-in marker");
        get_builtin_model(name);
    } else {
        let max = pgrx::pg_sys::MaxAllocSize as u64;
        ensure!(
            metadata.len() <= max,
            "configuration exceeds PostgreSQL's text limit"
        );
        let mut content = String::new();
        file.take(max + 1).read_to_string(&mut content)?;
        ensure!(
            content.len() as u64 <= max,
            "configuration exceeds PostgreSQL's text limit"
        );
        let config = serde_json::from_str(&content).context("invalid model configuration")?;
        let object = build_model(name, &config);
        PRELOADED_MODELS.insert(name.to_owned(), (content, object));
    }
    Ok(())
}

pub fn init() {
    let (dir, created) = match preload_directory(true) {
        Ok(result) => result,
        Err(err) => {
            pgrx::warning!("pg_tokenizer: cannot open preload directory: {}", err);
            return;
        }
    };
    if created {
        for name in builtin::PRELOAD_MODELS {
            if let Err(err) = write_preload(&dir, name, "") {
                pgrx::warning!(
                    "pg_tokenizer: cannot save default preload {}: {}",
                    name,
                    err
                );
            }
        }
    }
    let names = match preload_names(&dir, true) {
        Ok(names) => names,
        Err(err) => {
            pgrx::warning!("pg_tokenizer: cannot read preload directory: {}", err);
            return;
        }
    };
    for name in names {
        match std::panic::catch_unwind(|| load_preload(&dir, &name)) {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                pgrx::warning!("pg_tokenizer: skipping preload model {:?}: {}", name, err)
            }
            Err(_) => pgrx::warning!(
                "pg_tokenizer: skipping preload model {:?}: model failed to load",
                name
            ),
        }
    }
}
