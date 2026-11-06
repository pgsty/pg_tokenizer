// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

mod cache;
pub mod character_filter;
pub mod model;
pub mod pre_tokenizer;
pub mod text_analyzer;
pub mod token_filter;
pub mod tokenizer;
pub mod tsvector;
pub mod utils;

::pgrx::pg_module_magic!();

#[cfg(not(all(target_endian = "little", target_pointer_width = "64")))]
compile_error!("Target is not supported.");

#[cfg(not(any(
    feature = "pg14",
    feature = "pg15",
    feature = "pg16",
    feature = "pg17",
    feature = "pg18",
    feature = "pg19"
)))]
compile_error!("PostgreSQL version must be selected.");

#[pgrx::pg_guard]
extern "C-unwind" fn _PG_init() {
    // Under EXEC_BACKEND, children reload preloaded libraries with
    // IsUnderPostmaster set; only this flag identifies preloading.
    if !unsafe { pgrx::pg_sys::process_shared_preload_libraries_in_progress } {
        pgrx::error!("pg_tokenizer must be loaded via shared_preload_libraries.");
    }
    IS_MAIN.set(true);
    // Library worker threads must never call into PostgreSQL.
    utils::install_interrupt_check(|| {
        if is_main() {
            pgrx::check_for_interrupts!();
        }
    });

    cache::init();
    pre_tokenizer::init();
    model::init();
}

thread_local! {
    // Set on the thread that ran _PG_init; backends inherit it with fork.
    static IS_MAIN: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

// Whether this is the backend's main thread, the only thread that may call
// PostgreSQL.
#[must_use]
fn is_main() -> bool {
    IS_MAIN.get()
}

#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {
        // perform one-off initialization when the pg_test framework starts
    }

    pub fn postgresql_conf_options() -> Vec<&'static str> {
        // return any postgresql.conf settings that are required for your tests
        vec![r#"search_path = '"$user", public, tokenizer_catalog'"#]
    }
}

pgrx::extension_sql_file!("sql/privileges.sql", name = "catalog_privileges", finalize);
