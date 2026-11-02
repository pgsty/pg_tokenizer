// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.

use std::sync::{Arc, LazyLock};

use jieba_rs::Jieba;
use serde::{Deserialize, Serialize};

use super::{PreTokenizer, PreTokenizerPtr};

static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JiebaMode {
    Full,
    Precise,
    Search,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(default)]
pub struct JiebaConfig {
    mode: JiebaMode,
    enable_hmm: bool,
}

impl Default for JiebaConfig {
    fn default() -> Self {
        Self {
            mode: JiebaMode::Search,
            enable_hmm: true,
        }
    }
}

struct JiebaFull {
    enable_hmm: bool,
}

struct JiebaPrecise;

struct JiebaSearch {
    enable_hmm: bool,
}

impl PreTokenizer for JiebaFull {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        JIEBA.cut(text, self.enable_hmm)
    }
}

impl PreTokenizer for JiebaPrecise {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        JIEBA.cut_all(text)
    }
}

impl PreTokenizer for JiebaSearch {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        JIEBA.cut_for_search(text, self.enable_hmm)
    }
}

pub fn create_jieba_pre_tokenizer(config: JiebaConfig) -> PreTokenizerPtr {
    match config.mode {
        JiebaMode::Full => Arc::new(JiebaFull {
            enable_hmm: config.enable_hmm,
        }),
        JiebaMode::Precise => Arc::new(JiebaPrecise),
        JiebaMode::Search => Arc::new(JiebaSearch {
            enable_hmm: config.enable_hmm,
        }),
    }
}

pub fn init() {
    LazyLock::force(&JIEBA);
}
