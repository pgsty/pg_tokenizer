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

use crate::utils::check_for_interrupts;

pub trait TokenFilter {
    fn apply(&self, token: String) -> Vec<String>;

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<String> {
        let mut budget = crate::budget::Output::default();
        let mut results = Vec::with_capacity(tokens.len());
        for token in tokens {
            check_for_interrupts();
            let output = self.apply(token);
            for token in &output {
                budget.text(token.len());
            }
            results.extend(output);
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
    use super::TokenFilter;
    use crate::utils::interrupt_tests::count_interrupt_checks;

    #[test]
    fn default_batch_checks_for_interrupts_per_token() {
        let tokens = vec!["a".to_string(); 3000];
        let (result, checks) = count_interrupt_checks(|| SkipNonAlphanumeric.apply_batch(tokens));
        assert_eq!(result.len(), 3000);
        assert_eq!(checks, 3000);
    }
}
