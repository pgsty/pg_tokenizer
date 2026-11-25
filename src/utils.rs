// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::cell::RefCell;
use std::ffi::{c_char, c_long, CStr, CString};
use std::sync::OnceLock;

use pgrx::datum::DatumWithOid;
use pgrx::pg_sys::errcodes::PgSqlErrorCode;
use pgrx::pg_sys::panic::{ErrorReport, ErrorReportable};
use pgrx::{pg_sys, FromDatum, IntoDatum, PgLogLevel};

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
#[inline]
pub fn check_for_interrupts() {
    if let Some(check) = INTERRUPT_CHECK.get() {
        check();
    }
}

/// Check for interrupts at the first item and then every 1024 items of a loop.
#[inline]
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
pub fn catalog_one<T: OwnedSpiValue>(query: &str, args: &[DatumWithOid]) -> Option<T> {
    let query = CString::new(query).expect("query contains no NUL");
    catalog_one_cstr(&query, args)
}

pub fn catalog_one_cstr<T: OwnedSpiValue>(query: &CStr, args: &[DatumWithOid]) -> Option<T> {
    let mut result = None;
    read_only_each_cstr(query, args, 1, |row| {
        result = row.get::<T>(1);
    });
    result
}

/// # Safety
/// Decoding any listed SQL type (or a domain over it) must yield a fully owned value.
pub unsafe trait OwnedSpiValue: FromDatum {
    const SQL_TYPES: &'static [pg_sys::Oid];
}

unsafe impl OwnedSpiValue for bool {
    const SQL_TYPES: &'static [pg_sys::Oid] = &[pg_sys::BOOLOID];
}

unsafe impl OwnedSpiValue for i32 {
    const SQL_TYPES: &'static [pg_sys::Oid] = &[pg_sys::INT4OID];
}

unsafe impl OwnedSpiValue for i64 {
    const SQL_TYPES: &'static [pg_sys::Oid] = &[pg_sys::INT8OID];
}

unsafe impl OwnedSpiValue for pg_sys::Oid {
    const SQL_TYPES: &'static [pg_sys::Oid] = &[pg_sys::OIDOID];
}

unsafe impl OwnedSpiValue for String {
    const SQL_TYPES: &'static [pg_sys::Oid] = &[pg_sys::TEXTOID, pg_sys::VARCHAROID];
}

unsafe impl OwnedSpiValue for Vec<String> {
    const SQL_TYPES: &'static [pg_sys::Oid] = &[pg_sys::TEXTARRAYOID, pg_sys::VARCHARARRAYOID];
}

/// One row of a [`read_only_rows`] or [`KeptPlan`] result, valid only inside its callback.
pub struct ReadOnlyRow {
    tuple: pg_sys::HeapTuple,
    descriptor: pg_sys::TupleDesc,
    domain_bases: RefCell<Vec<(pg_sys::Oid, pg_sys::Oid)>>,
}

impl ReadOnlyRow {
    /// Decode a 1-based column into an owned Rust value, returning None for SQL NULL.
    #[inline]
    pub fn get<T: OwnedSpiValue>(&self, column: i32) -> Option<T> {
        unsafe {
            let oid = pg_sys::SPI_gettypeid(self.descriptor, column);
            if column < 1 || oid == pg_sys::InvalidOid {
                raise_error(
                    PgSqlErrorCode::ERRCODE_UNDEFINED_COLUMN,
                    format!("SPI result has no column {column}"),
                    None,
                    None,
                );
            }
            if !T::SQL_TYPES.contains(&oid) && !T::SQL_TYPES.contains(&self.base_type(oid)) {
                raise_error(
                    PgSqlErrorCode::ERRCODE_DATATYPE_MISMATCH,
                    format!("unexpected type for SPI result column {column}"),
                    Some(format!(
                        "Got type OID {oid}, expected one of {:?} or a domain over it.",
                        T::SQL_TYPES
                    )),
                    None,
                );
            }
            let mut is_null = false;
            let datum = pg_sys::SPI_getbinval(self.tuple, self.descriptor, column, &mut is_null);
            T::from_polymorphic_datum(datum, is_null, oid)
        }
    }

    #[cold]
    fn base_type(&self, oid: pg_sys::Oid) -> pg_sys::Oid {
        let mut bases = self.domain_bases.borrow_mut();
        if let Some((_, base)) = bases.iter().find(|(ty, _)| *ty == oid) {
            return *base;
        }
        let base = unsafe { pg_sys::getBaseType(oid) };
        bases.push((oid, base));
        base
    }
}

/// The current SPI invocation owns the tuple table throughout these callbacks.
unsafe fn visit_spi_rows(mut on_row: impl FnMut(&ReadOnlyRow)) -> usize {
    let processed = pg_sys::SPI_processed as usize;
    if processed == 0 || pg_sys::SPI_tuptable.is_null() {
        return processed;
    }
    let table = &*pg_sys::SPI_tuptable;
    let mut row = ReadOnlyRow {
        tuple: std::ptr::null_mut(),
        descriptor: table.tupdesc,
        domain_bases: RefCell::default(),
    };
    for i in 0..processed {
        check_for_interrupts_every(i);
        row.tuple = *table.vals.add(i);
        on_row(&row);
    }
    processed
}

fn with_spi_args<R>(
    args: &[DatumWithOid],
    f: impl FnOnce(*mut pg_sys::Oid, *mut pg_sys::Datum, *const c_char) -> R,
) -> R {
    if args.is_empty() {
        return f(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
    }
    if args.len() <= 4 {
        let mut types = [pg_sys::InvalidOid; 4];
        let mut values = [pg_sys::Datum::from(0usize); 4];
        let mut nulls = [b' ' as c_char; 4];
        for (i, arg) in args.iter().enumerate() {
            types[i] = arg.oid();
            match arg.datum() {
                Some(datum) => {
                    values[i] = datum.sans_lifetime();
                    nulls[i] = b' ' as c_char;
                }
                None => {
                    values[i] = pg_sys::Datum::from(0usize);
                    nulls[i] = b'n' as c_char;
                }
            }
        }
        return f(types.as_mut_ptr(), values.as_mut_ptr(), nulls.as_ptr());
    }
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
        .map(|arg| if arg.datum().is_some() { b' ' } else { b'n' } as c_char)
        .collect();
    f(types.as_mut_ptr(), values.as_mut_ptr(), nulls.as_ptr())
}

fn read_only_each_cstr(
    query: &CStr,
    args: &[DatumWithOid],
    limit: c_long,
    on_row: impl FnMut(&ReadOnlyRow),
) {
    with_spi_args(args, |types, values, nulls| {
        pgrx::Spi::connect(|_| unsafe {
            let status = pg_sys::SPI_execute_with_args(
                query.as_ptr(),
                args.len() as i32,
                types,
                values,
                nulls,
                true,
                limit,
            );
            assert!(status >= 0, "read-only SPI query failed: {status}");
            visit_spi_rows(on_row);
        })
    })
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
    limit: c_long,
    mut decode: impl FnMut(&ReadOnlyRow) -> R,
) -> Vec<R> {
    let query = CString::new(query).expect("query contains no NUL");
    let mut rows = Vec::new();
    read_only_each_cstr(&query, args, limit, |row| rows.push(decode(row)));
    rows
}

/// A saved PostgreSQL SPI plan living in CacheMemoryContext.
///
/// Hold inside `Rc<KeptPlan>` and release any outer `RefCell` borrow before
/// calling `execute_*` so nested relcache invalidations can never free a plan
/// that is currently executing on the stack.
pub struct KeptPlan {
    raw: pg_sys::SPIPlanPtr,
}

impl KeptPlan {
    pub fn prepare(query: &CStr, argtypes: &[pg_sys::Oid]) -> Self {
        pgrx::Spi::connect(|_| unsafe {
            let ptr = if argtypes.is_empty() {
                std::ptr::null_mut()
            } else {
                argtypes.as_ptr().cast_mut()
            };
            let plan = pg_sys::SPI_prepare(query.as_ptr(), argtypes.len() as i32, ptr);
            let spi_result = pg_sys::SPI_result;
            assert!(!plan.is_null(), "SPI_prepare failed: {spi_result}");
            let kept = pg_sys::SPI_keepplan(plan);
            assert!(kept == 0, "SPI_keepplan failed: {kept}");
            Self { raw: plan }
        })
    }

    pub fn execute_one<T: OwnedSpiValue>(
        &self,
        args: &[DatumWithOid],
        read_only: bool,
    ) -> Option<T> {
        let mut result = None;
        self.execute_each(args, read_only, 1, |row| {
            result = row.get::<T>(1);
        });
        result
    }

    pub fn execute_each(
        &self,
        args: &[DatumWithOid],
        read_only: bool,
        limit: c_long,
        on_row: impl FnMut(&ReadOnlyRow),
    ) -> usize {
        with_spi_args(args, |_, values, nulls| {
            let run = || unsafe {
                let status = pg_sys::SPI_execute_plan(self.raw, values, nulls, read_only, limit);
                assert!(status >= 0, "SPI_execute_plan failed: {status}");
                visit_spi_rows(on_row)
            };
            if read_only {
                pgrx::Spi::connect(|_| run())
            } else {
                pgrx::Spi::connect_mut(|_| run())
            }
        })
    }

    pub fn execute_void(&self, args: &[DatumWithOid], read_only: bool) -> usize {
        with_spi_args(args, |_, values, nulls| {
            let run = || unsafe {
                let status = pg_sys::SPI_execute_plan(self.raw, values, nulls, read_only, 0);
                assert!(status >= 0, "SPI_execute_plan failed: {status}");
                pg_sys::SPI_processed as usize
            };
            if read_only {
                pgrx::Spi::connect(|_| run())
            } else {
                pgrx::Spi::connect_mut(|_| run())
            }
        })
    }
}

impl Drop for KeptPlan {
    fn drop(&mut self) {
        unsafe {
            // SPI_freeplan drops saved plan sources and their contexts even during xact cleanup.
            if !pg_sys::proc_exit_inprogress {
                pg_sys::SPI_freeplan(self.raw);
            }
        }
    }
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
