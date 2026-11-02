// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025-2026 TensorChord Inc.

use std::borrow::Cow;

use super::CharacterFilter;

pub struct ToLowercase;

impl CharacterFilter for ToLowercase {
    fn apply(&self, text: &mut Cow<str>) {
        *text = Cow::Owned(text.to_lowercase());
    }
}
