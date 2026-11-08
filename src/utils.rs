// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::sync::OnceLock;

use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::errcodes::PgSqlErrorCode;
use pgrx::pg_sys::panic::{ErrorReport, ErrorReportable};
use pgrx::{FromDatum, IntoDatum, PgLogLevel};

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

/// Read an owned scalar using the caller's active snapshot, even after writes.
/// pgrx's select() switches to read-write SPI once a transaction has an XID;
/// that would give a version probe and the definition lookup different snapshots.
pub fn catalog_one<T: FromDatum>(query: &str, args: &[DatumWithOid]) -> Option<T> {
    read_only_rows(query, args, 1, |row| row.get::<T>(1))
        .into_iter()
        .next()
        .flatten()
}

/// One row of a [`read_only_rows`] result, valid only inside its callback.
pub struct ReadOnlyRow {
    tuple: pgrx::pg_sys::HeapTuple,
    descriptor: pgrx::pg_sys::TupleDesc,
}

impl ReadOnlyRow {
    /// Decode a 1-based column into an owned Rust value; None for SQL NULL.
    pub fn get<T: FromDatum>(&self, column: i32) -> Option<T> {
        let mut is_null = false;
        unsafe {
            let datum =
                pgrx::pg_sys::SPI_getbinval(self.tuple, self.descriptor, column, &mut is_null);
            T::from_datum(datum, is_null)
        }
    }
}

/// Run a query with read-only SPI under the caller's active snapshot and decode
/// every row (at most `limit`, 0 for all) before SPI memory is released.
///
/// Read-only SPI neither takes a new snapshot nor advances the command counter.
/// The read-write SPI that pgrx's select() uses once a transaction has an XID
/// does both, and PostgreSQL forbids advancing the command counter during a
/// parallel operation.
pub fn read_only_rows<R>(
    query: &str,
    args: &[DatumWithOid],
    limit: std::ffi::c_long,
    mut decode: impl FnMut(&ReadOnlyRow) -> R,
) -> Vec<R> {
    use pgrx::pg_sys;
    let query = std::ffi::CString::new(query).expect("query contains no NUL");
    let mut types: Vec<_> = args.iter().map(|arg| arg.oid()).collect();
    let mut values: Vec<_> = args
        .iter()
        .map(|arg| {
            arg.datum()
                .map_or(pg_sys::Datum::from(0usize), |datum| datum.sans_lifetime())
        })
        .collect();
    let nulls: Vec<_> = args
        .iter()
        .map(|arg| if arg.datum().is_some() { b' ' } else { b'n' } as std::ffi::c_char)
        .collect();
    pgrx::Spi::connect(|_| unsafe {
        let status = pg_sys::SPI_execute_with_args(
            query.as_ptr(),
            args.len() as i32,
            types.as_mut_ptr(),
            values.as_mut_ptr(),
            nulls.as_ptr(),
            true,
            limit,
        );
        assert!(status >= 0, "read-only SPI query failed: {status}");
        let processed = pg_sys::SPI_processed as usize;
        if processed == 0 {
            return Vec::new();
        }
        let table = &*pg_sys::SPI_tuptable;
        let mut rows = Vec::with_capacity(processed);
        for i in 0..processed {
            check_for_interrupts_every(i);
            rows.push(decode(&ReadOnlyRow {
                tuple: *table.vals.add(i),
                descriptor: table.tupdesc,
            }));
        }
        rows
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
