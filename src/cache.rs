// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

use pgrx::{
    register_subxact_callback, register_xact_callback, PgAtomic, PgSubXactCallbackEvent,
    PgXactCallbackEvent,
};

// The epoch is shared by all PostgreSQL backends.  It is deliberately a single
// scalar: the catalog is the source of truth, so an invalidation only needs to
// tell other backends that every derived object may be stale.
static CACHE_EPOCH: PgAtomic<AtomicU64> = unsafe { PgAtomic::new(c"pg_tokenizer_cache_epoch") };

// These values live in the current backend and are not shared with PostgreSQL.
static LOCAL_CACHE_EPOCH: AtomicU64 = AtomicU64::new(0);
static TRANSACTION_DIRTY: AtomicBool = AtomicBool::new(false);
static TRANSACTION_SAW_EPOCH: AtomicBool = AtomicBool::new(false);
static LAST_STATEMENT: AtomicI64 = AtomicI64::new(0);
static LAST_COMMAND: AtomicU32 = AtomicU32::new(0);
static VALIDATION_CALLBACKS: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
struct SnapshotKey {
    statement: i64,
    command: u32,
    snapshot: usize,
    snapshot_command: u32,
    recovery: bool,
}

thread_local! {
    static DIRTY_SUBTRANSACTIONS: RefCell<HashSet<pgrx::pg_sys::SubTransactionId>> = RefCell::default();
    static VALIDATED_SNAPSHOT: RefCell<Option<SnapshotKey>> = const { RefCell::new(None) };
    static CATALOG_VERSION: RefCell<Option<(pgrx::pg_sys::Oid, i64)>> = const { RefCell::new(None) };
}

#[allow(unexpected_cfgs)]
pub fn init() {
    pgrx::pg_shmem_init!(CACHE_EPOCH);
}

fn clear_local_caches() {
    VALIDATED_SNAPSHOT.with(|key| *key.borrow_mut() = None);
    crate::model::clear_cache();
    crate::text_analyzer::clear_cache();
    crate::token_filter::clear_caches();
    crate::tokenizer::clear_cache();
}

fn end_validation() {
    VALIDATED_SNAPSHOT.with(|key| *key.borrow_mut() = None);
    VALIDATION_CALLBACKS.store(false, Ordering::SeqCst);
}

// The version and definitions are read through the same active SQL snapshot.
// A single-row probe per statement works on the primary, during WAL replay and
// after promotion. Comparing in both directions also protects old snapshots.
fn check_catalog_version() -> bool {
    use pgrx::pg_sys;
    let snapshot = unsafe { pg_sys::GetActiveSnapshot() };
    let worker = unsafe { pg_sys::ParallelWorkerNumber } >= 0;
    let key = SnapshotKey {
        statement: unsafe { pg_sys::GetCurrentStatementStartTimestamp() },
        // Mark read-only commands used, so multiple statements in one protocol
        // message cannot reuse a key. Parallel workers execute one query and
        // must not set the leader's command-ID-used flag.
        command: unsafe { pg_sys::GetCurrentCommandId(!worker) },
        snapshot: snapshot as usize,
        snapshot_command: unsafe { (*snapshot).curcid },
        recovery: unsafe { pg_sys::RecoveryInProgress() },
    };
    if VALIDATED_SNAPSHOT.with(|cached| *cached.borrow() == Some(key)) {
        return true;
    }
    let namespace = unsafe { pg_sys::get_namespace_oid(c"tokenizer_catalog".as_ptr(), true) };
    let relation = unsafe { pg_sys::get_relname_relid(c"definition_version".as_ptr(), namespace) };
    if relation == pg_sys::InvalidOid {
        // Old extension SQL is usable while an administrator upgrades it.
        return false;
    }
    let extension = unsafe { pg_sys::get_extension_oid(c"pg_tokenizer".as_ptr(), false) };
    if unsafe { pg_sys::getExtensionOfObject(pg_sys::RelationRelationId, relation) } != extension {
        // A database owner could have occupied this name before the upgrade.
        // Never adopt an unrelated table as the extension's consistency signal.
        return false;
    }
    let version = crate::utils::catalog_one::<i64>(
        "SELECT version FROM tokenizer_catalog.definition_version WHERE singleton",
        &[],
    )
    .unwrap_or_else(|| {
        // Syscache can see an upgrade committed after the caller's snapshot.
        // Do not silently replace that snapshot: version and definitions must
        // remain consistent, including inside REPEATABLE READ transactions.
        let visible = crate::utils::catalog_one::<bool>(
            "SELECT EXISTS (SELECT FROM pg_catalog.pg_class WHERE oid = $1)",
            &[relation.into()],
        );
        if visible == Some(false) {
            crate::utils::raise_error(
                pg_sys::errcodes::PgSqlErrorCode::ERRCODE_T_R_SERIALIZATION_FAILURE,
                "pg_tokenizer was upgraded after the current snapshot",
                None,
                Some("Retry the transaction with a new snapshot."),
            );
        }
        pgrx::error!("pg_tokenizer definition version is missing")
    });
    let identity = (relation, version);
    let changed = CATALOG_VERSION.with(|cached| cached.replace(Some(identity)) != Some(identity));
    if changed {
        clear_local_caches();
    }
    VALIDATED_SNAPSHOT.with(|cached| *cached.borrow_mut() = Some(key));
    if !VALIDATION_CALLBACKS.swap(true, Ordering::SeqCst) {
        for event in [
            PgXactCallbackEvent::Commit,
            PgXactCallbackEvent::Prepare,
            PgXactCallbackEvent::ParallelCommit,
        ] {
            register_xact_callback(event, end_validation);
        }
        for event in [
            PgXactCallbackEvent::Abort,
            PgXactCallbackEvent::ParallelAbort,
        ] {
            register_xact_callback(event, || {
                // Direct catalog DML can load uncommitted definitions without
                // mark_catalog_changed(). Its rolled-back counter value may
                // later be reused by another transaction with different data.
                clear_local_caches();
                end_validation();
            });
        }
        register_subxact_callback(PgSubXactCallbackEvent::AbortSub, |_, _| {
            clear_local_caches();
        });
    }
    true
}

fn commit_transaction() {
    // The catalog change is visible now.  Publish the new epoch only at this
    // point, so another backend can never observe it before the commit.
    clear_local_caches();
    let changed = DIRTY_SUBTRANSACTIONS.with(|dirty| !dirty.take().is_empty());
    if changed {
        let epoch = CACHE_EPOCH.get().fetch_add(1, Ordering::SeqCst) + 1;
        LOCAL_CACHE_EPOCH.store(epoch, Ordering::SeqCst);
    }
    TRANSACTION_DIRTY.store(false, Ordering::SeqCst);
}

fn abort_transaction() {
    // Other backends never saw the rolled-back change; only this one did.
    clear_local_caches();
    DIRTY_SUBTRANSACTIONS.with(|dirty| dirty.borrow_mut().clear());
    TRANSACTION_DIRTY.store(false, Ordering::SeqCst);
}

fn end_transaction_after_new_epoch() {
    // Objects loaded after a new epoch was observed may come from a snapshot
    // taken before that commit.  Later transactions take newer snapshots.
    clear_local_caches();
    TRANSACTION_SAW_EPOCH.store(false, Ordering::SeqCst);
}

pub fn check() {
    if check_catalog_version() {
        return;
    }
    let shared_epoch = CACHE_EPOCH.get().load(Ordering::SeqCst);
    if LOCAL_CACHE_EPOCH.load(Ordering::SeqCst) != shared_epoch {
        clear_local_caches();
        LOCAL_CACHE_EPOCH.store(shared_epoch, Ordering::SeqCst);
        if !TRANSACTION_SAW_EPOCH.swap(true, Ordering::SeqCst) {
            register_xact_callback(PgXactCallbackEvent::Commit, end_transaction_after_new_epoch);
            register_xact_callback(PgXactCallbackEvent::Abort, end_transaction_after_new_epoch);
            register_xact_callback(
                PgXactCallbackEvent::Prepare,
                end_transaction_after_new_epoch,
            );
            register_xact_callback(
                PgXactCallbackEvent::ParallelCommit,
                end_transaction_after_new_epoch,
            );
            register_xact_callback(
                PgXactCallbackEvent::ParallelAbort,
                end_transaction_after_new_epoch,
            );
        }
    }
    // A parallel worker runs one query under the leader's snapshot, and it
    // must not mark command IDs used.
    let parallel_worker = unsafe { pgrx::pg_sys::ParallelWorkerNumber } >= 0;
    if TRANSACTION_SAW_EPOCH.load(Ordering::SeqCst) && !parallel_worker {
        // READ COMMITTED can take a newer snapshot before the transaction ends.
        // Keep objects within one statement, but do not carry a possibly old
        // snapshot's objects into its next statement (or the next transaction).
        let statement = unsafe { pgrx::pg_sys::GetCurrentStatementStartTimestamp() };
        // Multiple statements in a single protocol message share a timestamp.
        // Mark the command ID used so even a read-only statement advances it.
        let command = unsafe { pgrx::pg_sys::GetCurrentCommandId(true) };
        let changed_statement = LAST_STATEMENT.swap(statement, Ordering::SeqCst) != statement;
        let changed_command = LAST_COMMAND.swap(command, Ordering::SeqCst) != command;
        if changed_statement || changed_command {
            clear_local_caches();
        }
    }
}

pub fn mark_catalog_changed() {
    // Make the current backend see the catalog mutation immediately.  Other
    // backends wait for the commit callback, when the change is visible.
    clear_local_caches();
    DIRTY_SUBTRANSACTIONS.with(|dirty| {
        dirty
            .borrow_mut()
            .insert(unsafe { pgrx::pg_sys::GetCurrentSubTransactionId() });
    });

    if !TRANSACTION_DIRTY.swap(true, Ordering::SeqCst) {
        register_xact_callback(PgXactCallbackEvent::Commit, commit_transaction);
        register_xact_callback(PgXactCallbackEvent::Abort, abort_transaction);
        // COMMIT PREPARED may run in a backend that never calls back into us.
        register_xact_callback(PgXactCallbackEvent::PrePrepare, || {
            pgrx::error!("cannot PREPARE a transaction that changed pg_tokenizer definitions")
        });
        register_subxact_callback(PgSubXactCallbackEvent::AbortSub, |subid, _| {
            DIRTY_SUBTRANSACTIONS.with(|dirty| dirty.borrow_mut().remove(&subid));
            clear_local_caches();
        });
        register_subxact_callback(PgSubXactCallbackEvent::CommitSub, |subid, parent| {
            DIRTY_SUBTRANSACTIONS.with(|dirty| {
                let mut dirty = dirty.borrow_mut();
                if dirty.remove(&subid) {
                    dirty.insert(parent);
                }
            });
        });
    }
}
