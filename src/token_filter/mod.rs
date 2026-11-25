// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

mod ngram;
mod pg_dict;
mod skip_non_alphanumeric;
mod stemmer;
mod stopwords;
mod synonym;

use std::sync::Arc;

use ngram::{Ngram, NgramConfig};
use pg_dict::PgDictTokenFilter;
use serde::{Deserialize, Serialize};
use skip_non_alphanumeric::SkipNonAlphanumeric;
use stemmer::{StemmerKind, StemmerTokenFilter};

use crate::utils::check_for_interrupts_every;

pub trait TokenFilter {
    fn apply(&self, token: String) -> Vec<String>;

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<String> {
        let mut budget = crate::budget::Output::default();
        let mut results = Vec::with_capacity(tokens.len());
        for (i, token) in tokens.into_iter().enumerate() {
            check_for_interrupts_every(i);
            let output = self.apply(token);
            for token in &output {
                budget.text(token.len());
            }
            results.extend(output);
        }
        results
    }

    #[doc(hidden)]
    fn apply_batch_with_limits(
        &self,
        tokens: Vec<String>,
        limits: crate::budget::Limits,
    ) -> Vec<String> {
        let results = self.apply_batch(tokens);
        let mut budget = crate::budget::Output::new(limits);
        for (i, token) in results.iter().enumerate() {
            check_for_interrupts_every(i);
            budget.text(token.len());
        }
        results
    }
}
pub type TokenFilterPtr = Arc<dyn TokenFilter + Sync + Send>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum TokenFilterConfig {
    SkipNonAlphanumeric,
    Stemmer(StemmerKind),
    Stopwords(String),
    PgDict(String),
    Synonym(String),
    #[serde(rename = "ngram")]
    NGram(NgramConfig),
}

pub(crate) fn qualify_dictionaries(filters: &mut [TokenFilterConfig]) {
    for filter in filters {
        if let TokenFilterConfig::PgDict(name) = filter {
            *name = pg_dict::qualify(name);
        }
    }
}

pub fn get_token_filter(config: TokenFilterConfig) -> TokenFilterPtr {
    match config {
        TokenFilterConfig::SkipNonAlphanumeric => Arc::new(SkipNonAlphanumeric),
        TokenFilterConfig::Stemmer(kind) => Arc::new(StemmerTokenFilter::new(kind)),
        TokenFilterConfig::Stopwords(name) => stopwords::get_stopwords_token_filter(&name),
        TokenFilterConfig::PgDict(name) => Arc::new(PgDictTokenFilter::new(&name)),
        TokenFilterConfig::Synonym(name) => synonym::get_synonym_token_filter(&name),
        TokenFilterConfig::NGram(config) => Arc::new(Ngram::new(config).unwrap_or_else(|e| {
            crate::utils::raise_error(
                pgrx::pg_sys::errcodes::PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                "invalid ngram token filter configuration",
                Some(e.to_string()),
                None,
            )
        })),
    }
}

pub(crate) fn clear_caches() {
    stopwords::clear_cache();
    synonym::clear_cache();
}

#[cfg(test)]
mod tests {
    use super::skip_non_alphanumeric::SkipNonAlphanumeric;
    use super::stopwords::StopwordsTokenFilter;
    use super::synonym::SynonymTokenFilter;
    use super::TokenFilter;
    use crate::utils::interrupt_tests::count_interrupt_checks;

    struct IdentityFilter;
    impl TokenFilter for IdentityFilter {
        fn apply(&self, token: String) -> Vec<String> {
            vec![token]
        }
    }

    #[test]
    fn default_batch_checks_for_interrupts_every_1024_tokens() {
        let tokens = vec!["a".to_string(); 3000];
        let (result, checks) = count_interrupt_checks(|| IdentityFilter.apply_batch(tokens));
        assert_eq!(result.len(), 3000);
        assert_eq!(checks, 3000usize.div_ceil(1024));
    }

    #[test]
    fn in_place_filters_preserve_output_and_interrupt_checks() {
        let (kept, checks) = count_interrupt_checks(|| {
            SkipNonAlphanumeric.apply_batch(vec![
                "hello".into(),
                "---".into(),
                "世界".into(),
                "!".into(),
            ])
        });
        assert_eq!(kept, ["hello", "世界"]);
        assert_eq!(checks, 1);

        let stopwords = StopwordsTokenFilter::build("the\nand\n");
        let (kept, checks) = count_interrupt_checks(|| {
            stopwords.apply_batch(vec![
                "the".into(),
                "quick".into(),
                "and".into(),
                "fox".into(),
            ])
        });
        assert_eq!(kept, ["quick", "fox"]);
        assert_eq!(checks, 1);

        let synonyms = SynonymTokenFilter::build("pg postgres postgresql\n");
        let (mapped, checks) = count_interrupt_checks(|| {
            synonyms.apply_batch(vec!["postgresql".into(), "db".into(), "postgres".into()])
        });
        assert_eq!(mapped, ["pg", "db", "pg"]);
        assert_eq!(checks, 1);

        let many = vec!["hello".to_string(); 2500];
        let (kept, checks) = count_interrupt_checks(|| SkipNonAlphanumeric.apply_batch(many));
        assert_eq!(kept.len(), 2500);
        assert_eq!(checks, 2500usize.div_ceil(1024));
    }
}
