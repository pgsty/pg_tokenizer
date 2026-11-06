// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::OnceLock;

use pgrx::{
    datum::DatumWithOid,
    pg_sys::{
        errcodes::PgSqlErrorCode,
        panic::{ErrorReport, ErrorReportable},
    },
    FromDatum, IntoDatum, PgLogLevel,
};

/// Raise a PostgreSQL ERROR with a SQLSTATE and optional DETAIL and HINT.
#[track_caller]
pub fn raise_error(
    code: PgSqlErrorCode,
    message: impl Into<String>,
    detail: Option<String>,
    hint: Option<&str>,
) -> ! {
    let mut report = ErrorReport::new(code, message.into(), "pg_tokenizer");
    if let Some(detail) = detail {
        report = report.set_detail(detail);
    }
    if let Some(hint) = hint {
        report = report.set_hint(hint);
    }
    report.report(PgLogLevel::ERROR);
    unreachable!("ERROR reports do not return")
}

/// Raise ERRCODE_INVALID_PARAMETER_VALUE for a configuration that cannot be
/// parsed or validated, with the parser's message as DETAIL.
#[track_caller]
pub fn invalid_config(kind: &str, name: &str, error: impl std::fmt::Display) -> ! {
    raise_error(
        PgSqlErrorCode::ERRCODE_INVALID_PARAMETER_VALUE,
        format!("invalid configuration for {kind} \"{name}\""),
        Some(error.to_string()),
        None,
    )
}

static INTERRUPT_CHECK: OnceLock<fn()> = OnceLock::new();

/// Installed by _PG_init. Unit tests never run it, so their executables do not
/// reference PostgreSQL symbols through check_for_interrupts.
pub fn install_interrupt_check(check: fn()) {
    let _ = INTERRUPT_CHECK.set(check);
}

/// Process a pending query cancel or statement timeout. Call it only where no
/// lock or other resource needs explicit release.
pub fn check_for_interrupts() {
    if let Some(check) = INTERRUPT_CHECK.get() {
        check();
    }
}

/// Check for interrupts at the first item and then every 1024 items of a loop.
pub fn check_for_interrupts_every(i: usize) {
    if i.is_multiple_of(1024) {
        check_for_interrupts();
    }
}

pub fn spi_get_one<T>(query: &str, args: &[DatumWithOid]) -> Option<T>
where
    T: FromDatum + IntoDatum,
{
    pgrx::Spi::connect(|client| {
        let tuptable = client.select(query, Some(1), args).unwrap_or_report();

        match tuptable.first().get_one::<T>() {
            Ok(Some(bytes)) => Some(bytes),
            Ok(None) => panic!("Get null value when executing spi query: {}", query),
            Err(pgrx::spi::SpiError::InvalidPosition) => None,
            Err(e) => panic!(
                "Failed to executing spi query, error: {}, query: {}",
                e, query
            ),
        }
    })
}

#[cfg(test)]
pub(crate) mod interrupt_tests {
    use std::cell::Cell;

    thread_local! {
        static CHECKS: Cell<usize> = const { Cell::new(0) };
    }

    fn count() {
        CHECKS.set(CHECKS.get() + 1);
    }

    /// Run `f` and return how many interrupt checks this thread made. Every
    /// test installs the same counting hook, so the counts are per thread.
    pub(crate) fn count_interrupt_checks<R>(f: impl FnOnce() -> R) -> (R, usize) {
        super::install_interrupt_check(count);
        let before = CHECKS.get();
        let result = f();
        (result, CHECKS.get() - before)
    }
}
