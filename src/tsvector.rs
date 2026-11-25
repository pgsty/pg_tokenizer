// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::collections::HashMap;
use std::fmt::Write;
use std::ptr::NonNull;

use pgrx::pg_sys;

// PostgreSQL rejects tsvector lexemes of 2 KB (MAXSTRLEN) or longer.
const MAX_LEXEME_BYTES: usize = 2046;
// A tsvector keeps at most 256 positions per lexeme (MAXNUMPOS), and positions
// are at most 16383 (MAXENTRYPOS - 1); larger positions are clamped.
const MAX_POSITIONS: usize = 256;
const MAX_POSITION: u16 = 16383;
// Maximum byte length of the lexeme/position data area (MAXSTRPOS = (1 << 20) - 1).
const MAX_STR_POS: usize = (1 << 20) - 1;
const DATA_HDR_SIZE: usize = 8;
const WORD_ENTRY_SIZE: usize = std::mem::size_of::<pg_sys::WordEntry>();

const _: () =
    assert!(std::mem::size_of::<pg_sys::TSVectorData>() == DATA_HDR_SIZE && WORD_ENTRY_SIZE == 4);

/// Owned pointer to a freshly `palloc0`-allocated PostgreSQL `TSVectorData` varlena.
pub struct TsVectorOutput(NonNull<pg_sys::TSVectorData>);

unsafe impl pgrx::callconv::BoxRet for TsVectorOutput {
    unsafe fn box_into<'fcx>(
        self,
        fcinfo: &mut pgrx::callconv::FcInfo<'fcx>,
    ) -> pgrx::datum::Datum<'fcx> {
        unsafe { fcinfo.return_raw_datum(pg_sys::Datum::from(self.0.as_ptr())) }
    }
}

impl pgrx::IntoDatum for TsVectorOutput {
    fn into_datum(self) -> Option<pg_sys::Datum> {
        Some(pg_sys::Datum::from(self.0.as_ptr()))
    }

    fn type_oid() -> pg_sys::Oid {
        pg_sys::TSVECTOROID
    }
}

pgrx::impl_sql_translatable!(TsVectorOutput, "pg_catalog.tsvector");

#[derive(Debug, Clone, PartialEq, Eq)]
enum PosList {
    One(u16),
    Many(Vec<u16>),
}

impl PosList {
    #[inline]
    fn new(pos: u16) -> Self {
        Self::One(pos)
    }

    #[inline]
    fn push(&mut self, pos: u16) {
        // Positions only grow, so each list stays sorted; after clamping,
        // only 16383 can repeat.
        match self {
            PosList::One(first) => {
                if *first != MAX_POSITION {
                    let mut list = Vec::with_capacity(4);
                    list.push(*first);
                    list.push(pos);
                    *self = PosList::Many(list);
                }
            }
            PosList::Many(list) => {
                if list.len() < MAX_POSITIONS && list.last() != Some(&MAX_POSITION) {
                    list.push(pos);
                }
            }
        }
    }

    #[inline]
    fn as_slice(&self) -> &[u16] {
        match self {
            PosList::One(pos) => std::slice::from_ref(pos),
            PosList::Many(list) => list.as_slice(),
        }
    }
}

fn compute_tsvector_layout<'a>(entries: &[(&'a [u8], &'a [u16])]) -> Result<(usize, usize), usize> {
    let mut lenstr = 0usize;
    for (i, (lexeme, positions)) in entries.iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        debug_assert!(!lexeme.is_empty() && lexeme.len() <= MAX_LEXEME_BYTES);
        debug_assert!(!positions.is_empty() && positions.len() <= MAX_POSITIONS);
        lenstr = lenstr.saturating_add(lexeme.len());
        // SHORTALIGN: position vector begins on a 2-byte boundary in the data area.
        lenstr = (lenstr + 1) & !1;
        lenstr = lenstr.saturating_add(2 + 2 * positions.len());
        if lenstr > MAX_STR_POS {
            return Err(lenstr);
        }
    }
    let total_size = DATA_HDR_SIZE + entries.len() * WORD_ENTRY_SIZE + lenstr;
    Ok((total_size, lenstr))
}

/// Write a PostgreSQL `TSVectorData` body (everything except `vl_len_`) into a
/// zero-initialized buffer of exact `total_size` bytes. Entries MUST be sorted
/// strictly ascending by unsigned byte order (`tsCompareString`).
fn encode_tsvector_body(entries: &[(&[u8], &[u16])], dst: &mut [u8]) {
    let nentries = entries.len();
    dst[4..8].copy_from_slice(&(nentries as i32).to_ne_bytes());

    let data_base = DATA_HDR_SIZE + nentries * WORD_ENTRY_SIZE;
    let mut data_off = 0usize;

    for (idx, (lexeme, positions)) in entries.iter().enumerate() {
        crate::utils::check_for_interrupts_every(idx);
        let entry_pos = data_off;

        let bits = pg_sys::WordEntry::new_bitfield_1(1, lexeme.len() as u32, entry_pos as u32);
        let entry = pg_sys::WordEntry {
            _bindgen_align: [],
            _bitfield_1: bits,
        };
        let entry_off = DATA_HDR_SIZE + idx * WORD_ENTRY_SIZE;
        // SAFETY: WordEntry is 4 initialized bytes with no padding.
        let entry_bytes = unsafe {
            std::slice::from_raw_parts(std::ptr::addr_of!(entry).cast::<u8>(), WORD_ENTRY_SIZE)
        };
        dst[entry_off..entry_off + WORD_ENTRY_SIZE].copy_from_slice(entry_bytes);

        let lex_start = data_base + data_off;
        dst[lex_start..lex_start + lexeme.len()].copy_from_slice(lexeme);
        data_off += lexeme.len();

        // Advance to next 2-byte boundary; dst is zero-initialized so padding is 0.
        data_off = (data_off + 1) & !1;

        let pos_start = data_base + data_off;
        dst[pos_start..pos_start + 2].copy_from_slice(&(positions.len() as u16).to_ne_bytes());
        data_off += 2;

        for &pos in *positions {
            let p = data_base + data_off;
            // Weight D (default) has top 2 bits = 0, so WordEntryPos == pos.
            dst[p..p + 2].copy_from_slice(&pos.to_ne_bytes());
            data_off += 2;
        }
    }
    debug_assert_eq!(data_base + data_off, dst.len());
}

fn build_tsvector_datum(entries: &[(&[u8], &[u16])]) -> TsVectorOutput {
    let (total_size, _) = compute_tsvector_layout(entries).unwrap_or_else(|lenstr| {
        crate::utils::raise_error(
            pg_sys::errcodes::PgSqlErrorCode::ERRCODE_PROGRAM_LIMIT_EXCEEDED,
            format!("string is too long for tsvector ({lenstr} bytes, max {MAX_STR_POS} bytes)"),
            None,
            None,
        )
    });

    unsafe {
        let raw = pg_sys::palloc0(total_size).cast::<u8>();
        let buf = std::slice::from_raw_parts_mut(raw, total_size);
        encode_tsvector_body(entries, buf);
        pg_sys::SET_VARSIZE_4B!(raw.cast::<pg_sys::varlena>(), total_size as i32).get();
        TsVectorOutput(NonNull::new_unchecked(raw.cast::<pg_sys::TSVectorData>()))
    }
}

fn collect_token_entries(tokens: Vec<String>) -> Vec<(String, PosList)> {
    let mut map: HashMap<String, PosList> = HashMap::new();
    let mut position = 0u16;
    for (i, token) in tokens.into_iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        // Like to_tsvector, skip words PostgreSQL cannot store as a lexeme.
        if token.is_empty() || token.len() > MAX_LEXEME_BYTES {
            continue;
        }
        position = position.saturating_add(1).min(MAX_POSITION);
        if let Some(positions) = map.get_mut(token.as_str()) {
            positions.push(position);
        } else {
            map.insert(token, PosList::new(position));
        }
    }
    let mut sorted: Vec<(String, PosList)> = map.into_iter().collect();
    // PostgreSQL tsCompareString compares lexeme bytes unsigned.
    sorted.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    sorted
}

#[inline]
fn push_i32_decimal(value: i32, dst: &mut Vec<u8>) -> (usize, usize) {
    let start = dst.len();
    let mut tmp = [0u8; 11];
    let mut idx = tmp.len();
    let negative = value < 0;
    let mut mag = value.unsigned_abs();
    if mag == 0 {
        idx -= 1;
        tmp[idx] = b'0';
    } else {
        while mag > 0 {
            idx -= 1;
            tmp[idx] = b'0' + (mag % 10) as u8;
            mag /= 10;
        }
    }
    if negative {
        idx -= 1;
        tmp[idx] = b'-';
    }
    dst.extend_from_slice(&tmp[idx..]);
    (start, dst.len() - start)
}

fn collect_id_entries(ids: &[i32]) -> (Vec<u8>, Vec<(usize, usize, PosList)>) {
    let mut map: HashMap<i32, PosList> = HashMap::new();
    let mut position = 0u16;
    for (i, &id) in ids.iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        position = position.saturating_add(1).min(MAX_POSITION);
        map.entry(id)
            .and_modify(|positions| positions.push(position))
            .or_insert_with(|| PosList::new(position));
    }
    let mut lexemes = Vec::with_capacity(map.len() * 6);
    let mut entries: Vec<(usize, usize, PosList)> = Vec::with_capacity(map.len());
    for (i, (id, positions)) in map.into_iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        let (off, len) = push_i32_decimal(id, &mut lexemes);
        entries.push((off, len, positions));
    }
    entries.sort_unstable_by(|a, b| lexemes[a.0..a.0 + a.1].cmp(&lexemes[b.0..b.0 + b.1]));
    (lexemes, entries)
}

#[pgrx::pg_extern(stable, parallel_safe)]
pub fn analyze_to_tsvector(text: &str, text_analyzer_name: &str) -> TsVectorOutput {
    let tokens = crate::text_analyzer::get_text_analyzer(text_analyzer_name).apply(text);
    let sorted = collect_token_entries(tokens);
    let views: Vec<(&[u8], &[u16])> = sorted
        .iter()
        .map(|(lex, pos)| (lex.as_bytes(), pos.as_slice()))
        .collect();
    build_tsvector_datum(&views)
}

#[pgrx::pg_extern(stable, parallel_safe)]
pub fn tokenize_to_tsvector(text: &str, tokenizer_name: &str) -> TsVectorOutput {
    let tokenizer = crate::tokenizer::get_tokenizer(tokenizer_name);
    let ids = tokenizer.tokenize_i32(text, tokenizer_name);
    let (lexemes, sorted) = collect_id_entries(&ids);
    let views: Vec<(&[u8], &[u16])> = sorted
        .iter()
        .map(|(off, len, pos)| (&lexemes[*off..*off + *len], pos.as_slice()))
        .collect();
    build_tsvector_datum(&views)
}

// Retained for backward compatibility with un-upgraded 1.0–1.2 SQL wrappers
// and byte-for-byte parity checks against PostgreSQL's tsvectorin parser.
#[pgrx::pg_extern(immutable, parallel_safe)]
fn _tokens_to_tsvector_text(tokens: Vec<String>) -> String {
    let mut lexemes: Vec<(String, Vec<u16>)> = Vec::new();
    let mut slots: HashMap<String, usize> = HashMap::new();
    let mut position = 0u16;
    let mut estimated_bytes = 0usize;
    for (i, token) in tokens.into_iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        // Like to_tsvector, skip words PostgreSQL cannot store as a lexeme.
        if token.is_empty() || token.len() > MAX_LEXEME_BYTES {
            continue;
        }
        position = position.saturating_add(1).min(MAX_POSITION);
        let slot = match slots.get(&token) {
            Some(&slot) => slot,
            None => {
                estimated_bytes = estimated_bytes.saturating_add(token.len() + 4);
                slots.insert(token.clone(), lexemes.len());
                lexemes.push((token, Vec::new()));
                lexemes.len() - 1
            }
        };
        // Positions only grow, so each list stays sorted; after clamping,
        // only 16383 can repeat.
        let positions = &mut lexemes[slot].1;
        if positions.len() < MAX_POSITIONS && positions.last() != Some(&MAX_POSITION) {
            positions.push(position);
            estimated_bytes = estimated_bytes.saturating_add(6);
        }
    }
    let mut result = String::with_capacity(estimated_bytes);
    for (lexeme, positions) in lexemes {
        result.push('\'');
        for ch in lexeme.chars() {
            if matches!(ch, '\'' | '\\') {
                result.push(ch);
            }
            result.push(ch);
        }
        result.push_str("':");
        for (i, position) in positions.iter().enumerate() {
            if i > 0 {
                result.push(',');
            }
            write!(result, "{position}").unwrap();
        }
        result.push(' ');
    }
    result
}

// Retained for backward compatibility with un-upgraded 1.0–1.2 SQL wrappers.
#[pgrx::pg_extern(stable, parallel_safe)]
fn _analyze_to_tsvector_text(text: &str, text_analyzer_name: &str) -> String {
    _tokens_to_tsvector_text(
        crate::text_analyzer::get_text_analyzer(text_analyzer_name).apply(text),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        _tokens_to_tsvector_text, collect_id_entries, collect_token_entries,
        compute_tsvector_layout, encode_tsvector_body, DATA_HDR_SIZE, MAX_STR_POS, WORD_ENTRY_SIZE,
    };
    use pgrx::pg_sys;

    fn words(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn positions(range: std::ops::RangeInclusive<usize>) -> String {
        range.map(|p| p.to_string()).collect::<Vec<_>>().join(",")
    }

    #[test]
    fn preserves_lexemes_and_output_positions() {
        assert_eq!(
            _tokens_to_tsvector_text(words(&["a b", "'\\", "中文", "", "a b"])),
            "'a b':1,4 '''\\\\':2 '中文':3 "
        );
        assert_eq!(_tokens_to_tsvector_text(vec![]), "");
        assert_eq!(_tokens_to_tsvector_text(words(&["", ""])), "");
    }

    #[test]
    fn skipped_tokens_leave_no_position_gaps() {
        let long = "x".repeat(2047);
        assert_eq!(
            _tokens_to_tsvector_text(vec!["a".into(), long, "".into(), "b".into(), "a".into()]),
            "'a':1,3 'b':2 "
        );
        let longest = "y".repeat(2046);
        assert_eq!(
            _tokens_to_tsvector_text(vec![longest.clone()]),
            format!("'{longest}':1 ")
        );
    }

    #[test]
    fn keeps_the_first_256_positions_of_a_lexeme() {
        for (repeat, kept) in [(255, 255), (256, 256), (257, 256)] {
            let mut tokens = vec!["x".to_string(); repeat];
            tokens.push("y".into());
            assert_eq!(
                _tokens_to_tsvector_text(tokens),
                format!("'x':{} 'y':{} ", positions(1..=kept), repeat + 1)
            );
        }
    }

    #[test]
    fn clamps_postgresql_positions() {
        let mut tokens = vec!["x".to_string(); 16384];
        tokens[16381] = "a".into(); // position 16382
        tokens[16382] = "b".into(); // position 16383
        tokens[16383] = "a".into(); // position 16384, clamped to 16383
        tokens.push("b".into());
        tokens.push("late".into());
        assert_eq!(
            _tokens_to_tsvector_text(tokens),
            format!(
                "'x':{} 'a':16382,16383 'b':16383 'late':16383 ",
                positions(1..=256)
            )
        );
    }

    #[test]
    fn checks_for_interrupts_every_1024_tokens() {
        use crate::utils::interrupt_tests::count_interrupt_checks;
        let (_, checks) =
            count_interrupt_checks(|| _tokens_to_tsvector_text(vec!["x".to_string(); 3000]));
        assert_eq!(checks, 3); // tokens 0, 1024 and 2048
    }

    #[test]
    fn repeated_tokens_stay_small() {
        let result = _tokens_to_tsvector_text(vec!["the".to_string(); 400_000]);
        assert_eq!(result, format!("'the':{} ", positions(1..=256)));
    }

    #[test]
    fn binary_encoder_matches_postgresql_layout() {
        let sorted = collect_token_entries(words(&["b", "aa", "'\\", "中文", "", "b"]));
        let views: Vec<(&[u8], &[u16])> = sorted
            .iter()
            .map(|(l, p)| (l.as_bytes(), p.as_slice()))
            .collect();
        let (total, _lenstr) = compute_tsvector_layout(&views).unwrap();
        let mut buf = vec![0u8; total];
        encode_tsvector_body(&views, &mut buf);

        let size = i32::from_ne_bytes(buf[4..8].try_into().unwrap()) as usize;
        assert_eq!(size, 4);

        let data_base = DATA_HDR_SIZE + size * WORD_ENTRY_SIZE;
        let mut decoded = Vec::new();
        for idx in 0..size {
            let off = DATA_HDR_SIZE + idx * WORD_ENTRY_SIZE;
            let entry = unsafe {
                std::ptr::read_unaligned(
                    buf[off..off + WORD_ENTRY_SIZE]
                        .as_ptr()
                        .cast::<pg_sys::WordEntry>(),
                )
            };
            assert_eq!(entry.haspos(), 1);
            let pos = entry.pos() as usize;
            let len = entry.len() as usize;
            let lex = &buf[data_base + pos..data_base + pos + len];
            let p_off = data_base + ((pos + len + 1) & !1);
            if (pos + len) % 2 == 1 {
                assert_eq!(buf[data_base + pos + len], 0);
            }
            let npos = u16::from_ne_bytes(buf[p_off..p_off + 2].try_into().unwrap()) as usize;
            let mut plist = Vec::with_capacity(npos);
            for j in 0..npos {
                let s = p_off + 2 + 2 * j;
                plist.push(u16::from_ne_bytes(buf[s..s + 2].try_into().unwrap()));
            }
            decoded.push((std::str::from_utf8(lex).unwrap().to_string(), plist));
        }
        assert_eq!(
            decoded,
            vec![
                ("'\\".into(), vec![3]),
                ("aa".into(), vec![2]),
                ("b".into(), vec![1, 5]),
                ("中文".into(), vec![4]),
            ]
        );
    }

    #[test]
    fn id_entries_sort_by_ascii_bytes_and_clamp() {
        let (lexemes, entries) = collect_id_entries(&[10, 2, 0, 10, i32::MAX]);
        let decoded: Vec<(&str, &[u16])> = entries
            .iter()
            .map(|(off, len, pos)| {
                (
                    std::str::from_utf8(&lexemes[*off..*off + *len]).unwrap(),
                    pos.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            decoded,
            vec![
                ("0", &[3][..]),
                ("10", &[1, 4][..]),
                ("2", &[2][..]),
                ("2147483647", &[5][..]),
            ]
        );
    }

    #[test]
    fn binary_layout_rejects_maxstrpos_overflow() {
        // 512 entries of 2046 bytes + 4 bytes of pos = 512 * 2050 = 1_049_600 > 1_048_575.
        let lex = vec![b'a'; 2046];
        let pos = [1u16];
        let entries: Vec<(&[u8], &[u16])> = vec![(&lex[..], &pos[..]); 512];
        let err = compute_tsvector_layout(&entries).unwrap_err();
        assert!(err > MAX_STR_POS);
    }
}
