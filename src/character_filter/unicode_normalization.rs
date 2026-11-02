// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.

#![allow(clippy::upper_case_acronyms)]

use std::{borrow::Cow, sync::Arc};

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
        *text = Cow::Owned(text.nfc().collect::<String>());
    }
}

impl CharacterFilter for NFD {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(text.nfd().collect::<String>());
    }
}

impl CharacterFilter for NFKC {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(text.nfkc().collect::<String>());
    }
}

impl CharacterFilter for NFKD {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(text.nfkd().collect::<String>());
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
