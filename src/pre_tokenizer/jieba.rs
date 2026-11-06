// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

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

// Full mode lists every dictionary word in the text; it has no HMM step.
struct JiebaFull;

// Precise mode is the most likely segmentation; HMM handles unknown words.
struct JiebaPrecise {
    enable_hmm: bool,
}

// Search mode is precise mode plus the dictionary words inside long words.
struct JiebaSearch {
    enable_hmm: bool,
}

impl PreTokenizer for JiebaFull {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        JIEBA.cut_all(text).into_iter().map(|t| t.word).collect()
    }
}

impl PreTokenizer for JiebaPrecise {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        JIEBA
            .cut(text, self.enable_hmm)
            .into_iter()
            .map(|t| t.word)
            .collect()
    }
}

impl PreTokenizer for JiebaSearch {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        JIEBA
            .cut_for_search(text, self.enable_hmm)
            .into_iter()
            .map(|t| t.word)
            .collect()
    }
}

pub fn create_jieba_pre_tokenizer(config: JiebaConfig) -> PreTokenizerPtr {
    match config.mode {
        JiebaMode::Full => Arc::new(JiebaFull),
        JiebaMode::Precise => Arc::new(JiebaPrecise {
            enable_hmm: config.enable_hmm,
        }),
        JiebaMode::Search => Arc::new(JiebaSearch {
            enable_hmm: config.enable_hmm,
        }),
    }
}

pub fn init() {
    LazyLock::force(&JIEBA);
}

#[cfg(test)]
mod tests {
    use super::{create_jieba_pre_tokenizer, JiebaConfig, JiebaMode};

    fn cut(mode: JiebaMode, enable_hmm: bool, text: &str) -> Vec<String> {
        let pre_tokenizer = create_jieba_pre_tokenizer(JiebaConfig { mode, enable_hmm });
        pre_tokenizer
            .pre_tokenize(text)
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn modes_follow_jieba() {
        let text = "我来到北京清华大学";
        assert_eq!(
            cut(JiebaMode::Full, true, text),
            [
                "我",
                "来",
                "来到",
                "到",
                "北",
                "北京",
                "京",
                "清",
                "清华",
                "清华大学",
                "华",
                "华大",
                "大",
                "大学",
                "学"
            ]
        );
        assert_eq!(
            cut(JiebaMode::Precise, true, text),
            ["我", "来到", "北京", "清华大学"]
        );
        assert_eq!(
            cut(JiebaMode::Search, true, text),
            ["我", "来到", "北京", "清华", "华大", "大学", "清华大学"]
        );
    }

    #[test]
    fn hmm_applies_to_precise_and_search_only() {
        let text = "他来到了网易杭研大厦";
        assert_ne!(
            cut(JiebaMode::Precise, true, text),
            cut(JiebaMode::Precise, false, text)
        );
        assert_eq!(
            cut(JiebaMode::Full, true, text),
            cut(JiebaMode::Full, false, text)
        );
    }
}
