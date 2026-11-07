# pg_tokenizer 1.0.1 — final adversarial review and release readiness

2026-11-08 (Asia/Shanghai). **Local release preparation accepted.** The reviewed implementation is merged into local `main` at `c69f5f2e49f1732a11a944fa75eda28d05bb20b4`. A clean build from that commit passed the acceptance below. No confirmed correctness blocker remains within the tested PostgreSQL 18.6 / macOS arm64 scope. Publishing still requires the Linux/PostgreSQL release matrix; local acceptance is not evidence that that matrix passed.

No push, tag, release, package upload, shared-server installation, or site deployment was performed. The report/evidence commit following the merge changes only `reports/`; it does not change the tested implementation. The companion documentation commit is `85df61d`.

## Final findings and disposition

1. **Fixed: a legacy dictionary could silently change meaning during upgrade.** The phase-3 candidate preferred `pg_catalog.simple` when a custom `simple` also existed. The original creator's search_path cannot be recovered, so that preference was unjustified. Commit `47ed49f` makes both lazy loading and migration reject every ambiguous unqualified name. The administrator must explicitly choose the intended schema. Missing names also fail, and failed upgrades retain extension version 1.0.0 transactionally.
2. **Fixed: the new shadowing regression could mask the bug.** A second ambiguous dictionary caused the same test to fail first. The final test checks lazy analyzer and inline-tokenizer loads separately, then checks each migration branch independently. A custom `simple` with `ACCEPT=false` proves that explicitly choosing it preserves `UPPER` and model ID 7 instead of switching to the builtin's lowercase result.
3. **Documentation reconciled.** Repository and bilingual website documentation now agree on unique-name migration, restart/replay ordering, Lindera server-file privileges, cache snapshot semantics and resource-budget limits. Jieba `full`/`precise` mappings are corrected; users of either mode in 1.0.0 or earlier must regenerate persisted output and rebuild dependent indexes.

| Issue | Final assessment | Tracking |
| --- | --- | --- |
| [#10](https://github.com/pgsty/pg_tokenizer/issues/10) | Fixed locally. WAL-replicated version checks and definition loads share the active snapshot; long-lived standby sessions refresh after replay and after promotion. | Close as locally completed; commits are not yet published. |
| [#13](https://github.com/pgsty/pg_tokenizer/issues/13) | Partially mitigated. Input, token-count, accounted-byte and opaque-call input budgets plus interrupt checks constrain extension-owned work. | Remains open: third-party allocations, regex/library work and cancellation latency are not hard-bounded. USERSET budgets are not a resource security boundary. |
| [#14](https://github.com/pgsty/pg_tokenizer/issues/14) | Catalog writer parallel markings and dictionary namespace/OID handling are fixed, alongside the earlier configuration/ID/preload fixes. | Remains open for runtime-schema separation: database owners still need CREATE on `tokenizer_catalog`. Windows execution has not been validated here. |
| [#15](https://github.com/pgsty/pg_tokenizer/issues/15) | Local review, batch commits, merge and acceptance completed. | Remains open for publication, release-matrix validation and the remaining scoped work. |

The intentional operational costs remain: definition-changing transactions serialize on one version row, old SQL snapshots retain their old definitions, and persisted vectors/indexes are not rewritten automatically. Phase-3 performance measurements in [REPORT.md](REPORT.md) belong to its recorded candidate, not a new benchmark of the final merge.

## Acceptance from merged main

Environment: PostgreSQL 18.6 (Homebrew), Rust 1.99.1, macOS arm64, features `pg18 lindera-ipadic`. Build and database directories were new under `/tmp/pg-tokenizer-final-main-20261108`. No previous database cluster was reused. The acceptance manifest records a clean `main` at the source commit above, exact library hashes, staged SQL hashes, control paths, command results and cluster cleanup.

| Check | Result / evidence |
| --- | --- |
| Fresh optimized build and generated SQL | Passed; [build](evidence/final-main/build.log). SQL matches the checked-in script except for the order of five independent catalog-table declarations and trailing whitespace. All other SQL is compared exactly. |
| Rust unit tests | 31 passed; [unit](evidence/final-main/unit.log). |
| Clippy, formatting, metadata | `-D warnings`, Rustfmt and Taplo passed; [Clippy](evidence/final-main/clippy.log), [static checks](evidence/final-main/static-checks.json). |
| Integration | 41 passed, including upgrade chains, permissions, concurrent writes, snapshot/abort behavior, dictionary ambiguity/DDL and dump/restore; [integration](evidence/final-main/integration.log). |
| SQLLogicTest | All 22 files passed with explicit private socket and port; [SQLLogicTest](evidence/final-main/slt.log). |
| Physical standby | Replay barriers, all five definition classes and dependent caches, old snapshots, rollback, promotion and immediate-stop recovery passed; [standby](evidence/final-main/standby.log). |
| Preload/restart | Invalid entries, database identity and Lindera file-permission boundaries passed; [preload](evidence/final-main/preload.log). |
| Joint extensions | All six preload orders passed, including forced vchordrq/vchordg and BM25 index plans/results; [orders](evidence/final-main/combo-orders.log), [hashes](evidence/final-main/combo-final.json). |
| Companion site | `make check` passed with both committed OINK 1.5.0 and the existing working dependency 1.5.1; both had zero missing targets/fragments. [1.5.0](evidence/final-main/site-check-1.5.0.log), [1.5.1](evidence/final-main/site-check-1.5.1.log), [inputs](evidence/final-main/site-inputs.json). |

Joint acceptance uses frozen vchord `77e7bf393356cce7d722894db5880626ed5800f9`, vchord_bm25 `8ea553270373f5270886cf5530f6134bb65b47bc`, and locally installed pgvector 0.8.7. It does not validate later changes in those repositories. Their frozen dylib hashes are asserted by the runner. Tokenizer SHA256: `ad8a2cf5d3a7e38671728d0c333fc9ef144878aa2315c2a85edb5df5a8513058`.

All disposable clusters stopped and were removed. Full machine-readable evidence: [acceptance.json](evidence/final-main/acceptance.json). The six website pages were committed separately; unrelated pre-existing website edits were checked byte-for-byte and preserved. `AGENTS.md` was not committed.

## Reproduction

Build from the intended candidate into a new artifact directory. The runner is specifically for this macOS/PG18 validation setup; its peer directory must contain the frozen libraries and SQL whose hashes it checks. Use a Python environment with `psycopg`, and put `pg_config`, PostgreSQL tools and `sqllogictest` on PATH.

```bash
export PGRX_PG_CONFIG_PATH=/opt/homebrew/bin/pg_config
export CARGO_TARGET_DIR=/tmp/pg-tokenizer-final-main-20261108/target
cargo pgrx schema --release --cargo=--locked --features 'pg18 lindera-ipadic' \
  --out /tmp/pg-tokenizer-final-main-20261108/schema.sql
cargo test --locked --no-fail-fast --features 'pg18 lindera-ipadic'
cargo clippy --locked --all-targets --features 'pg18 lindera-ipadic' -- -D warnings
/tmp/pg-tokenizer-phase3-20261107/venv/bin/python \
  reports/phase3-1.0.1/scripts/final_acceptance.py \
  /tmp/pg-tokenizer-final-main-20261108 "$PWD" /tmp/pg-tokenizer-phase3-20261107
```

The runner stages generated install SQL, all historical install/upgrade scripts, and control files whose module paths bind the exact staged libraries. A first pre-merge attempt omitted that binding for the preload test and failed; it is excluded from acceptance. The corrected runner passed before and after merging. The site's first check found a modified global Go module cache, so final checks extracted verified cached module ZIPs into a private cache and left the shared cache untouched.

## Publication gate

Cargo/Cargo.lock are 1.0.1, the control file intentionally uses `@CARGO_VERSION@` for package substitution, and matching installation SQL plus the `0.1.0 → 0.1.1 → 1.0.0 → 1.0.1` migration chain are present. Historical install SQL is unchanged.

Before publishing, run the configured Check workflow on the chosen release commit across PostgreSQL 14–18 and Linux x86_64/aarch64, then build/review the release packages from that same commit. Windows and other optional dictionaries are outside this local validation. The release workflow already verifies the commit's successful Check run and Cargo/install-SQL version before packaging. Upgrade notes must retain the Jieba regeneration requirement, ambiguous-dictionary qualification, replica restarts/replay, and the unresolved #13/#14 boundaries.
