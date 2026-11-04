// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::fmt::Write;

// PostgreSQL rejects tsvector lexemes of 2 KB (MAXSTRLEN) or longer.
const MAX_LEXEME_BYTES: usize = 2046;

// Let PostgreSQL build and validate the native datum. Escaping the tsvector
// input syntax keeps punctuation, whitespace and subword markers intact.
#[pgrx::pg_extern(immutable, parallel_safe)]
fn _tokens_to_tsvector_text(tokens: Vec<String>) -> String {
    let mut result = String::new();
    let mut position = 0;
    for token in tokens {
        // Like to_tsvector, skip words PostgreSQL cannot store as a lexeme.
        if token.is_empty() || token.len() > MAX_LEXEME_BYTES {
            continue;
        }
        position += 1;
        result.push('\'');
        for ch in token.chars() {
            if matches!(ch, '\'' | '\\') {
                result.push(ch);
            }
            result.push(ch);
        }
        write!(result, "':{} ", position.min(16383)).unwrap();
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

    #[test]
    fn preserves_lexemes_and_output_positions() {
        assert_eq!(
            _tokens_to_tsvector_text(vec![
                "a b".into(),
                "'\\".into(),
                "中文".into(),
                "".into(),
                "a b".into()
            ]),
            "'a b':1 '''\\\\':2 '中文':3 'a b':4 "
        );
        assert_eq!(_tokens_to_tsvector_text(vec![]), "");
    }

    #[test]
    fn clamps_postgresql_positions() {
        let result = _tokens_to_tsvector_text(vec!["x".into(); 16384]);
        assert!(result.ends_with("'x':16383 'x':16383 "));
    }
}
