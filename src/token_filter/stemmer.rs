// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::borrow::Cow;

use rust_stemmers::Stemmer;
use serde::{Deserialize, Serialize};

use super::TokenFilter;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum StemmerKind {
    Arabic,
    Armenian,
    Basque,
    Catalan,
    Danish,
    Dutch,
    EnglishPorter,
    EnglishPorter2,
    Estonian,
    Finnish,
    French,
    German,
    Greek,
    Hindi,
    Hungarian,
    Indonesian,
    Irish,
    Italian,
    Lithuanian,
    Nepali,
    Norwegian,
    Portuguese,
    Romanian,
    Russian,
    Serbian,
    Spanish,
    Swedish,
    Tamil,
    Turkish,
    Yiddish,
}

impl From<StemmerKind> for rust_stemmers::Algorithm {
    fn from(kind: StemmerKind) -> Self {
        match kind {
            StemmerKind::Arabic => rust_stemmers::Algorithm::Arabic,
            StemmerKind::Armenian => rust_stemmers::Algorithm::Armenian,
            StemmerKind::Basque => rust_stemmers::Algorithm::Basque,
            StemmerKind::Catalan => rust_stemmers::Algorithm::Catalan,
            StemmerKind::Danish => rust_stemmers::Algorithm::Danish,
            StemmerKind::Dutch => rust_stemmers::Algorithm::Dutch,
            StemmerKind::EnglishPorter2 => rust_stemmers::Algorithm::English,
            StemmerKind::Estonian => rust_stemmers::Algorithm::Estonian,
            StemmerKind::Finnish => rust_stemmers::Algorithm::Finnish,
            StemmerKind::French => rust_stemmers::Algorithm::French,
            StemmerKind::German => rust_stemmers::Algorithm::German,
            StemmerKind::Greek => rust_stemmers::Algorithm::Greek,
            StemmerKind::Hindi => rust_stemmers::Algorithm::Hindi,
            StemmerKind::Hungarian => rust_stemmers::Algorithm::Hungarian,
            StemmerKind::Indonesian => rust_stemmers::Algorithm::Indonesian,
            StemmerKind::Irish => rust_stemmers::Algorithm::Irish,
            StemmerKind::Italian => rust_stemmers::Algorithm::Italian,
            StemmerKind::Lithuanian => rust_stemmers::Algorithm::Lithuanian,
            StemmerKind::Nepali => rust_stemmers::Algorithm::Nepali,
            StemmerKind::Norwegian => rust_stemmers::Algorithm::Norwegian,
            StemmerKind::EnglishPorter => rust_stemmers::Algorithm::Porter,
            StemmerKind::Portuguese => rust_stemmers::Algorithm::Portuguese,
            StemmerKind::Romanian => rust_stemmers::Algorithm::Romanian,
            StemmerKind::Russian => rust_stemmers::Algorithm::Russian,
            StemmerKind::Serbian => rust_stemmers::Algorithm::Serbian,
            StemmerKind::Spanish => rust_stemmers::Algorithm::Spanish,
            StemmerKind::Swedish => rust_stemmers::Algorithm::Swedish,
            StemmerKind::Tamil => rust_stemmers::Algorithm::Tamil,
            StemmerKind::Turkish => rust_stemmers::Algorithm::Turkish,
            StemmerKind::Yiddish => rust_stemmers::Algorithm::Yiddish,
        }
    }
}

pub struct StemmerTokenFilter {
    stemmer: Stemmer,
}

impl StemmerTokenFilter {
    pub fn new(kind: StemmerKind) -> Self {
        StemmerTokenFilter {
            stemmer: Stemmer::create(kind.into()),
        }
    }
}

impl TokenFilter for StemmerTokenFilter {
    fn apply(&self, token: String) -> Vec<String> {
        let stemmed = self.stemmer.stem(&token);
        vec![stemmed.into_owned()]
    }

    fn apply_batch(&self, mut tokens: Vec<String>) -> Vec<String> {
        for token in tokens.iter_mut() {
            crate::utils::check_for_interrupts();
            let stemmed = self.stemmer.stem(token);
            if let Cow::Owned(s) = stemmed {
                *token = s;
            }
        }
        tokens
    }
}
