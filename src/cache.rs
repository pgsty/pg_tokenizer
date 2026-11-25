// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at <http://www.apache.org/licenses/LICENSE-2.0>
//
// Copyright (c) 2025 TensorChord Inc.
// Copyright (c) 2026 PGSTY PTE. LTD.
// Maintained by Ruohang Feng @ PGSTY (rh@vonng.com)

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, CStr};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};

use pgrx::{pg_sys, PgAtomic};

use crate::utils::KeptPlan;

#[cfg(feature = "pg19")]
pub(crate) type SysCacheId = pg_sys::SysCacheIdentifier::Type;
#[cfg(not(feature = "pg19"))]
pub(crate) type SysCacheId = std::ffi::c_int;

// Older minors only expose namespace identity. Membership changes use relcache on every major.
#[cfg(any(feature = "pg15", feature = "pg16", feature = "pg17"))]
pub(crate) const EXTENSION_CACHE_ID: SysCacheId =
    pg_sys::SysCacheIdentifier::NAMESPACEOID as SysCacheId;
#[cfg(any(feature = "pg18", feature = "pg19"))]
pub(crate) const EXTENSION_CACHE_ID: SysCacheId =
    pg_sys::SysCacheIdentifier::EXTENSIONOID as SysCacheId;

// Shared across all backends. Bumped inside catalog-change triggers (before
// commit) and on commit, so readers never see a committed definition change
// before observing the new epoch and XID barrier.
static CACHE_EPOCH: PgAtomic<AtomicU64> = unsafe { PgAtomic::new(c"pg_tokenizer_cache_epoch") };

// Exclusive lower bound on FullTransactionId (64-bit, never wraps): any primary
// snapshot with xmin >= CHANGE_BARRIER_FXID is strictly past every definition
// writer and every pre-startup/pre-promotion prepared transaction.
// 0 = uninitialized; 1 = ancient barrier (> 2^31 XIDs old, retired); >= 3 = FXID.
static CHANGE_BARRIER_FXID: PgAtomic<AtomicU64> =
    unsafe { PgAtomic::new(c"pg_tokenizer_change_barrier_fxid") };

const BARRIER_UNINITIALIZED: u64 = 0;
const BARRIER_EXPIRED: u64 = 1;
const FIRST_NORMAL_XID: u32 = 3;

// Backend-local state (never shared across processes).
static LOCAL_CACHE_EPOCH: AtomicU64 = AtomicU64::new(0);
static TRANSACTION_DIRTY: AtomicBool = AtomicBool::new(false);
static TRANSACTION_API_DIRTY: AtomicBool = AtomicBool::new(false);
static TRANSACTION_SAW_EPOCH: AtomicBool = AtomicBool::new(false);
static TRANSACTION_UNSAFE_LEGACY: AtomicBool = AtomicBool::new(false);
static LAST_STATEMENT: AtomicI64 = AtomicI64::new(0);
static LAST_COMMAND: AtomicU32 = AtomicU32::new(0);
static mut PREVIOUS_UTILITY: pg_sys::ProcessUtility_hook_type = None;
static CALLBACKS_REGISTERED: AtomicBool = AtomicBool::new(false);

// Bumped asynchronously by PostgreSQL relcache/syscache invalidation callbacks.
static CATALOG_IDENTITY_GEN: AtomicU64 = AtomicU64::new(1);
static CACHED_DEF_VER_OID: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
struct SnapshotKey {
    statement: i64,
    command: u32,
    snapshot: usize,
    snapshot_command: u32,
    snapshot_xmin: u32,
    snapshot_xmax: u32,
    identity_gen: u64,
    recovery: bool,
}

struct CatalogIdentity {
    gen: u64,
    def_ver_oid: pg_sys::Oid,
    trigger_ready: bool,
    version_plan: Rc<KeptPlan>,
}

thread_local! {
    // Each live writer owns the count of statements still awaiting their AFTER trigger.
    static DIRTY_SUBTRANSACTIONS: RefCell<HashMap<pg_sys::SubTransactionId, usize>> = RefCell::default();
    static MEMBERSHIP_DDL: Cell<bool> = const { Cell::new(false) };
    static VALIDATED_SNAPSHOT: Cell<Option<SnapshotKey>> = const { Cell::new(None) };
    static CATALOG_VERSION: Cell<Option<(pg_sys::Oid, i64)>> = const { Cell::new(None) };
    // (identity_gen, validated_shared_epoch) when local caches are proven clean
    // for primary snapshots at or after CHANGE_BARRIER_FXID.
    static LOCAL_CLEAN: Cell<(u64, Option<u64>)> = const { Cell::new((0, None)) };
    static CATALOG_IDENTITY: RefCell<Option<CatalogIdentity>> = const { RefCell::new(None) };
}

#[allow(unexpected_cfgs)]
pub fn init() {
    pgrx::pg_shmem_init!(CACHE_EPOCH);
    pgrx::pg_shmem_init!(CHANGE_BARRIER_FXID);
    unsafe {
        PREVIOUS_UTILITY = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(process_utility);
    }
}

pub(crate) fn membership_ddl_active() -> bool {
    MEMBERSHIP_DDL.with(Cell::get)
}

struct MembershipDdl(bool);

impl Drop for MembershipDdl {
    fn drop(&mut self) {
        MEMBERSHIP_DDL.with(|active| active.set(self.0));
    }
}

#[allow(clippy::too_many_arguments, reason = "PostgreSQL hook signature")]
#[pgrx::pg_guard]
unsafe extern "C-unwind" fn process_utility(
    plan: *mut pg_sys::PlannedStmt,
    query: *const c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    completion: *mut pg_sys::QueryCompletion,
) {
    let stmt = (*plan).utilityStmt;
    let _membership = if (*stmt).type_ == pg_sys::NodeTag::T_AlterExtensionContentsStmt
        && CStr::from_ptr((*stmt.cast::<pg_sys::AlterExtensionContentsStmt>()).extname)
            == c"pg_tokenizer"
    {
        // pg_depend changes invalidate catalog snapshots but have no syscache callback.
        let guard = MembershipDdl(MEMBERSHIP_DDL.with(|active| active.replace(true)));
        pg_sys::CacheInvalidateRelcacheByRelid(pg_sys::ExtensionRelationId);
        // Deliver locally before ddl_command_start can cache the old membership.
        pg_sys::CommandCounterIncrement();
        Some(guard)
    } else {
        None
    };
    if let Some(previous) = PREVIOUS_UTILITY {
        pg_sys::ffi::pg_guard_ffi_boundary(|| {
            previous(
                plan,
                query,
                read_only_tree,
                context,
                params,
                env,
                dest,
                completion,
            )
        });
    } else {
        pg_sys::standard_ProcessUtility(
            plan,
            query,
            read_only_tree,
            context,
            params,
            env,
            dest,
            completion,
        );
    }
}

#[inline]
fn advance_fxid(fxid: u64) -> u64 {
    let next = fxid.wrapping_add(1);
    if (next as u32) < FIRST_NORMAL_XID {
        (next & !0xFFFF_FFFF) | u64::from(FIRST_NORMAL_XID)
    } else {
        next
    }
}

/// Returns true iff a live snapshot's 32-bit `xmin` is guaranteed >= `barrier_fxid`.
///
/// PostgreSQL enforces `next_fxid - snapshot_xmin_fxid < 2^31` via xidStopLimit,
/// so for any barrier within 2^31 XIDs of nextXid, signed 32-bit modular
/// comparison `(barrier_xid - xmin) as i32 <= 0` is exact with zero false
/// positives. Barriers older than 2^31 XIDs are strictly behind every live
/// snapshot (and are retired to `BARRIER_EXPIRED` on the slow path).
#[inline]
fn xid_at_or_after_barrier(xmin: u32, barrier_fxid: u64) -> bool {
    if barrier_fxid == BARRIER_EXPIRED {
        return true;
    }
    if barrier_fxid < u64::from(FIRST_NORMAL_XID) || xmin < FIRST_NORMAL_XID {
        return false;
    }
    let barrier_xid = barrier_fxid as u32;
    xmin == barrier_xid || (barrier_xid.wrapping_sub(xmin) as i32) < 0
}

fn ensure_barrier_initialized() -> u64 {
    let atomic = CHANGE_BARRIER_FXID.get();
    let current = atomic.load(Ordering::Acquire);
    if current != BARRIER_UNINITIALIZED {
        return current;
    }
    let next = unsafe { pg_sys::ReadNextFullTransactionId().value };
    match atomic.compare_exchange(
        BARRIER_UNINITIALIZED,
        next,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => next,
        Err(existing) => existing,
    }
}

fn maybe_retire_ancient_barrier(barrier: u64) -> u64 {
    if barrier < u64::from(FIRST_NORMAL_XID) {
        return barrier;
    }
    let next = unsafe { pg_sys::ReadNextFullTransactionId().value };
    if next.saturating_sub(barrier) >= (1u64 << 31)
        && CHANGE_BARRIER_FXID
            .get()
            .compare_exchange(
                barrier,
                BARRIER_EXPIRED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    {
        return BARRIER_EXPIRED;
    }
    barrier
}

fn publish_barrier_and_bump_epoch(fxid: u64) {
    if fxid >= u64::from(FIRST_NORMAL_XID) {
        let target = advance_fxid(fxid);
        let atomic = CHANGE_BARRIER_FXID.get();
        let mut current = atomic.load(Ordering::Acquire);
        while current < target {
            match atomic.compare_exchange_weak(current, target, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }
    CACHE_EPOCH.get().fetch_add(1, Ordering::SeqCst);
}

#[pgrx::pg_guard]
unsafe extern "C-unwind" fn on_relcache_inval(_arg: pg_sys::Datum, relid: pg_sys::Oid) {
    let tracked = CACHED_DEF_VER_OID.load(Ordering::Relaxed);
    if relid == pg_sys::InvalidOid
        || relid == pg_sys::ExtensionRelationId
        || (tracked != 0 && u32::from(relid) == tracked)
    {
        CATALOG_IDENTITY_GEN.fetch_add(1, Ordering::Release);
    }
}

#[pgrx::pg_guard]
unsafe extern "C-unwind" fn on_syscache_inval(
    _arg: pg_sys::Datum,
    _cacheid: SysCacheId,
    _hashvalue: u32,
) {
    CATALOG_IDENTITY_GEN.fetch_add(1, Ordering::Release);
}

#[pgrx::pg_guard]
unsafe extern "C-unwind" fn on_xact_event(
    event: pg_sys::XactEvent::Type,
    _arg: *mut std::ffi::c_void,
) {
    use pg_sys::XactEvent;
    match event {
        XactEvent::XACT_EVENT_PRE_PREPARE => {
            if TRANSACTION_API_DIRTY.load(Ordering::Relaxed) {
                pgrx::error!("cannot PREPARE a transaction that changed pg_tokenizer definitions");
            }
        }
        XactEvent::XACT_EVENT_COMMIT | XactEvent::XACT_EVENT_PARALLEL_COMMIT => {
            VALIDATED_SNAPSHOT.with(|key| key.set(None));
            TRANSACTION_API_DIRTY.store(false, Ordering::Relaxed);
            if TRANSACTION_DIRTY.swap(false, Ordering::Relaxed) {
                clear_local_caches();
                DIRTY_SUBTRANSACTIONS.with(|dirty| dirty.borrow_mut().clear());
                let fxid = pg_sys::GetTopFullTransactionIdIfAny().value;
                publish_barrier_and_bump_epoch(fxid);
                LOCAL_CACHE_EPOCH
                    .store(CACHE_EPOCH.get().load(Ordering::Acquire), Ordering::Relaxed);
            }
            TRANSACTION_UNSAFE_LEGACY.store(false, Ordering::Relaxed);
            if TRANSACTION_SAW_EPOCH.swap(false, Ordering::Relaxed) {
                clear_local_caches();
            }
        }
        XactEvent::XACT_EVENT_ABORT
        | XactEvent::XACT_EVENT_PARALLEL_ABORT
        | XactEvent::XACT_EVENT_PREPARE => {
            VALIDATED_SNAPSHOT.with(|key| key.set(None));
            let was_dirty = TRANSACTION_DIRTY.swap(false, Ordering::Relaxed);
            TRANSACTION_API_DIRTY.store(false, Ordering::Relaxed);
            let was_legacy = TRANSACTION_UNSAFE_LEGACY.swap(false, Ordering::Relaxed);
            let saw_epoch = TRANSACTION_SAW_EPOCH.swap(false, Ordering::Relaxed);
            if was_dirty {
                DIRTY_SUBTRANSACTIONS.with(|dirty| dirty.borrow_mut().clear());
            }
            if was_dirty || was_legacy || saw_epoch {
                clear_local_caches();
            }
        }
        _ => {}
    }
}

#[pgrx::pg_guard]
unsafe extern "C-unwind" fn on_subxact_event(
    event: pg_sys::SubXactEvent::Type,
    my_subid: pg_sys::SubTransactionId,
    parent_subid: pg_sys::SubTransactionId,
    _arg: *mut std::ffi::c_void,
) {
    use pg_sys::SubXactEvent;
    match event {
        SubXactEvent::SUBXACT_EVENT_ABORT_SUB => {
            VALIDATED_SNAPSHOT.with(|key| key.set(None));
            let was_dirty = TRANSACTION_DIRTY.load(Ordering::Relaxed)
                && DIRTY_SUBTRANSACTIONS.with(|dirty| {
                    let mut dirty = dirty.borrow_mut();
                    let removed = dirty.remove(&my_subid).is_some();
                    TRANSACTION_DIRTY.store(!dirty.is_empty(), Ordering::Relaxed);
                    removed
                });
            if was_dirty || TRANSACTION_UNSAFE_LEGACY.load(Ordering::Relaxed) {
                clear_local_caches();
            }
        }
        SubXactEvent::SUBXACT_EVENT_COMMIT_SUB if TRANSACTION_DIRTY.load(Ordering::Relaxed) => {
            DIRTY_SUBTRANSACTIONS.with(|dirty| {
                let mut dirty = dirty.borrow_mut();
                if let Some(pending) = dirty.remove(&my_subid) {
                    *dirty.entry(parent_subid).or_default() += pending;
                }
            });
        }
        _ => {}
    }
}

#[inline]
fn ensure_callbacks_registered() {
    if CALLBACKS_REGISTERED.load(Ordering::Relaxed) {
        return;
    }
    register_callbacks_once();
}

#[cold]
fn register_callbacks_once() {
    if CALLBACKS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        pg_sys::RegisterXactCallback(Some(on_xact_event), std::ptr::null_mut());
        pg_sys::RegisterSubXactCallback(Some(on_subxact_event), std::ptr::null_mut());
        pg_sys::CacheRegisterRelcacheCallback(Some(on_relcache_inval), pg_sys::Datum::from(0usize));
        pg_sys::CacheRegisterSyscacheCallback(
            pg_sys::SysCacheIdentifier::NAMESPACENAME as SysCacheId,
            Some(on_syscache_inval),
            pg_sys::Datum::from(0usize),
        );
        pg_sys::CacheRegisterSyscacheCallback(
            EXTENSION_CACHE_ID,
            Some(on_syscache_inval),
            pg_sys::Datum::from(0usize),
        );
        pg_sys::CacheRegisterSyscacheCallback(
            pg_sys::SysCacheIdentifier::PROCOID as SysCacheId,
            Some(on_syscache_inval),
            pg_sys::Datum::from(0usize),
        );
    }
}

fn clear_local_caches() {
    VALIDATED_SNAPSHOT.with(|key| key.set(None));
    CATALOG_VERSION.with(|ver| ver.set(None));
    LOCAL_CLEAN.with(|clean| clean.set((0, None)));
    crate::model::clear_cache();
    crate::text_analyzer::clear_cache();
    crate::token_filter::clear_caches();
    crate::tokenizer::clear_cache();
}

fn resolve_catalog_identity() -> Option<(u64, pg_sys::Oid, bool, Rc<KeptPlan>)> {
    let gen = CATALOG_IDENTITY_GEN.load(Ordering::Acquire);
    let cached = CATALOG_IDENTITY.with(|slot| {
        let borrow = slot.borrow();
        borrow.as_ref().filter(|id| id.gen == gen).map(|id| {
            (
                id.gen,
                id.def_ver_oid,
                id.trigger_ready,
                Rc::clone(&id.version_plan),
            )
        })
    });
    if cached.is_some() {
        return cached;
    }

    // Re-verify extension ownership of definition_version and _notify_catalog_changed.
    let namespace = unsafe { pg_sys::get_namespace_oid(c"tokenizer_catalog".as_ptr(), true) };
    if namespace == pg_sys::InvalidOid {
        CATALOG_IDENTITY.with(|slot| *slot.borrow_mut() = None);
        CACHED_DEF_VER_OID.store(0, Ordering::Relaxed);
        return None;
    }
    let relation = unsafe { pg_sys::get_relname_relid(c"definition_version".as_ptr(), namespace) };
    if relation == pg_sys::InvalidOid {
        // Pre-1.1.0 extension SQL; do not cache negative lookup so an upgrade is seen immediately.
        CATALOG_IDENTITY.with(|slot| *slot.borrow_mut() = None);
        CACHED_DEF_VER_OID.store(0, Ordering::Relaxed);
        return None;
    }
    let extension = unsafe { pg_sys::get_extension_oid(c"pg_tokenizer".as_ptr(), true) };
    if extension == pg_sys::InvalidOid
        || unsafe { pg_sys::getExtensionOfObject(pg_sys::RelationRelationId, relation) }
            != extension
    {
        // An unowned counter cannot validate definitions across snapshots or COMMIT PREPARED.
        TRANSACTION_SAW_EPOCH.store(true, Ordering::Relaxed);
        CATALOG_IDENTITY.with(|slot| *slot.borrow_mut() = None);
        CACHED_DEF_VER_OID.store(0, Ordering::Relaxed);
        return None;
    }

    // Check whether the 1.3.0+ notification helper is installed and owned by pg_tokenizer.
    let trigger_ready = unsafe {
        let empty_args = pg_sys::buildoidvector(std::ptr::null(), 0);
        let func_oid = pg_sys::GetSysCacheOid(
            pg_sys::SysCacheIdentifier::PROCNAMEARGSNSP as SysCacheId,
            pg_sys::Anum_pg_proc_oid as pg_sys::AttrNumber,
            pg_sys::Datum::from(c"_notify_catalog_changed".as_ptr()),
            pg_sys::Datum::from(empty_args),
            pg_sys::Datum::from(namespace),
            pg_sys::Datum::from(0usize),
        );
        pg_sys::pfree(empty_args.cast());
        func_oid != pg_sys::InvalidOid
            && pg_sys::getExtensionOfObject(pg_sys::ProcedureRelationId, func_oid) == extension
    };

    CACHED_DEF_VER_OID.store(u32::from(relation), Ordering::Relaxed);
    let plan = Rc::new(KeptPlan::prepare(
        c"SELECT version FROM tokenizer_catalog.definition_version WHERE singleton",
        &[],
    ));

    // Only publish if no concurrent invalidation raced while preparing.
    let gen_after = CATALOG_IDENTITY_GEN.load(Ordering::Acquire);
    if gen_after == gen && !membership_ddl_active() {
        CATALOG_IDENTITY.with(|slot| {
            *slot.borrow_mut() = Some(CatalogIdentity {
                gen,
                def_ver_oid: relation,
                trigger_ready,
                version_plan: Rc::clone(&plan),
            });
        });
    }
    Some((gen, relation, trigger_ready, plan))
}

#[inline]
fn fast_path_valid() -> bool {
    if TRANSACTION_DIRTY.load(Ordering::Relaxed)
        || TRANSACTION_SAW_EPOCH.load(Ordering::Relaxed)
        || TRANSACTION_UNSAFE_LEGACY.load(Ordering::Relaxed)
    {
        return false;
    }
    if unsafe { pg_sys::RecoveryInProgress() } {
        return false;
    }
    if !unsafe { pg_sys::ActiveSnapshotSet() } {
        return false;
    }
    let snapshot = unsafe { pg_sys::GetActiveSnapshot() };
    if unsafe { (*snapshot).takenDuringRecovery } {
        return false;
    }
    let identity_gen = CATALOG_IDENTITY_GEN.load(Ordering::Acquire);
    let (clean_gen, clean_epoch) = LOCAL_CLEAN.with(|c| c.get());
    let Some(clean_epoch) = clean_epoch else {
        return false;
    };
    if clean_gen != identity_gen {
        return false;
    }
    let shared_epoch = CACHE_EPOCH.get().load(Ordering::Acquire);
    if shared_epoch != clean_epoch {
        return false;
    }
    let barrier = CHANGE_BARRIER_FXID.get().load(Ordering::Acquire);
    let xmin = unsafe { (*snapshot).xmin.into_inner() };
    xid_at_or_after_barrier(xmin, barrier)
}

fn check_catalog_version() -> bool {
    if !unsafe { pg_sys::ActiveSnapshotSet() } {
        return false;
    }
    let snapshot = unsafe { pg_sys::GetActiveSnapshot() };
    let in_recovery = unsafe { pg_sys::RecoveryInProgress() };
    let taken_during_recovery = unsafe { (*snapshot).takenDuringRecovery };
    let xmin = unsafe { (*snapshot).xmin.into_inner() };
    let xmax = unsafe { (*snapshot).xmax.into_inner() };
    let worker = unsafe { pg_sys::ParallelWorkerNumber } >= 0;
    let statement = unsafe { pg_sys::GetCurrentStatementStartTimestamp() };
    let snapshot_command = unsafe { (*snapshot).curcid };

    // Fast within-statement reuse on the slow path without mutating currentCommandIdUsed.
    let read_command = unsafe { pg_sys::GetCurrentCommandId(false) };
    let current_gen = CATALOG_IDENTITY_GEN.load(Ordering::Acquire);
    let probe_key = SnapshotKey {
        statement,
        command: read_command,
        snapshot: snapshot as usize,
        snapshot_command,
        snapshot_xmin: xmin,
        snapshot_xmax: xmax,
        identity_gen: current_gen,
        recovery: in_recovery,
    };
    if VALIDATED_SNAPSHOT.with(|cached| cached.get() == Some(probe_key)) {
        return true;
    }

    let Some((identity_gen, relation, trigger_ready, version_plan)) = resolve_catalog_identity()
    else {
        if CATALOG_VERSION.with(|ver| ver.get().is_some()) {
            clear_local_caches();
        }
        return false;
    };

    // Sample shared epoch BEFORE barrier (reverse of writer's barrier-then-epoch
    // order) and BEFORE executing the version read so any concurrent writer is
    // guaranteed to be detected.
    let (epoch_before, barrier_before) = if !in_recovery {
        let epoch = CACHE_EPOCH.get().load(Ordering::Acquire);
        let mut barrier = ensure_barrier_initialized();
        if !xid_at_or_after_barrier(xmin, barrier) {
            barrier = maybe_retire_ancient_barrier(barrier);
        }
        (epoch, barrier)
    } else {
        (0, BARRIER_UNINITIALIZED)
    };

    let version = version_plan
        .execute_one::<i64>(&[], true)
        .unwrap_or_else(|| {
            // Syscache can see an upgrade committed after the caller's snapshot.
            let visible = crate::utils::catalog_one_cstr::<bool>(
                c"SELECT EXISTS (SELECT FROM pg_catalog.pg_class WHERE oid = $1)",
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
    let validating_rows = TRANSACTION_DIRTY.load(Ordering::Relaxed)
        && DIRTY_SUBTRANSACTIONS.with(|dirty| dirty.borrow().values().any(|pending| *pending > 0));
    if changed || validating_rows {
        clear_local_caches();
        CATALOG_VERSION.with(|cached| cached.set(Some(identity)));
    }

    // Mark read-only command ID used ONCE per validated statement (not per row)
    // so multiple statements in a single protocol message never reuse a key.
    let marked_command = unsafe { pg_sys::GetCurrentCommandId(!worker) };
    let validated_key = SnapshotKey {
        command: marked_command,
        identity_gen,
        ..probe_key
    };
    let in_membership_ddl = membership_ddl_active();
    if !in_membership_ddl {
        VALIDATED_SNAPSHOT.with(|cached| cached.set(Some(validated_key)));
    }

    // Un-upgraded 1.1.0/1.2.0 SQL lacks the trigger notification hook, so
    // direct DML could dirty this backend without setting TRANSACTION_DIRTY.
    if !in_recovery && !trigger_ready {
        TRANSACTION_UNSAFE_LEGACY.store(true, Ordering::Relaxed);
        LOCAL_CLEAN.with(|c| c.set((0, None)));
        return true;
    }

    // Promote to the zero-SPI primary fast path only when the cache is provably
    // consistent with every snapshot at or after barrier_before.
    let can_use_fast_path = !in_recovery
        && !taken_during_recovery
        && trigger_ready
        && !in_membership_ddl
        && !TRANSACTION_DIRTY.load(Ordering::Relaxed)
        && !TRANSACTION_SAW_EPOCH.load(Ordering::Relaxed)
        && xid_at_or_after_barrier(xmin, barrier_before)
        && CACHE_EPOCH.get().load(Ordering::Acquire) == epoch_before
        && CATALOG_IDENTITY_GEN.load(Ordering::Acquire) == identity_gen;

    if can_use_fast_path {
        LOCAL_CLEAN.with(|c| c.set((identity_gen, Some(epoch_before))));
    } else {
        LOCAL_CLEAN.with(|c| c.set((0, None)));
    }
    true
}

fn check_legacy_epoch() {
    let shared_epoch = CACHE_EPOCH.get().load(Ordering::SeqCst);
    if LOCAL_CACHE_EPOCH.load(Ordering::SeqCst) != shared_epoch {
        clear_local_caches();
        LOCAL_CACHE_EPOCH.store(shared_epoch, Ordering::SeqCst);
        TRANSACTION_SAW_EPOCH.store(true, Ordering::SeqCst);
    }
    let parallel_worker = unsafe { pg_sys::ParallelWorkerNumber } >= 0;
    if TRANSACTION_SAW_EPOCH.load(Ordering::SeqCst) && !parallel_worker {
        let statement = unsafe { pg_sys::GetCurrentStatementStartTimestamp() };
        let command = unsafe { pg_sys::GetCurrentCommandId(true) };
        let changed_statement = LAST_STATEMENT.swap(statement, Ordering::SeqCst) != statement;
        let changed_command = LAST_COMMAND.swap(command, Ordering::SeqCst) != command;
        if changed_statement || changed_command {
            clear_local_caches();
        }
    }
}

#[inline]
pub fn check() {
    ensure_callbacks_registered();
    if fast_path_valid() {
        return;
    }
    check_slow();
}

#[cold]
fn check_slow() {
    if check_catalog_version() {
        return;
    }
    check_legacy_epoch();
}

enum DefinitionEvent {
    BeginStatement,
    EndStatement,
    Changed,
    Api,
}

fn mark_changed(event: DefinitionEvent) {
    ensure_callbacks_registered();
    clear_local_caches();

    let subid = unsafe { pg_sys::GetCurrentSubTransactionId() };
    DIRTY_SUBTRANSACTIONS.with(|dirty| {
        let mut dirty = dirty.borrow_mut();
        let pending = dirty.entry(subid).or_default();
        match event {
            DefinitionEvent::BeginStatement => *pending += 1,
            DefinitionEvent::EndStatement => *pending = pending.saturating_sub(1),
            DefinitionEvent::Changed | DefinitionEvent::Api => {}
        }
    });
    TRANSACTION_DIRTY.store(true, Ordering::Relaxed);
    if matches!(event, DefinitionEvent::BeginStatement) {
        return;
    }

    // Publish the XID barrier and bump the shared epoch BEFORE commit so no
    // concurrent snapshot can observe the committed change via ProcArray before
    // noticing the invalidation, and so 2PC prepared transactions automatically
    // keep concurrent readers on the slow path until COMMIT/ROLLBACK PREPARED.
    let fxid = unsafe { pg_sys::GetTopFullTransactionIdIfAny().value };
    publish_barrier_and_bump_epoch(fxid);

    if matches!(event, DefinitionEvent::Api) {
        TRANSACTION_API_DIRTY.store(true, Ordering::Relaxed);
    }
}

pub fn mark_catalog_changed() {
    mark_changed(DefinitionEvent::Api);
}

#[pgrx::pg_extern(volatile, parallel_unsafe)]
fn _notify_catalog_changed() {
    mark_changed(DefinitionEvent::EndStatement);
}

#[pgrx::pg_trigger]
fn _notify_definition_version_changed<'a>(
    trigger: &'a pgrx::PgTrigger<'a>,
) -> Result<Option<pgrx::prelude::PgHeapTuple<'a, pgrx::AllocatedByPostgres>>, ()> {
    mark_changed(if trigger.event().fired_before() {
        DefinitionEvent::BeginStatement
    } else {
        DefinitionEvent::Changed
    });
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{advance_fxid, xid_at_or_after_barrier, BARRIER_EXPIRED, BARRIER_UNINITIALIZED};

    #[test]
    fn advance_fxid_skips_special_xids_on_wraparound() {
        assert_eq!(advance_fxid(3), 4);
        assert_eq!(advance_fxid(0xFFFF_FFFE), 0xFFFF_FFFF);
        assert_eq!(advance_fxid(0xFFFF_FFFF), 0x1_0000_0003);
        assert_eq!(advance_fxid(0x1_FFFF_FFFF), 0x2_0000_0003);
    }

    #[test]
    fn xid_barrier_checks_normal_and_wraparound_snapshots() {
        assert!(!xid_at_or_after_barrier(100, BARRIER_UNINITIALIZED));
        assert!(!xid_at_or_after_barrier(0, 100));
        assert!(!xid_at_or_after_barrier(2, 100));
        assert!(xid_at_or_after_barrier(100, BARRIER_EXPIRED));

        // Normal ordering: snapshot xmin must be >= barrier.
        assert!(!xid_at_or_after_barrier(999, 1000));
        assert!(xid_at_or_after_barrier(1000, 1000));
        assert!(xid_at_or_after_barrier(1001, 1000));

        // Imported old snapshot well behind barrier.
        assert!(!xid_at_or_after_barrier(500, 1_000_500));
        assert!(xid_at_or_after_barrier(1_000_500, 1_000_500));

        // Across 32-bit XID wraparound (0xFFFF_FF00 -> 0x1_0000_0100).
        let barrier_before_wrap = 0xFFFF_FFF0u64;
        assert!(!xid_at_or_after_barrier(0xFFFF_FFEF, barrier_before_wrap));
        assert!(xid_at_or_after_barrier(0xFFFF_FFF0, barrier_before_wrap));
        assert!(xid_at_or_after_barrier(3, barrier_before_wrap));
        assert!(xid_at_or_after_barrier(1000, barrier_before_wrap));

        let barrier_after_wrap = 0x1_0000_0010u64;
        assert!(!xid_at_or_after_barrier(0xFFFF_FFF0, barrier_after_wrap));
        assert!(!xid_at_or_after_barrier(0x0000_000F, barrier_after_wrap));
        assert!(xid_at_or_after_barrier(0x0000_0010, barrier_after_wrap));
        assert!(xid_at_or_after_barrier(0x0000_0020, barrier_after_wrap));
    }
}
