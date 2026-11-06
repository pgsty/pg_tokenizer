// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use super::TokenFilter;

#[derive(Clone, Debug, Serialize, Deserialize, Validate)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
#[validate(schema(function = "NgramConfig::validate_grams"))]
pub struct NgramConfig {
    #[serde(default = "NgramConfig::default_max_gram")]
    #[validate(range(min = 1, max = 255))]
    pub max_gram: usize,
    #[serde(default = "NgramConfig::default_min_gram")]
    #[validate(range(min = 1, max = 255))]
    pub min_gram: usize,
    #[serde(default = "NgramConfig::default_preserve_original")]
    pub preserve_original: bool,
}

impl NgramConfig {
    fn default_max_gram() -> usize {
        2
    }
    fn default_min_gram() -> usize {
        1
    }
    fn default_preserve_original() -> bool {
        false
    }
    fn validate_grams(&self) -> Result<(), ValidationError> {
        if self.min_gram > self.max_gram {
            return Err(ValidationError::new(
                "min_gram must be less than or equal to max_gram",
            ));
        }
        Ok(())
    }
}

pub struct Ngram {
    config: NgramConfig,
}

impl TokenFilter for Ngram {
    fn apply(&self, token: String) -> Vec<String> {
        let min_gram = self.config.min_gram;
        let max_gram = self.config.max_gram;
        // N-grams count characters (Unicode scalar values), not bytes.
        let bounds: Vec<usize> = token
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(token.len()))
            .collect();
        let chars = bounds.len() - 1;
        let mut results = Vec::new();
        // A token shorter than min_gram has no n-grams.
        for start in 0..(chars + 1).saturating_sub(min_gram) {
            for end in (start + min_gram)..=(start + max_gram).min(chars) {
                results.push(token[bounds[start]..bounds[end]].to_string());
                // A long token has very many grams; let a cancel stop it.
                crate::utils::check_for_interrupts_every(results.len());
            }
        }
        // A token whose length is in range is already one of the n-grams.
        if self.config.preserve_original && !(min_gram..=max_gram).contains(&chars) {
            results.push(token);
        }
        results
    }
}

impl Ngram {
    pub fn new(config: NgramConfig) -> Result<Self, validator::ValidationErrors> {
        config.validate()?;
        Ok(Ngram { config })
    }
}

#[cfg(test)]
mod tests {
    use super::{Ngram, NgramConfig, TokenFilter};

    fn ngram(min_gram: usize, max_gram: usize, preserve_original: bool) -> Ngram {
        Ngram::new(NgramConfig {
            max_gram,
            min_gram,
            preserve_original,
        })
        .unwrap()
    }

    #[test]
    fn ascii_output_is_unchanged() {
        let grams = ngram(1, 2, false).apply("abc".into());
        assert_eq!(grams, ["a", "ab", "b", "bc", "c"]);
        let grams = ngram(2, 3, false).apply("abcd".into());
        assert_eq!(grams, ["ab", "abc", "bc", "bcd", "cd"]);
    }

    #[test]
    fn counts_characters_not_bytes() {
        assert_eq!(
            ngram(1, 2, false).apply("中文".into()),
            ["中", "中文", "文"]
        );
        assert_eq!(ngram(2, 2, false).apply("café".into()), ["ca", "af", "fé"]);
        assert_eq!(ngram(1, 1, false).apply("🦀x".into()), ["🦀", "x"]);
        // A combining mark is a character of its own.
        assert_eq!(
            ngram(1, 1, false).apply("e\u{301}".into()),
            ["e", "\u{301}"]
        );
    }

    #[test]
    fn short_and_empty_tokens() {
        assert!(ngram(2, 3, false).apply("a".into()).is_empty());
        assert!(ngram(2, 3, false).apply("中".into()).is_empty());
        assert!(ngram(1, 2, false).apply(String::new()).is_empty());
        assert_eq!(ngram(2, 3, true).apply("中".into()), ["中"]);
        assert_eq!(ngram(1, 2, true).apply(String::new()), [""]);
    }

    #[test]
    fn checks_for_interrupts_every_1024_grams() {
        use crate::utils::interrupt_tests::count_interrupt_checks;
        let (grams, checks) =
            count_interrupt_checks(|| ngram(1, 1, false).apply("a".repeat(10_000)));
        assert_eq!(grams.len(), 10_000);
        assert_eq!(checks, 10_000 / 1024);
    }

    #[test]
    fn preserve_original_once() {
        assert_eq!(ngram(1, 2, true).apply("中文".into()), ["中", "中文", "文"]);
        assert_eq!(ngram(1, 1, true).apply("中文".into()), ["中", "文", "中文"]);
        assert_eq!(ngram(255, 255, true).apply("ab".into()), ["ab"]);
    }
}
