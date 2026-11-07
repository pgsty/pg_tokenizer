// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use std::sync::OnceLock;

static REPORT: OnceLock<fn(&str, usize) -> !> = OnceLock::new();
static LIMITS: OnceLock<fn() -> Limits> = OnceLock::new();

struct Limits {
    input: usize,
    tokens: usize,
    output: usize,
    piece: usize,
}

impl Limits {
    fn current() -> Self {
        LIMITS.get().map_or(
            Self {
                input: 16 * 1024 * 1024,
                tokens: 1_000_000,
                output: 64 * 1024 * 1024,
                piece: 1024 * 1024,
            },
            |read| read(),
        )
    }
}

static INPUT: GucSetting<i32> = GucSetting::<i32>::new(16 * 1024 * 1024);
static TOKENS: GucSetting<i32> = GucSetting::<i32>::new(1_000_000);
static OUTPUT: GucSetting<i32> = GucSetting::<i32>::new(64 * 1024 * 1024);
static PIECE: GucSetting<i32> = GucSetting::<i32>::new(1024 * 1024);

pub fn init() {
    let _ = LIMITS.set(|| Limits {
        input: INPUT.get() as usize,
        tokens: TOKENS.get() as usize,
        output: OUTPUT.get() as usize,
        piece: PIECE.get() as usize,
    });
    let _ = REPORT.set(|setting, limit| {
        crate::utils::raise_error(
            pgrx::pg_sys::errcodes::PgSqlErrorCode::ERRCODE_PROGRAM_LIMIT_EXCEEDED,
            format!("pg_tokenizer exceeds {setting} ({limit})"),
            None,
            Some("Reduce the document or expansion, or explicitly raise the pg_tokenizer limit."),
        )
    });
    for (name, description, setting) in [
        (
            c"pg_tokenizer.max_input_bytes",
            c"Maximum bytes in one analyzer input.",
            &INPUT,
        ),
        (
            c"pg_tokenizer.max_output_tokens",
            c"Maximum tokens in one analysis stage.",
            &TOKENS,
        ),
        (
            c"pg_tokenizer.max_output_bytes",
            c"Maximum accounted bytes in one analysis stage.",
            &OUTPUT,
        ),
        (
            c"pg_tokenizer.max_piece_bytes",
            c"Maximum input bytes in one opaque tokenizer or regex call.",
            &PIECE,
        ),
    ] {
        GucRegistry::define_int_guc(
            name,
            description,
            description,
            setting,
            1,
            i32::MAX,
            GucContext::Userset,
            GucFlags::default(),
        );
    }
}

fn exceeded(setting: &str, limit: usize) -> ! {
    // Like the interrupt hook, standalone unit tests must not link PG symbols.
    if let Some(report) = REPORT.get() {
        report(setting, limit);
    }
    panic!("pg_tokenizer exceeds {setting} ({limit})")
}

fn check(value: usize, limit: usize, setting: &str) {
    if value > limit {
        exceeded(setting, limit);
    }
}

pub fn input(bytes: usize) {
    check(bytes, Limits::current().input, "max_input_bytes");
}
pub fn piece(bytes: usize) {
    check(bytes, Limits::current().piece, "max_piece_bytes");
}
pub fn bytes(bytes: usize) {
    check(bytes, Limits::current().output, "max_output_bytes");
}
pub fn tokens(tokens: usize) {
    check(tokens, Limits::current().tokens, "max_output_tokens");
}

pub struct Output {
    count: usize,
    bytes: usize,
    limits: Limits,
}

impl Default for Output {
    fn default() -> Self {
        Self {
            count: 0,
            bytes: 0,
            limits: Limits::current(),
        }
    }
}

impl Output {
    pub fn workspace(&self, bytes: usize) {
        check(bytes, self.limits.output, "max_output_bytes");
    }
    pub fn text(&mut self, len: usize) {
        self.add(1, len.saturating_add(std::mem::size_of::<String>()));
    }

    pub fn ids(&mut self, count: usize) {
        self.add(count, count.saturating_mul(std::mem::size_of::<u32>()));
    }

    fn add(&mut self, count: usize, size: usize) {
        self.count = self.count.saturating_add(count);
        self.bytes = self.bytes.saturating_add(size);
        check(self.count, self.limits.tokens, "max_output_tokens");
        self.workspace(self.bytes);
    }
}

pub fn borrowed<'a>(parts: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut budget = Output::default();
    let mut result = Vec::new();
    for (i, part) in parts.enumerate() {
        crate::utils::check_for_interrupts_every(i);
        budget.text(part.len());
        result.push(part);
    }
    result
}

pub fn text(parts: &[String]) {
    let mut budget = Output::default();
    for (i, part) in parts.iter().enumerate() {
        crate::utils::check_for_interrupts_every(i);
        budget.text(part.len());
    }
}

pub fn characters(chars: impl Iterator<Item = char>) -> String {
    let mut result = String::new();
    let limit = Limits::current().output;
    for (i, ch) in chars.enumerate() {
        crate::utils::check_for_interrupts_every(i);
        check(
            result.len().saturating_add(ch.len_utf8()),
            limit,
            "max_output_bytes",
        );
        result.push(ch);
    }
    result
}
