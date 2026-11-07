# pg_tokenizer 1.1.0 — final local acceptance

2026-11-08, Asia/Shanghai. **The implementation and local acceptance gates passed.**
The next release upgrades directly from **1.0.0 to 1.1.0**. The unpublished
1.0.1 install/migration files have been removed. Older versions retain the
`0.1.0 → 0.1.1 → 1.0.0 → 1.1.0` path. Historical reports under
`reports/phase3-1.0.1` remain unchanged and are excluded from source distributions.

Validated merge: **`19e2636a05c378f8928afdf8405998c425b13f72`**, on clean local
`main`. The evidence/report commit that follows changes no implementation.
Bilingual website documentation: **`eb8a20fea6a8af645a418e08c26889925386c55a`**.
No Git push, tag, public release, PGXN upload or website deployment was performed.

## Issue disposition

| Issue | Conclusion for 1.1.0 |
| --- | --- |
| [#13](https://github.com/pgsty/pg_tokenizer/issues/13) | Keep open with narrowed scope: allocations/work inside third-party libraries and bounded cancellation latency. Existing admission/stage budgets and extension-loop interrupt checks pass; they are not a hard RSS limit or a security boundary. |
| [#14](https://github.com/pgsty/pg_tokenizer/issues/14) | Locally completed. Runtime vocabularies now live in `tokenizer_data`; nonowner CREATE grants are removed from `tokenizer_catalog`. Earlier parallel-marking, dictionary-resolution, configuration, ID-range and preload fixes remain included. Windows initialization was reviewed, but Windows execution was not tested locally. |
| [#15](https://github.com/pgsty/pg_tokenizer/issues/15) | Keep open as the 1.1.0 release tracker: Linux matrix, package review, account/name authorization and publication remain. |

The prior fixes for #7–#12 are included in this release; there is no intervening
1.0.1 release. The remaining #13 work is explicitly documented rather than
represented as solved. Formal release still requires PostgreSQL 14–18 on Linux
x86_64/aarch64; this report establishes local PG18/macOS acceptance only.

## Adversarial review conclusions

Runtime migration uses `ALTER TABLE ... SET SCHEMA`, preserving table and
sequence OIDs, owners, ACLs and vocabulary IDs. Indexes move with their tables;
dependent views and source-table triggers continue to work. Existing nonowner
schema grants, including grant options, are copied to the data schema. Schema
owners retain PostgreSQL's inherent owner rights. The administrator performing
the upgrade must be able to alter all vocabulary tables, including those owned
by other roles.

The migration locks the model catalog before scanning it. Custom model creation
and deletion acquire a conflicting lock before choosing a vocabulary schema.
Both orders of concurrent creation/upgrade are covered. READ COMMITTED,
REPEATABLE READ and SERIALIZABLE creators waiting across an upgrade receive a
retryable `40001` rather than leaving a new table in the old schema. Name
collisions and a failure after a partial move roll back the entire migration.
The new library continues to support installed 1.0.0 SQL before the upgrade.

Review reproduced a snapshot race: PostgreSQL's namespace cache can find the
new version table while the caller's old SQL snapshot cannot see its singleton.
The fix distinguishes a newly invisible relation from an actually missing
counter, requests a transaction retry for the former, and keeps an error for
the latter. It does not replace the caller's snapshot or weaken cache checks.

Packaging review found that cargo-pgrx only discovers migration aliases in the
top-level `sql/` directory. The direct 1.0.0 → 1.1.0 alias is now present, and
the PGXN builder explicitly stages all migrations. The source ZIP dereferences
repository SQL symlinks so ZIP clients can build without `.git` or symlink
support. A modified staged package is rejected before any installation write.

Upgrade notes also retain the need to qualify ambiguous legacy `pg_dict`
names, restart primary/replica libraries, wait for replay, and regenerate
persisted output/indexes produced with the swapped Jieba `full`/`precise` modes
in 1.0.0 or earlier. Stored output is not rewritten automatically.

## Acceptance evidence

Environment: PostgreSQL 18.6, Rust 1.99.1, cargo-pgrx 0.19.3, macOS arm64,
features `pg18 lindera-ipadic`. The merged source was built in a new target
directory at `/tmp/pg-tokenizer-1.1.0-final-20261108`. Every database cluster was
disposable. See [environment](evidence/main/environment.json) and the
[main acceptance manifest](evidence/main/acceptance.json).

| Check | Result |
| --- | --- |
| Optimized build and SQL generation | Passed. Generated and checked-in SQL match except the order of five independent catalog-table declarations and terminal whitespace. [Build](evidence/main/build.log). |
| Rust tests and Clippy | 31 unit tests passed; Clippy with `-D warnings` passed. [Tests](evidence/main/unit.log), [Clippy](evidence/main/clippy.log). |
| Static checks | Rustfmt, Taplo, Python compilation, workflow YAML, clean source and unchanged historical SQL passed. [Checks](evidence/main/static-checks.json). |
| Integration | 49 passed, covering upgrades, concurrency, permissions, IDs, dependencies, snapshots, rollback and dump/restore. [Log](evidence/main/integration.log). |
| SQLLogicTest | All 22 files passed. [Log](evidence/main/slt.log). |
| Physical standby | 1.0.0 schema migration replay in a previously used session, definition changes, old snapshots, promotion, writes through migrated vocabulary triggers and immediate-stop recovery passed. [Log](evidence/main/standby.log). |
| Preload | Restart, malformed entries, database identity and server-file privilege cases passed. [Log](evidence/main/preload.log). |
| Three-extension coexistence | All six preload orders passed, including BM25, vchordrq and vchordg index plans/results. [Log](evidence/main/combo-orders.log), [hashes](evidence/main/combo-final.json). |
| PGXN metadata/source | Official PGXN::Meta::Validator v0.16.0 passed. The archive has 103 unique rooted files, correct versions, complete aliases, no 1.0.1 SQL, and no reports/AGENTS/.github. Rebuilding the same ref yielded an identical ZIP. [Validator](evidence/main/pgxn-validator.log), [archive checks](evidence/main/source-package.json). |
| PGXN client | pgxnclient 1.3.2 built the final source ZIP without `.git` and installed six verified files to a private DESTDIR. [Log](evidence/main/pgxn-client-install.log), [manifest](evidence/main/pgxn-install-manifest.json). |
| Installed PGXN artifact | Its actual installed library/generated SQL separately passed the same 49 integration tests, 22 SLT files, standby/recovery, preload and six extension orders. [Manifest](evidence/pgxn/acceptance.json). |
| Website | `make check` passed with committed OINK 1.5.0 and the existing working OINK 1.5.1; both have zero missing targets/fragments. [1.5.0](evidence/site-check-1.5.0.log), [1.5.1](evidence/site-check-1.5.1.log). |

Joint tests use frozen vchord `77e7bf393356cce7d722894db5880626ed5800f9`,
vchord_bm25 `8ea553270373f5270886cf5530f6134bb65b47bc`, and pgvector 0.8.7.
They do not validate later changes in those repositories. The runner asserts
the frozen peer library hashes. The newly built tokenizer library SHA-256 is
`1af8eecc7092bb5af5a88a686bf556a1e895152d409e8dc9467ca15f2255ee8e`;
the independently packaged library is
`14b31c1f775937b1587a2eec21203119f98a02937b25a40d75ea38c290118581`.

All disposable clusters stopped and were removed. The 21 pre-existing website
modifications were verified unchanged, and only the six tokenizer pages were
committed. No AGENTS.md was changed or committed. Evidence hashes are recorded
in [SHA256.json](evidence/SHA256.json).

## Reviewable source package and publication steps

Source ZIP: `build/dist/pg_tokenizer-1.1.0.zip`, 9,285,222 bytes, built from the
validated merge above. SHA-256:

```text
640a1dfd25da632525e508e5916b8971013acff11e539eca41c915355ed5ac44
```

The ZIP is a local preparation artifact. After choosing the final publication
commit, run the Linux Check matrix, tag that commit, build/review its release
assets and then publish. The Release workflow creates a draft GitHub release
with Linux ZIP/DEB packages for ten PostgreSQL/architecture combinations and
the PGXN source ZIP/checksum. It does not upload to PGXN.

PGXN still requires an approved Manager account and confirmed permission to
publish the `pg_tokenizer` distribution/extension. The public index lookup
returned no entry; that does not establish account status or name availability.
Manual upload uses the Manager form or a password-prompting API call. An
unattended upload would additionally need securely configured credentials and
a publication job. See the [release/PGXN guide](../../docs/08-release.md) for
the official links, build prerequisites and upload/verification commands.

## Reproduction

Use a new artifact root, PostgreSQL tools and sqllogictest on PATH, Python with
psycopg, and the frozen peer libraries/SQL whose hashes the runner asserts:

```bash
export PGRX_PG_CONFIG_PATH=/opt/homebrew/bin/pg_config
export CARGO_TARGET_DIR=/tmp/tokenizer-110-check/target
mkdir -p /tmp/tokenizer-110-check/logs
cargo pgrx schema --release --cargo=--locked --features 'pg18 lindera-ipadic' \
  --out /tmp/tokenizer-110-check/schema.sql
cargo test --locked --no-fail-fast --features 'pg18 lindera-ipadic'
cargo clippy --locked --all-targets --features 'pg18 lindera-ipadic' -- -D warnings
python reports/release-1.1.0/scripts/final_acceptance.py \
  /tmp/tokenizer-110-check "$PWD" /path/to/frozen-peers
make dist REF=19e2636a05c378f8928afdf8405998c425b13f72 DIST_DIR=/tmp/tokenizer-dist
DESTDIR=/tmp/tokenizer-pgxn-install pgxn install --pg_config "$PGRX_PG_CONFIG_PATH" \
  --make make --nosudo /tmp/tokenizer-dist/pg_tokenizer-1.1.0.zip
```

The local acceptance runner uses macOS dylibs and PostgreSQL 18's private
extension-control path. The GitHub Check workflow is the portable Linux matrix.
