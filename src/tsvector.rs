// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::{collections::HashMap, fmt::Write};

// PostgreSQL rejects tsvector lexemes of 2 KB (MAXSTRLEN) or longer.
const MAX_LEXEME_BYTES: usize = 2046;
// A tsvector keeps at most 256 positions per lexeme (MAXNUMPOS), and positions
// are at most 16383 (MAXENTRYPOS - 1); larger positions are clamped.
const MAX_POSITIONS: usize = 256;
const MAX_POSITION: usize = 16383;

// Let PostgreSQL build and validate the native datum. Escaping the tsvector
// input syntax keeps punctuation, whitespace and subword markers intact.
//
// tsvectorin applies its 1 MB limit to the text it parses before it merges
// duplicate lexemes, so emit each lexeme once with the positions PostgreSQL
// would keep: the first 256, sorted and distinct, ending at the clamped 16383.
// Later tokens are still read, so a new lexeme after position 16383 is kept.
#[pgrx::pg_extern(immutable, parallel_safe)]
fn _tokens_to_tsvector_text(tokens: Vec<String>) -> String {
    let mut lexemes: Vec<(String, Vec<usize>)> = Vec::new();
    let mut slots: HashMap<String, usize> = HashMap::new();
    let mut position = 0;
    for (i, token) in tokens.into_iter().enumerate() {
        if i % 1024 == 0 {
            crate::utils::check_for_interrupts();
        }
        // Like to_tsvector, skip words PostgreSQL cannot store as a lexeme.
        if token.is_empty() || token.len() > MAX_LEXEME_BYTES {
            continue;
        }
        position = (position + 1).min(MAX_POSITION);
        let slot = match slots.get(&token) {
            Some(&slot) => slot,
            None => {
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
        }
    }
    let mut result = String::new();
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

// Keep catalog lookup STABLE all the way through the SQL wrapper, including
// cached plans. Analyzer output must not be folded into a plan-time constant.
#[pgrx::pg_extern(stable, parallel_safe)]
fn _analyze_to_tsvector_text(text: &str, text_analyzer_name: &str) -> String {
    _tokens_to_tsvector_text(
        crate::text_analyzer::get_text_analyzer(text_analyzer_name).apply(text),
    )
}

pgrx::extension_sql_file!(
    "sql/tsvector.sql",
    name = "tsvector_functions",
    requires = [
        _tokens_to_tsvector_text,
        _analyze_to_tsvector_text,
        tokenizer::tokenize
    ]
);

#[cfg(test)]
mod tests {
    use super::_tokens_to_tsvector_text;

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
    fn repeated_tokens_stay_small() {
        let result = _tokens_to_tsvector_text(vec!["the".to_string(); 400_000]);
        assert_eq!(result, format!("'the':{} ", positions(1..=256)));
    }
}
