// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::{Arc, LazyLock};

use super::TokenizerModelPtr;

macro_rules! MODEL_DIR {
    () => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/model")
    };
}

static BERT_BASE_UNCASED_BYTES: &[u8] =
    include_bytes!(concat!(MODEL_DIR!(), "/bert_base_uncased.json"));
static BERT_BASE_UNCASED: LazyLock<Arc<tokenizers::Tokenizer>> =
    LazyLock::new(|| Arc::new(tokenizers::Tokenizer::from_bytes(BERT_BASE_UNCASED_BYTES).unwrap()));
static WIKI_TOCKEN_STR: &str = include_str!(concat!(MODEL_DIR!(), "/wiki_tocken.json"));
static WIKI_TOCKEN: LazyLock<Arc<tocken::tokenizer::Tokenizer>> =
    LazyLock::new(|| Arc::new(tocken::tokenizer::Tokenizer::loads(WIKI_TOCKEN_STR)));
static GEMMA2B_BYTES: &[u8] = include_bytes!(concat!(MODEL_DIR!(), "/gemma2b.json"));
static GEMMA2B: LazyLock<Arc<tokenizers::Tokenizer>> =
    LazyLock::new(|| Arc::new(tokenizers::Tokenizer::from_bytes(GEMMA2B_BYTES).unwrap()));
static LLMLINGUA2_BYTES: &[u8] = include_bytes!(concat!(MODEL_DIR!(), "/llmlingua2.json"));
static LLMLINGUA2: LazyLock<Arc<tokenizers::Tokenizer>> =
    LazyLock::new(|| Arc::new(tokenizers::Tokenizer::from_bytes(LLMLINGUA2_BYTES).unwrap()));

pub const PRELOAD_MODELS: &[&str] = &["llmlingua2"];

pub fn is_builtin_model(name: &str) -> bool {
    matches!(
        name,
        "bert_base_uncased" | "wiki_tocken" | "gemma2b" | "llmlingua2"
    )
}

pub fn get_builtin_model(name: &str) -> Option<TokenizerModelPtr> {
    match name {
        "bert_base_uncased" => Some(BERT_BASE_UNCASED.clone()),
        "wiki_tocken" => Some(WIKI_TOCKEN.clone()),
        "gemma2b" => Some(GEMMA2B.clone()),
        "llmlingua2" => Some(LLMLINGUA2.clone()),
        _ => None,
    }
}
