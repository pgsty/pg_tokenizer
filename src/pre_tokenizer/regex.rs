// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use super::PreTokenizer;

pub struct RegexPreTokenizer {
    pattern: regex::Regex,
}

impl RegexPreTokenizer {
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: regex::Regex::new(pattern).unwrap(),
        }
    }
}

impl PreTokenizer for RegexPreTokenizer {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        self.pattern.find_iter(text).map(|m| m.as_str()).collect()
    }
}
