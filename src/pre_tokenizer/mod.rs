// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

mod jieba;
mod regex;
mod unicode_segmentation;

use std::sync::Arc;

use jieba::{create_jieba_pre_tokenizer, JiebaConfig};
use regex::RegexPreTokenizer;
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentationPretokenizer;

pub trait PreTokenizer {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str>;
}
pub type PreTokenizerPtr = Arc<dyn PreTokenizer + Send + Sync>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum PreTokenizerConfig {
    Regex(String),
    UnicodeSegmentation,
    Jieba(JiebaConfig),
}

pub fn get_pre_tokenizer(config: PreTokenizerConfig) -> PreTokenizerPtr {
    match config {
        PreTokenizerConfig::Regex(pattern) => Arc::new(RegexPreTokenizer::new(&pattern)),
        PreTokenizerConfig::UnicodeSegmentation => Arc::new(UnicodeSegmentationPretokenizer),
        PreTokenizerConfig::Jieba(config) => create_jieba_pre_tokenizer(config),
    }
}

pub fn init() {
    jieba::init();
}
