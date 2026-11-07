# pg_tokenizer phase 3 — 1.0.1 implementation and validation

2026-11-07, macOS arm64, PostgreSQL 18.6 (Homebrew), Rust 1.99.1, features `pg18 lindera-ipadic`.

Implementation commit: `cddf6ac2969364f34c4029fea60367f90cafa890`, based on `4d955eb499cd42ae54183577a368e81b54c91f27` in `claude/review-fixes-20261106`. This is a local release candidate; no tag, push, PR, issue comment or issue closure was made. The tokenizer main checkout was not modified.

## Delivered behavior and scope

| Issue | Result | Remaining boundary |
| --- | --- | --- |
| #10 | Definitions and their version are read under the same active SQL snapshot. Physical standby replay and promotion refresh existing sessions. | Old snapshots and lagging replicas intentionally retain their older view. Definition writes serialize until commit. |
| #14 | Twelve remaining catalog writers are `PARALLEL UNSAFE`; `pg_dict` stores a qualified name and refreshes dictionary OIDs after DDL. | Database owners retain schema CREATE. Moving runtime objects into another schema is deferred. |
| #13 | Configurable input, output-count, accounted-byte and opaque-call input budgets raise `54000` without truncation. Extension-owned expansion loops are checked. | Partial mitigation only: opaque third-party allocations and cancellation latency are not hard-bounded. |

The patch includes Rust exports, generated 1.0.1 installation SQL, the 1.0.0 → 1.0.1 migration, regression tests, CI additions, README/reference/upgrade documentation, and six bilingual tokenizer website pages. Historical installation scripts are unchanged. SQL signatures, vocabulary IDs and persisted storage formats are preserved.

## Why the cache implementation changed

The primary's shared-memory epoch cannot travel through physical WAL replay. The new singleton `tokenizer_catalog.definition_version` row is updated by AFTER STATEMENT triggers on the five definition tables: analyzers, tokenizers, models, stopwords and synonyms. Its changes commit, roll back and replicate with those definitions. Vocabulary inserts do not update it.

A backend probes the version once per statement/snapshot validation key, then reuses that validation for dependent lookups. A different version in either direction clears derived objects, so an older snapshot can reload older definitions as well as a new snapshot loading new ones. The key includes statement timestamp, command ID, active snapshot identity/command ID and recovery state. Parallel workers never call `GetCurrentCommandId(true)`.

Two existing SPI behaviors needed correction. pgrx 0.19.3 `select()` can switch to read-write SPI after a transaction has acquired an XID, which takes a newer snapshot. Definition reads now use `SPI_execute_with_args(..., read_only=true)` and copy owned results before SPI cleanup. Catalog mutations using a one-row RETURNING limit could stop execution before AFTER STATEMENT triggers; their executor limits were removed while retaining the unique-key predicates.

Top-level and subtransaction aborts clear cached objects. This is necessary even for administrative direct DML: a version loaded from an aborted transaction can later be reused by another writer with different definitions. The regression deterministically reproduced that stale-object reuse in an intermediate build, then passed with abort cleanup. API calls still reject preparing transactions that change definitions. Old installed SQL continues to use the legacy epoch until the upgrade commits and replays.

The version relation must belong to the extension; a pre-existing unrelated relation is not adopted. PUBLIC receives only SELECT on the version row. The internal trigger is SECURITY DEFINER with fixed `search_path=pg_catalog`, and its EXECUTE privilege is revoked from PUBLIC and `pg_database_owner`. Upgrade collisions fail transactionally. The version row is excluded from config dumps because restoring definitions increments a fresh counter.

## Dictionary and budget behavior

New `pg_dict` definitions resolve under the creator's search_path and persist quoted schema-qualified names. Each batch resolves the name through PostgreSQL's caches. DROP/CREATE finds the new OID; ALTER relies on PostgreSQL's text-search dictionary invalidation. Rename or schema moves require updating the stored reference.

Upgrade cannot reconstruct the original creator's search_path. An unqualified legacy name resolves to a matching `pg_catalog` dictionary first, otherwise to a uniquely named custom dictionary. Missing or ambiguous names abort with guidance. Deliberately shadowed builtin names must be qualified before upgrading.

| USERSET GUC | Default |
| --- | ---: |
| `pg_tokenizer.max_input_bytes` | 16 MiB |
| `pg_tokenizer.max_output_tokens` | 1,000,000 |
| `pg_tokenizer.max_output_bytes` | 64 MiB |
| `pg_tokenizer.max_piece_bytes` | 1 MiB |

The checks cover normalization output, pre-tokenizer copies, ngram offsets/strings, token-filter batches, model IDs, custom token splitting, dictionary copies, and HF fixed padding/padding multiples. Accounted token bytes include string descriptors. Limits are captured once for each output accumulator. Lowercasing retains Rust's contextual final-sigma behavior. New bulk loops include interrupt checks.

These are admission and per-stage budgets, not an RSS cap or a resource security boundary. Detoasting happens before entry. Allocator spare capacity, overlapping buffers, cached models and library workspace add memory. Arbitrary HF normalizers/post-processors/truncation overflow may allocate before output checks. A single regex search, library call or dictionary call can delay cancellation until it returns. No dependency fork, asynchronous cancellation, or semantics-changing input splitting was introduced. #13 therefore remains incomplete.

## Deterministic before/after evidence

| Probe | Before | Current |
| --- | --- | --- |
| Writer catalog flags | Twelve create/drop functions were marked SAFE. | All catalog writers are UNSAFE; planner check passes. |
| Dictionary namespace | A reader with another search_path got `running` instead of the creator's `run`. | Qualified reference returns `run`. |
| Dictionary DROP/CREATE | After two warmup statements, the frozen baseline fails with `cache lookup failed for text search dictionary`. | Same reader uses the replacement dictionary; ALTER is also observed. |
| Analysis budgets | Five input/count/byte/piece subcases do not raise `54000`; HF fixed padding is also admitted. | Each oversized case raises `54000`; the connection remains usable. |
| Snapshot after prior write | A long-running statement sees IDs from a concurrently committed replacement. | The old statement retains old IDs; its following statement refreshes. |
| Physical replay | An existing standby session returns `UPPER` after the lowercase definition has replayed. | Existing sessions and dependent tokenizers refresh at the replay barrier; old RR snapshots retain old definitions. |
| Direct-DML abort/version reuse | Intermediate build `6da6e364…` returns an aborted lowercase definition after another writer commits the same version with ngrams. | Returns the committed ngrams after abort cleanup. |

See [baseline failures](evidence/regressions-before.log), [dictionary OID failure](evidence/dictionary-oid-before.log), [all four GUCs before](evidence/budgets-before.log) / [after](evidence/budgets-after.log), [standby failure](evidence/standby-before.log), and [direct-DML abort failure](evidence/direct-dml-abort-before.log). The dictionary test deliberately warms two statements: one statement was insufficient because the legacy epoch path discarded that first cache.

## Validation on the final library

- [40 integration tests passed](evidence/integration.log), including upgrade compatibility, owner permissions, parallel plans, dictionary DDL, long snapshots, direct-DML rollback and dump/restore. The additional piece-limit subcase subsequently passed in the targeted [budget test](evidence/budgets-after.log).
- [31 Rust unit tests passed](evidence/unit.log); [Clippy with `-D warnings`](evidence/clippy.log), `cargo fmt --check`, Taplo 0.10.0 `fmt --check`, and `git diff --check` passed.
- [All 22 SQLLogicTest files passed](evidence/slt.log), including cancellation and tsvector aggregation. Those large-workload tests explicitly raise the relevant admission limits so they still exercise cancellation/aggregation.
- [Physical standby test passed](evidence/standby.log): all five definition classes and dependent caches, replay pause/resume with LSN barriers, long-lived sessions, old RR snapshots, rollback, promotion, immediate-stop recovery.
- [Preload test passed](evidence/preload.log): malformed entries, restart, cross-database identity and Lindera file permissions.
- [Six preload orders passed](evidence/combo-orders.log) with the final tokenizer library. The [workload](scripts/combo.sql) sends analyzer lexemes and BERT token IDs through native tsvector into separate BM25 indexes. It checks both BM25 plans and results, VACUUM/maintenance, and separately forces vchordrq and vchordg index scans. The [runner](scripts/joint_acceptance.py) uses its own disposable cluster and asserts the frozen input hashes and SQL versions.
- [Website `make check` passed](evidence/site-check.log) in an isolated working-file copy: zero missing internal targets or fragments. Existing warnings concerned absent Git metadata in the copy and deprecated libsass. The six tokenizer changes are preserved as a [separate patch](evidence/tokenizer-site.patch); unrelated site edits remain untouched and uncommitted.

Generated installation SQL matches the final pgrx output after removing the extra terminal blank line. CI now stages all historical install scripts and runs the physical standby test. Linux, Windows, other PostgreSQL majors and other optional dictionaries were not run in this local validation; CI configuration is not evidence that its matrix has run.

The commands below reproduce the build and standalone cluster checks from this checkout. Set `PGRX_PG_CONFIG_PATH` to the selected installation's `pg_config` and put that installation on PATH for the Python tests; all build outputs stay in the private artifact root. Re-running the integration tests also requires starting the owned `primary` cluster and using its recorded DSN. SQLLogicTest requires a database where the matching extension is already created and explicit `--host`/`--port` arguments.

```bash
export CARGO_TARGET_DIR=/tmp/pg-tokenizer-phase3-20261107/target
export PGRX_PG_CONFIG_PATH=/opt/homebrew/bin/pg_config
cargo pgrx schema --release --cargo=--locked --features 'pg18 lindera-ipadic' \
  --out /tmp/pg-tokenizer-phase3-20261107/schema.sql
cargo clippy --locked --all-targets --features 'pg18 lindera-ipadic' -- -D warnings
cargo test --locked --no-fail-fast --features 'pg18 lindera-ipadic'
export PG_TOKENIZER_TEST_LIBRARY=/tmp/pg-tokenizer-phase3-20261107/current/lib/pg_tokenizer.dylib
export PG_TOKENIZER_TEST_SHAREDIR=/tmp/pg-tokenizer-phase3-20261107/current/share
/tmp/pg-tokenizer-phase3-20261107/venv/bin/python tests/test_standby.py
/tmp/pg-tokenizer-phase3-20261107/venv/bin/python tests/test_preload.py
/tmp/pg-tokenizer-phase3-20261107/venv/bin/python \
  reports/phase3-1.0.1/scripts/joint_acceptance.py /tmp/pg-tokenizer-phase3-20261107
```

## Measured cost

The [paired benchmark](scripts/benchmark.py) uses warm caches, varying text, no JIT or query parallelism, seven alternating batch samples and three individual-call samples. These are executor/wall-clock measurements on one shared workstation, not isolated CPU profiles or production throughput predictions. Full samples are in [benchmark.json](evidence/benchmark.json).

| Workload | Frozen baseline median | Current median |
| --- | ---: | ---: |
| Simple analyzer, 50,000 rows in one SQL statement | 26.035 ms | 30.937 ms (+18.8%) |
| BERT tokenizer, 5,000 rows | 35.245 ms | 33.686 ms (overlapping samples; no speedup claim) |
| Ngrams, 10,000 rows | 25.621 ms | 27.158 ms (+6.0%) |
| 3,000 separate analyzer statements | 56.121 ms | 76.400 ms (+36.1%, about 6.8 µs/call extra) |
| WAL for 300 warmed read calls | 0 bytes | 0 bytes |
| WAL for one analyzer definition insert | 328 bytes | 400 bytes |

The contention probe holds one definition transaction open, observes whether the second writer is blocked, then deliberately holds that dependency for 300 ms. The baseline writer completed without blocking (1.50 ms); the current writer blocked on the first writer (316.92 ms total). This demonstrates the intended transaction-length lock cost, not an intrinsic 317 ms operation time. Ordinary vocabulary inserts avoid the version-row update. The 72-byte WAL difference is specific to this probe; page images, checkpoints and other statements can change it.

## Upgrade and operational limits

Install matching library/SQL files, restart PostgreSQL to load the preloaded library, and run `ALTER EXTENSION pg_tokenizer UPDATE TO '1.0.1'` in each database as its extension owner/administrator. Deploy and restart matching libraries on replicas too. Until the SQL upgrade is visible, the legacy primary-only cache path applies.

There are migration paths from 0.1.0, 0.1.1 and 1.0.0. Vocabulary IDs and function dependencies are retained. Review newly rejected bulk inputs and dictionary qualification before upgrading. Changing analyzer/model/dictionary output still requires explicit regeneration of persisted vectors and rebuilding affected indexes. Wait for commit LSN replay and take a new snapshot when queries must match a primary-side definition change.

The runtime-schema split is intentionally deferred: moving vocabulary tables, sequences, triggers and ACLs is a separate migration with broader compatibility costs. The existing CREATE privilege is therefore a documented limit of #14, not a claimed fix. No all-platform or hard-resource-bound claim is made.

## Artifact identities and test housekeeping

| Component | Source commit | SHA256 of dylib |
| --- | --- | --- |
| Current pg_tokenizer | `cddf6ac2969364f34c4029fea60367f90cafa890` | `411166e687b30cb1555c6fb46c32cd29e3674e84746371a90cfd92d08d248d0e` |
| Baseline pg_tokenizer | `4d955eb499cd42ae54183577a368e81b54c91f27` | `007936cdd67a9e54786e35e34c2372e072d1f94071efbe1db25177583c2823c6` |
| Frozen vchord | `77e7bf393356cce7d722894db5880626ed5800f9` | `1294402dfe45899f3f462151557615f7eb2df64c21933553760c75c067a2030e` |
| Frozen vchord_bm25 | `8ea553270373f5270886cf5530f6134bb65b47bc` | `44a4b29a5783a2b2b1cc89f68f9364a282d4a54f9618d49e2d28e01bda208765` |

See [the manifest](evidence/artifacts.json) and [joint-test hashes](evidence/combo.json). Joint acceptance is for these frozen peers; later changes in their worktrees need separate integration validation. The vector dependency was the locally installed pgvector 0.8.7.

Builds, staged libraries and cluster files remain outside Git under `/tmp/pg-tokenizer-phase3-20261107`. Primary, baseline and the earlier joint cluster are [stopped](evidence/cleanup.json); the final joint/preload/standby runners stop and remove their own temporary clusters. The temporary build/database directories can be reclaimed after review; this report retains the small evidence and reproduction sources.

Saved logs have ANSI color and trailing whitespace removed. The website patch uses zero context (`git apply --unidiff-zero`); the edits are already present in the companion checkout, so it is an additional handoff copy. No build products, databases or executables are included in this report directory.

Two harness mistakes were corrected during validation. An early joint run supplied `share/extension` instead of `share` as `extension_control_path` and selected system SQL versions; the final runner asserts every extension version. An early SQLLogicTest invocation supplied `PGHOST/PGPORT`, which that CLI ignores in favor of `SLT_HOST/SLT_PORT` or explicit arguments. It therefore attempted the default localhost:5432 database. Every file failed inside BEGIN before any commit; those transactions rolled back, and a second attempt failed because its database did not exist there. No shared installation or server configuration was changed. Final SQLLogicTest runs explicitly selected the private socket/port. These failed attempts are not counted as validation results.
