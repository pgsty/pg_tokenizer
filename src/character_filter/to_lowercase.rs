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
