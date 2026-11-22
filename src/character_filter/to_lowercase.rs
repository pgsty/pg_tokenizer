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
        const CHUNK_BYTES: usize = 1024;
        let mut uppercase = false;
        let ascii = text.as_bytes().chunks(CHUNK_BYTES).all(|chunk| {
            crate::utils::check_for_interrupts();
            uppercase = uppercase || chunk.iter().any(u8::is_ascii_uppercase);
            chunk.is_ascii()
        });
        if ascii {
            crate::budget::bytes(text.len());
            if uppercase {
                let mut lowercase = String::with_capacity(text.len());
                for start in (0..text.len()).step_by(CHUNK_BYTES) {
                    crate::utils::check_for_interrupts();
                    let end = (start + CHUNK_BYTES).min(text.len());
                    lowercase.push_str(&text[start..end]);
                    lowercase[start..].make_ascii_lowercase();
                }
                *text = Cow::Owned(lowercase);
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
    use crate::utils::interrupt_tests::count_interrupt_checks;

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

    #[test]
    fn ascii_chunks_check_interrupts_and_preserve_unicode_boundaries() {
        for len in [0usize, 1, 1023, 1024, 1025, 4097] {
            let source = "a".repeat(len);
            let mut text = Cow::Borrowed(source.as_str());
            let (_, checks) = count_interrupt_checks(|| ToLowercase.apply(&mut text));
            assert!(matches!(text, Cow::Borrowed(_)));
            assert_eq!(checks, len.div_ceil(1024));

            let source = source.to_ascii_uppercase();
            let mut text = Cow::Borrowed(source.as_str());
            let (_, checks) = count_interrupt_checks(|| ToLowercase.apply(&mut text));
            assert_eq!(text, "a".repeat(len));
            assert_eq!(checks, 2 * len.div_ceil(1024));
        }
        let source = format!("{}ΟΣ", "A".repeat(1023));
        let mut text = Cow::Borrowed(source.as_str());
        ToLowercase.apply(&mut text);
        assert_eq!(text, source.to_lowercase());
    }
}
