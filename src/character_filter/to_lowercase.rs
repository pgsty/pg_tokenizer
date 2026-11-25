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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SigmaNeighbor {
    Cased,
    Ignorable,
    Other,
}

#[cold]
fn classify_sigma_neighbor(ch: char) -> SigmaNeighbor {
    if ch.is_ascii_alphabetic() {
        return SigmaNeighbor::Cased;
    }
    if ch.is_ascii_whitespace() || ch.is_ascii_digit() {
        return SigmaNeighbor::Other;
    }
    // Delegate to std::str::to_lowercase() on a 2/3-char probe so Cased and
    // Case_Ignorable classification matches Rust's exact Unicode version.
    let mut probe = [0u8; 10];
    let mut len = ch.encode_utf8(&mut probe).len();
    len += 'Σ'.encode_utf8(&mut probe[len..]).len();
    if std::str::from_utf8(&probe[..len])
        .unwrap()
        .to_lowercase()
        .ends_with('ς')
    {
        return SigmaNeighbor::Cased;
    }
    probe[0] = b'a';
    let mut len = 1;
    len += ch.encode_utf8(&mut probe[len..]).len();
    len += 'Σ'.encode_utf8(&mut probe[len..]).len();
    if std::str::from_utf8(&probe[..len])
        .unwrap()
        .to_lowercase()
        .ends_with('ς')
    {
        SigmaNeighbor::Ignorable
    } else {
        SigmaNeighbor::Other
    }
}

#[cold]
fn lowercase_greek_sigma(text: &str, idx: usize) -> char {
    let mut preceded_by_cased = false;
    for (i, ch) in text[..idx].chars().rev().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        match classify_sigma_neighbor(ch) {
            SigmaNeighbor::Cased => {
                preceded_by_cased = true;
                break;
            }
            SigmaNeighbor::Ignorable => continue,
            SigmaNeighbor::Other => break,
        }
    }
    if !preceded_by_cased {
        return 'σ';
    }
    let after = idx + 'Σ'.len_utf8();
    for (i, ch) in text[after..].chars().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        match classify_sigma_neighbor(ch) {
            SigmaNeighbor::Cased => return 'σ',
            SigmaNeighbor::Ignorable => continue,
            SigmaNeighbor::Other => break,
        }
    }
    'ς'
}

impl CharacterFilter for ToLowercase {
    fn apply(&self, text: &mut Cow<str>) {
        self.apply_with_limits(text, crate::budget::Limits::current());
    }

    fn apply_with_limits(&self, text: &mut Cow<str>, limits: crate::budget::Limits) {
        const CHUNK_BYTES: usize = 1024;
        let mut uppercase = false;
        let ascii = text.as_bytes().chunks(CHUNK_BYTES).all(|chunk| {
            crate::utils::check_for_interrupts();
            uppercase = uppercase || chunk.iter().any(u8::is_ascii_uppercase);
            chunk.is_ascii()
        });
        if ascii {
            limits.check_bytes(text.len());
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
        // Single-pass Unicode lowercase: avoid allocating when all characters
        // are already lowercase (e.g. CJK) while preserving final-sigma rules.
        let src = text.as_ref();
        let mut out: Option<String> = None;
        for (i, (idx, ch)) in src.char_indices().enumerate() {
            crate::utils::check_for_interrupts_every(i);
            if ch == 'Σ' {
                let buf = out.get_or_insert_with(|| {
                    let mut s = String::with_capacity(src.len().min(limits.output));
                    limits.check_bytes(idx);
                    s.push_str(&src[..idx]);
                    s
                });
                let mapped = lowercase_greek_sigma(src, idx);
                limits.check_bytes(buf.len().saturating_add(mapped.len_utf8()));
                buf.push(mapped);
                continue;
            }
            if ch.is_ascii() {
                let lower = ch.to_ascii_lowercase();
                if let Some(buf) = &mut out {
                    limits.check_bytes(buf.len().saturating_add(1));
                    buf.push(lower);
                } else if lower != ch {
                    let mut s = String::with_capacity(src.len().min(limits.output));
                    limits.check_bytes(idx.saturating_add(1));
                    s.push_str(&src[..idx]);
                    s.push(lower);
                    out = Some(s);
                }
                continue;
            }
            let mut lower = ch.to_lowercase();
            let first = lower.next().expect("to_lowercase yields at least one char");
            let second = lower.next();
            if let Some(buf) = &mut out {
                limits.check_bytes(buf.len().saturating_add(first.len_utf8()));
                buf.push(first);
                if let Some(c2) = second {
                    limits.check_bytes(buf.len().saturating_add(c2.len_utf8()));
                    buf.push(c2);
                    for extra in lower {
                        limits.check_bytes(buf.len().saturating_add(extra.len_utf8()));
                        buf.push(extra);
                    }
                }
            } else if first != ch || second.is_some() {
                let mut s = String::with_capacity(src.len().min(limits.output));
                limits.check_bytes(idx.saturating_add(first.len_utf8()));
                s.push_str(&src[..idx]);
                s.push(first);
                if let Some(c2) = second {
                    limits.check_bytes(s.len().saturating_add(c2.len_utf8()));
                    s.push(c2);
                    for extra in lower {
                        limits.check_bytes(s.len().saturating_add(extra.len_utf8()));
                        s.push(extra);
                    }
                }
                out = Some(s);
            }
        }
        if let Some(lowercase) = out {
            *text = Cow::Owned(lowercase);
        } else {
            limits.check_bytes(src.len());
        }
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

    #[test]
    fn unicode_single_pass_avoids_alloc_for_cjk_and_matches_stdlib() {
        let mut cjk = Cow::Borrowed("中文搜索与向量检索 123");
        ToLowercase.apply(&mut cjk);
        assert!(matches!(cjk, Cow::Borrowed("中文搜索与向量检索 123")));

        for sample in [
            "PostgreSQL 数据库 VectorChord",
            "ΌΔΥΣΣΕΎΣ",
            "Σ",
            "ΣΣ",
            "a'Σ.",
            "a'Σ.b",
            "a\u{0301}Σ\u{0301}",
            "a\u{02B0}Σ",
            "\u{02B0}Σ",
            "İSTANBUL GROẞ CAFÉ",
            "中文İΣ",
        ] {
            let mut text = Cow::Borrowed(sample);
            ToLowercase.apply(&mut text);
            assert_eq!(text.as_ref(), sample.to_lowercase(), "sample={sample:?}");
        }
    }
}
