# Tests for `pg_tokenizer`

## SQLLogicTest Suite

Run all SQL test cases against a running PostgreSQL 15–19 instance with `pg_tokenizer` (`lindera-ipadic`) preloaded and created:

```shell
sqllogictest --db "$USER" --user "$USER" './tests/**/*.slt'
```

Each `.slt` file runs inside a transaction and rolls back on completion.

## Python Test Suites

1. **Tooling & schema/symbol unit tests** (no PostgreSQL or Rust build required):

```shell
python3 tests/test_pgxn.py
```

2. **Concurrency, permissions, and upgrade integration tests** (`tests/test_integration.py`):
   Requires an administrator connection (`CREATEDB` and `CREATEROLE`), `max_prepared_transactions >= 10`, and all `sql/install/*.sql` and `sql/upgrade/*.sql` scripts installed in `$(pg_config --sharedir)/extension/`:

```shell
python3 -m venv .venv
.venv/bin/pip install 'psycopg[binary]'
PG_TOKENIZER_TEST_DSN='host=localhost user=postgres dbname=postgres' \
  .venv/bin/python tests/test_integration.py
```

   Set `PG_TOKENIZER_TEST_LINDERA_DICTIONARY=/path/to/ipadic` when testing packages built without `--features lindera-ipadic`.

3. **Preload recovery and database isolation** (`tests/test_preload.py`):
   Creates and tears down its own private cluster using `pg_config` from `PATH`:

```shell
.venv/bin/python tests/test_preload.py
```

4. **Physical standby replay, snapshots, promotion, and crash recovery** (`tests/test_standby.py`):
   Creates and tears down disposable primary and standby clusters on ephemeral local ports:

```shell
.venv/bin/python tests/test_standby.py
```
