// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::ffi::{CStr, CString};

use pgrx::IntoDatum;

use super::TokenFilter;

pub struct PgDictTokenFilter {
    name: String,
}

fn dictionary_oid(name: &str) -> pgrx::pg_sys::Oid {
    unsafe {
        pgrx::direct_function_call::<pgrx::pg_sys::Oid>(
            pgrx::pg_sys::regdictionaryin,
            &[CString::new(name).unwrap().as_c_str().into_datum()],
        )
    }
    .expect("regdictionary input returned NULL")
}

// Resolve under the creator's search_path, then persist a properly quoted name.
pub(super) fn qualify(name: &str) -> String {
    let oid = dictionary_oid(name);
    crate::utils::spi_get_one(
        "SELECT pg_catalog.format('%I.%I', n.nspname, d.dictname) \
         FROM pg_catalog.pg_ts_dict d JOIN pg_catalog.pg_namespace n ON n.oid = d.dictnamespace \
         WHERE d.oid = $1",
        &[oid.into()],
    )
    .expect("dictionary disappeared during resolution")
}

impl PgDictTokenFilter {
    pub fn new(name: &str) -> Self {
        let parts = unsafe {
            pgrx::direct_function_call::<Vec<String>>(
                pgrx::pg_sys::parse_ident,
                &[name.into_datum(), true.into_datum()],
            )
        }
        .expect("parse_ident returned NULL");
        // Before 1.1.0 names were stored unqualified. Keep a legacy name only
        // when it identifies exactly one dictionary; a custom dictionary that
        // shadows a pg_catalog builtin is ambiguous because the old creator's
        // search_path cannot be recovered.
        let name = if parts.len() == 1 {
            let names: Vec<String> = crate::utils::spi_get_one(
                "SELECT COALESCE(pg_catalog.array_agg(pg_catalog.format('%I.%I', n.nspname, d.dictname) \
                 ORDER BY n.nspname <> 'pg_catalog', n.nspname), ARRAY[]::text[]) \
                 FROM pg_catalog.pg_ts_dict d JOIN pg_catalog.pg_namespace n ON n.oid = d.dictnamespace \
                 WHERE d.dictname = $1",
                &[parts[0].as_str().into()],
            )
            .unwrap_or_default();
            match names.as_slice() {
                [one] => one.clone(),
                _ => crate::utils::raise_error(
                    pgrx::pg_sys::errcodes::PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
                    format!("legacy pg_dict name {name:?} does not identify one dictionary"),
                    None,
                    Some(
                        "Recreate the analyzer or tokenizer with a schema-qualified pg_dict name.",
                    ),
                ),
            }
        } else {
            name.to_owned()
        };
        // Validate on load; OIDs are deliberately not retained across calls.
        unsafe { pgrx::pg_sys::lookup_ts_dictionary_cache(dictionary_oid(&name)) };
        PgDictTokenFilter { name }
    }
}

impl TokenFilter for PgDictTokenFilter {
    fn apply(&self, token: String) -> Vec<String> {
        self.apply_with_oid(dictionary_oid(&self.name), token)
    }

    fn apply_batch(&self, tokens: Vec<String>) -> Vec<String> {
        // Resolve once per batch, using PostgreSQL's own namespace/syscaches.
        // DROP/CREATE gets its new OID; ALTER is handled by the TS cache.
        let oid = dictionary_oid(&self.name);
        let mut budget = crate::budget::Output::default();
        let mut results = Vec::with_capacity(tokens.len());
        for token in tokens {
            crate::utils::check_for_interrupts();
            let output = self.apply_with_oid(oid, token);
            for token in &output {
                budget.text(token.len());
            }
            results.extend(output);
        }
        results
    }
}

impl PgDictTokenFilter {
    fn apply_with_oid(&self, oid: pgrx::pg_sys::Oid, token: String) -> Vec<String> {
        crate::budget::piece(token.len());
        unsafe {
            let dict = pgrx::pg_sys::lookup_ts_dictionary_cache(oid)
                .as_mut()
                .unwrap();

            let length: i32 = token.len().try_into().unwrap();
            // Follow ts_lexize(): pass the DictSubState argument that
            // dictionaries such as thesaurus require, and, when one asks for
            // the next token, report the end of input instead.
            let mut state = pgrx::pg_sys::DictSubState {
                isend: false,
                getnext: false,
                private_state: std::ptr::null_mut(),
            };
            let mut lexize = |state: *mut pgrx::pg_sys::DictSubState| {
                pgrx::pg_sys::FunctionCall4Coll(
                    &raw mut dict.lexize,
                    pgrx::pg_sys::InvalidOid,
                    dict.dictData.into(),
                    token.as_ptr().into(),
                    length.into(),
                    state.into(),
                )
            };
            let mut res = lexize(&raw mut state);
            if state.getnext {
                state.isend = true;
                let last = lexize(&raw mut state);
                if !last.is_null() {
                    res = last;
                }
            }
            if res.is_null() {
                // not recognized
                return vec![token];
            }

            let mut lexeme_ptr: *const pgrx::pg_sys::TSLexeme = res.cast_mut_ptr();
            let mut results = Vec::new();
            let mut budget = crate::budget::Output::default();
            while !(*lexeme_ptr).lexeme.is_null() {
                let str = CStr::from_ptr((*lexeme_ptr).lexeme);
                budget.text(str.to_bytes().len());
                results.push(str.to_str().unwrap().to_string());
                pgrx::pg_sys::pfree((*lexeme_ptr).lexeme.cast());
                lexeme_ptr = lexeme_ptr.add(1);
            }
            pgrx::pg_sys::pfree(res.cast_mut_ptr());

            results
        }
    }
}
