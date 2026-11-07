// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

#![allow(clippy::upper_case_acronyms)]

use std::borrow::Cow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use super::{CharacterFilter, CharacterFilterPtr};

#[derive(Clone, Serialize, Deserialize)]
pub enum UnicodeNormalizationConfig {
    #[serde(rename = "nfc")]
    NFC,
    #[serde(rename = "nfd")]
    NFD,
    #[serde(rename = "nfkc")]
    NFKC,
    #[serde(rename = "nfkd")]
    NFKD,
}

pub struct NFC;
pub struct NFD;
pub struct NFKC;
pub struct NFKD;

impl CharacterFilter for NFC {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(crate::budget::characters(text.nfc()));
    }
}

impl CharacterFilter for NFD {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(crate::budget::characters(text.nfd()));
    }
}

impl CharacterFilter for NFKC {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(crate::budget::characters(text.nfkc()));
    }
}

impl CharacterFilter for NFKD {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(crate::budget::characters(text.nfkd()));
    }
}

pub fn get_unicode_normalization(config: UnicodeNormalizationConfig) -> CharacterFilterPtr {
    match config {
        UnicodeNormalizationConfig::NFC => Arc::new(NFC),
        UnicodeNormalizationConfig::NFD => Arc::new(NFD),
        UnicodeNormalizationConfig::NFKC => Arc::new(NFKC),
        UnicodeNormalizationConfig::NFKD => Arc::new(NFKD),
    }
}
