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

fn is_ascii_within_limit(text: &str, limits: crate::budget::Limits) -> bool {
    const CHUNK_BYTES: usize = 1024;
    let ascii = text.as_bytes().chunks(CHUNK_BYTES).all(|chunk| {
        crate::utils::check_for_interrupts();
        chunk.is_ascii()
    });
    if ascii {
        limits.check_bytes(text.len());
    }
    ascii
}

impl CharacterFilter for NFC {
    fn apply(&self, text: &mut Cow<str>) {
        self.apply_with_limits(text, crate::budget::Limits::current());
    }

    fn apply_with_limits(&self, text: &mut Cow<str>, limits: crate::budget::Limits) {
        if is_ascii_within_limit(text.as_ref(), limits) {
            return;
        }
        *text = Cow::Owned(crate::budget::characters_with_capacity(
            text.nfc(),
            text.len(),
            limits,
        ));
    }
}

impl CharacterFilter for NFD {
    fn apply(&self, text: &mut Cow<str>) {
        self.apply_with_limits(text, crate::budget::Limits::current());
    }

    fn apply_with_limits(&self, text: &mut Cow<str>, limits: crate::budget::Limits) {
        if is_ascii_within_limit(text.as_ref(), limits) {
            return;
        }
        *text = Cow::Owned(crate::budget::characters_with_capacity(
            text.nfd(),
            text.len(),
            limits,
        ));
    }
}

impl CharacterFilter for NFKC {
    fn apply(&self, text: &mut Cow<str>) {
        self.apply_with_limits(text, crate::budget::Limits::current());
    }

    fn apply_with_limits(&self, text: &mut Cow<str>, limits: crate::budget::Limits) {
        if is_ascii_within_limit(text.as_ref(), limits) {
            return;
        }
        *text = Cow::Owned(crate::budget::characters_with_capacity(
            text.nfkc(),
            text.len(),
            limits,
        ));
    }
}

impl CharacterFilter for NFKD {
    fn apply(&self, text: &mut Cow<str>) {
        self.apply_with_limits(text, crate::budget::Limits::current());
    }

    fn apply_with_limits(&self, text: &mut Cow<str>, limits: crate::budget::Limits) {
        if is_ascii_within_limit(text.as_ref(), limits) {
            return;
        }
        *text = Cow::Owned(crate::budget::characters_with_capacity(
            text.nfkd(),
            text.len(),
            limits,
        ));
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

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{CharacterFilter, NFC, NFD, NFKC, NFKD};
    use crate::utils::interrupt_tests::count_interrupt_checks;

    #[test]
    fn ascii_avoids_allocation_for_all_forms() {
        let source = "PostgreSQL 18 BM25 search 123!";
        for filter in [&NFC as &dyn CharacterFilter, &NFD, &NFKC, &NFKD] {
            let mut text = Cow::Borrowed(source);
            let (_, checks) = count_interrupt_checks(|| filter.apply(&mut text));
            assert!(matches!(text, Cow::Borrowed(s) if s == source));
            assert_eq!(checks, 1);
        }
    }

    #[test]
    fn non_ascii_normalizes_expected_forms() {
        let mut nfc = Cow::Borrowed("cafe\u{0301}");
        NFC.apply(&mut nfc);
        assert_eq!(nfc, "café");

        let mut nfd = Cow::Borrowed("café");
        NFD.apply(&mut nfd);
        assert_eq!(nfd, "cafe\u{0301}");

        let mut nfkc = Cow::Borrowed("ﬁ ＡＢＣ");
        NFKC.apply(&mut nfkc);
        assert_eq!(nfkc, "fi ABC");

        let mut nfkd = Cow::Borrowed("ﬁ café");
        NFKD.apply(&mut nfkd);
        assert_eq!(nfkd, "fi cafe\u{0301}");
    }
}
