// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use unicode_segmentation::UnicodeSegmentation;

use super::PreTokenizer;

pub struct UnicodeSegmentationPretokenizer;

impl PreTokenizer for UnicodeSegmentationPretokenizer {
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        text.unicode_words().collect()
    }
}
