// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

mod to_lowercase;
mod unicode_normalization;

use std::borrow::Cow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use to_lowercase::ToLowercase;
use unicode_normalization::UnicodeNormalizationConfig;

pub trait CharacterFilter {
    fn apply(&self, text: &mut Cow<str>);
}
pub type CharacterFilterPtr = Arc<dyn CharacterFilter + Send + Sync>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum CharacterFilterConfig {
    ToLowercase,
    UnicodeNormalization(UnicodeNormalizationConfig),
}

pub fn get_character_filter(config: CharacterFilterConfig) -> CharacterFilterPtr {
    match config {
        CharacterFilterConfig::ToLowercase => Arc::new(ToLowercase),
        CharacterFilterConfig::UnicodeNormalization(config) => {
            unicode_normalization::get_unicode_normalization(config)
        }
    }
}
