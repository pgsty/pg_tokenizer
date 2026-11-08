## Tests for pg_tokenizer

We use [sqllogictest-rs](https://github.com/risinglightdb/sqllogictest-rs) to test the SQL queries.

To run all tests, use the following command:
```shell
sqllogictest './tests/**/*.slt'
```

Each time you modify the source code, you can run the following command to clean up the test data and reload the extension:
```shell
psql -f ./tests/init.sql
```

## Note

For each test, it'll rollback the transaction after the test, so you don't need to worry about the side effects of the tests. If you want to keep the data after the test for debugging, you can add the following line to the end of the test:
```sql
statement ok
COMMIT;
```

## Concurrency, roles, and upgrades

Run the Python tests against a disposable cluster with `pg_tokenizer` built with `lindera-ipadic`, preloaded, and
both the current install SQL and historical `sql/install/pg_tokenizer--0.1.[01].sql` files
installed in its extension directory:

```shell
python3 -m venv .venv
.venv/bin/pip install 'psycopg[binary]'
PG_TOKENIZER_TEST_DSN='host=localhost user=postgres dbname=postgres' \
  .venv/bin/python tests/test_integration.py
```

The administrator connection must be allowed to create databases and roles. Tests
create uniquely named databases/roles and remove them afterward. They verify
opposite transaction token orders, concurrent multi-row inserts, complete stored
embeddings after waiting, lock isolation between models, strong-isolation retries, rollback, non-superuser
database owners, unrelated-role denial, Lindera file-dictionary privileges in fresh backends (the test writes a small user dictionary into the server's `pg_tokenizer` data directory), ownership changes, and the 0.1.0/0.1.1/1.0.0 to 1.1.0 upgrades
without rebuilding vocabularies or dropping dependent functions. They also check legacy three-argument triggers, fresh/upgrade catalog equivalence, sequence-name collisions, and qualified names shadowed by literal names containing dots. The regular SQL
suite covers filtering, identifier/literal quoting, predicate-only updates, NULL
sources, tsvector escaping, positions, limits, a native GIN phrase query, and Lindera
dictionary privileges (it writes `pg_tokenizer/lindera_privileges.csv` in the data directory).

Restart the test server after replacing the shared library; it is preloaded.

Set `max_prepared_transactions = 10` on the disposable test cluster to exercise two-phase transaction checks. The regressions also cover concurrent invalidation, old snapshots within and across transactions, multi-statement protocol messages, parallel workers, prepared read-only transactions, trigger overload attacks, preload ACLs, known-token writers, missing source tables, long lexemes, analyzer prepared plans, upgrade refusal for `IMMUTABLE`-only dependents, and a `pg_dump`/restore round trip (requires `pg_dump` and `psql` on PATH). Run the suite with an outer timeout: the original cache deadlock cannot be interrupted by a server statement timeout.

For a native package without embedded IPADIC, set `PG_TOKENIZER_TEST_LINDERA_DICTIONARY` to an IPADIC dictionary directory compiled with Lindera 6. The default is `embedded://ipadic` for GitHub/source builds with that feature.

Run `python tests/test_preload.py` as a non-root OS user with `pg_config` on PATH. It creates and stops its own disposable cluster, checks startup with malformed/stale preload entries, verifies same-name model isolation between databases, and checks that a preloaded Lindera model with a user dictionary is usable by a role without `pg_read_server_files`. Set `PG_TOKENIZER_TEST_LIBRARY` to an absolute library path and `PG_TOKENIZER_TEST_SHAREDIR` to a staged share directory when testing an uninstalled PG18+ package.

Run `python tests/test_standby.py` with the same staged library/share settings to test physical WAL replay in disposable primary/standby clusters. It pauses and resumes replay at explicit LSN barriers, retains a long-lived connection and an old transaction snapshot, promotes the standby, and restarts it after an immediate stop. It creates private ports and sockets and never restarts a shared server. Integration tests also assert writer parallel markings/plans, dictionary name/OID behavior, deterministic budget errors and fresh/upgrade equivalence. `PG_TOKENIZER_TEST_VERSION` can select a historical fresh install for before/after regression runs.

The 1.1.0 cases also verify the runtime schema split, preserved object identities/ACLs, migration rollback after a partial move, both orders of concurrent creation and upgrade, old-snapshot retry, and schema-migration replay followed by promotion and crash recovery. They also check that the 1.0.0 → 1.1.0 upgrade refuses `REPEATABLE READ`/`SERIALIZABLE` (explicit or via `default_transaction_isolation`), and that custom-model vocabularies can be read by a leader participating in a parallel plan after the transaction has written, including `CREATE TABLE AS` and `REFRESH MATERIALIZED VIEW`.

`python3 tests/test_pgxn.py` tests `tools/pgxn.py` without PostgreSQL or Rust: installation renames new files over old ones instead of rewriting a possibly mapped library in place, a tampered stage installs nothing, and a failed build cannot leave an older stage for `make install`.
