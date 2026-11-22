// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::borrow::Cow;

use super::CharacterFilter;

pub struct ToLowercase;

impl CharacterFilter for ToLowercase {
    fn apply(&self, text: &mut Cow<str>) {
        if text.is_ascii() {
            crate::utils::check_for_interrupts();
            // ASCII lowercasing preserves byte length and has no context-sensitive rules.
            crate::budget::bytes(text.len());
            if text.bytes().any(|b| b.is_ascii_uppercase()) {
                *text = Cow::Owned(text.to_ascii_lowercase());
            }
            return;
        }
        // Preserve str::to_lowercase's context-sensitive final sigma rule.
        let mut bytes = 0usize;
        for (i, ch) in text.chars().enumerate() {
            crate::utils::check_for_interrupts_every(i);
            bytes = bytes.saturating_add(ch.to_lowercase().map(char::len_utf8).sum::<usize>());
        }
        crate::budget::bytes(bytes);
        *text = Cow::Owned(text.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{CharacterFilter, ToLowercase};

    #[test]
    fn ascii_lowercase_avoids_allocation() {
        let mut text = Cow::Borrowed("already lower 123!");
        ToLowercase.apply(&mut text);
        assert!(matches!(text, Cow::Borrowed("already lower 123!")));
    }

    #[test]
    fn ascii_uppercase_and_unicode_final_sigma() {
        let mut ascii = Cow::Borrowed("Hello, PostgreSQL 19!");
        ToLowercase.apply(&mut ascii);
        assert_eq!(ascii, "hello, postgresql 19!");

        let mut greek = Cow::Borrowed("ΌΔΥΣΣΕΎΣ");
        ToLowercase.apply(&mut greek);
        assert_eq!(greek, "όδυσσεύς");
    }
}
