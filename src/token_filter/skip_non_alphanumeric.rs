// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use super::TokenFilter;

pub struct SkipNonAlphanumeric;

impl TokenFilter for SkipNonAlphanumeric {
    fn apply(&self, token: String) -> Vec<String> {
        if token.chars().any(char::is_alphanumeric) {
            vec![token]
        } else {
            vec![]
        }
    }
}
