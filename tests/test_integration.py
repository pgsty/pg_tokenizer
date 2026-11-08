"""Multi-session/role/migration tests. Use a disposable PostgreSQL cluster.

PG_TOKENIZER_TEST_DSN must name an administrator connection with CREATEDB and
CREATEROLE. Each test creates and removes its own database and roles.
"""

import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import threading
import time
import unittest
import uuid

import psycopg
from psycopg import sql


class IntegrationTests(unittest.TestCase):
    def setUp(self):
        self.dsn = os.environ["PG_TOKENIZER_TEST_DSN"]
        self.admin = psycopg.connect(self.dsn, autocommit=True)
        self.addCleanup(self.admin.close)
        suffix = uuid.uuid4().hex[:12]
        self.dbname = f"tokenizer_test_{suffix}"
        self.owner = f"tokenizer_owner_{suffix}"
        self.outsider = f"tokenizer_other_{suffix}"
        self.admin.execute(
            sql.SQL("CREATE ROLE {} LOGIN").format(sql.Identifier(self.owner))
        )
        self.addCleanup(
            self.admin.execute,
            sql.SQL("DROP ROLE {}").format(sql.Identifier(self.owner)),
        )
        self.admin.execute(
            sql.SQL("CREATE ROLE {} LOGIN").format(sql.Identifier(self.outsider))
        )
        self.addCleanup(
            self.admin.execute,
            sql.SQL("DROP ROLE {}").format(sql.Identifier(self.outsider)),
        )
        self.admin.execute(
            sql.SQL("CREATE DATABASE {} OWNER {}").format(
                sql.Identifier(self.dbname), sql.Identifier(self.owner)
            )
        )
        self.addCleanup(
            self.admin.execute,
            sql.SQL("DROP DATABASE {} WITH (FORCE)").format(
                sql.Identifier(self.dbname)
            ),
        )
        self.db = self.connect()
        self.addCleanup(self.db.close)
        if self._testMethodName == "test_database_owner_permissions":
            self.db.execute(
                "ALTER DEFAULT PRIVILEGES REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC"
            )
        version = (
            "0.1.0"
            if "from_010" in self._testMethodName
            else ("1.0.0" if "from_100" in self._testMethodName else
                  ("0.1.1" if "upgrade" in self._testMethodName else os.environ.get("PG_TOKENIZER_TEST_VERSION", "1.1.0")))
        )
        self.db.execute(
            sql.SQL("CREATE EXTENSION pg_tokenizer VERSION {}").format(
                sql.Literal(version)
            )
        )

    def connect(self, **kwargs):
        conn = psycopg.connect(self.dsn, dbname=self.dbname, autocommit=True, **kwargs)
        conn.execute("SET search_path = public, tokenizer_catalog, tokenizer_data")
        conn.execute("SET statement_timeout = '15s'")
        return conn

    def make_model(self, conn, name="race"):
        conn.execute(
            sql.SQL("CREATE TABLE {} (body text, embedding int[])").format(
                sql.Identifier(name)
            )
        )
        conn.execute(
            "SELECT create_text_analyzer(%s, 'pre_tokenizer = \"unicode_segmentation\"')",
            (name,),
        )
        conn.execute(
            "SELECT create_custom_model_tokenizer_and_trigger(%s, %s, %s, %s, 'body', 'embedding')",
            (name, name, name, name),
        )

    def assert_embeddings(self, conn, name="race"):
        result = conn.execute(
            sql.SQL("""
            SELECT bool_and(embedding = tokenize(body, %s) AND cardinality(embedding) > 0)
            FROM {}
        """).format(sql.Identifier(name)),
            (name,),
        ).fetchone()[0]
        self.assertTrue(result)

    def test_concurrent_opposite_transactions(self):
        self.make_model(self.db)
        self.make_model(self.db, "independent")
        # Opposite token order across statements defeats a per-row ORDER BY fix.
        with self.connect() as a, self.connect() as b, self.connect() as observer:
            a.execute("BEGIN")
            a.execute("INSERT INTO race(body) VALUES ('alpha alpha')")
            pid = b.info.backend_pid

            def second_writer():
                with b.transaction():
                    b.execute("INSERT INTO race(body) VALUES ('beta beta')")
                    b.execute("INSERT INTO race(body) VALUES ('alpha beta')")

            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(second_writer)
                try:
                    deadline = time.monotonic() + 5
                    wait = None
                    while time.monotonic() < deadline:
                        wait = observer.execute(
                            "SELECT wait_event FROM pg_stat_activity WHERE pid = %s",
                            (pid,),
                        ).fetchone()[0]
                        if wait in ("advisory", "transactionid") or future.done():
                            break
                        time.sleep(0.02)
                    self.assertEqual(wait, "advisory")
                    # A different model has a different lock, and readers remain usable.
                    observer.execute(
                        "INSERT INTO independent(body) VALUES ('beta alpha')"
                    )
                    observer.execute("SELECT tokenize('alpha', 'race')")
                    a.execute("INSERT INTO race(body) VALUES ('beta alpha')")
                finally:
                    a.execute("COMMIT")
                future.result(timeout=10)
        self.assert_embeddings(self.db)
        self.assertEqual(
            self.db.execute("SELECT count(*) FROM model_race").fetchone()[0], 2
        )

    def test_concurrent_batches_and_rollback(self):
        self.make_model(self.db)
        barrier = threading.Barrier(6)

        def writer(worker):
            with self.connect() as conn:
                barrier.wait(timeout=10)
                for batch in range(8):
                    words = [f"word{batch}x{i}" for i in range(6)]
                    if worker % 2:
                        words.reverse()
                    with conn.transaction():
                        conn.execute(
                            "INSERT INTO race(body) SELECT unnest(%s::text[])",
                            ([f"{word} {word}" for word in words],),
                        )

        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            list(pool.map(writer, range(6)))
        self.assertEqual(
            self.db.execute("SELECT count(*) FROM model_race").fetchone()[0], 48
        )
        self.assertEqual(
            self.db.execute("SELECT count(*) FROM race").fetchone()[0], 288
        )
        self.assert_embeddings(self.db)
        with self.connect() as conn:
            conn.execute("BEGIN")
            conn.execute("INSERT INTO race(body) VALUES ('rollbackword')")
            conn.execute("ROLLBACK")
        self.assertEqual(
            self.db.execute(
                "SELECT count(*) FROM model_race WHERE token = 'rollbackword'"
            ).fetchone()[0],
            0,
        )
        self.db.execute("INSERT INTO race(body) VALUES ('rollbackword')")
        self.assert_embeddings(self.db)

    def test_catalog_cache_invalidation_across_backends_and_rollbacks(self):
        analyzer = "cache_analyzer"
        tokenizer = "cache_tokenizer"
        model = "cache_model"

        self.db.execute(f"SELECT create_text_analyzer('{analyzer}', '')")
        with self.connect() as peer:
            self.assertEqual(
                peer.execute(
                    "SELECT analyze_to_tsvector('UPPER', %s)::text", (analyzer,)
                ).fetchone()[0],
                "'UPPER':1",
            )

            self.db.execute(f"SELECT drop_text_analyzer('{analyzer}')")
            self.db.execute(
                f"SELECT create_text_analyzer('{analyzer}', 'character_filters = [\"to_lowercase\"]')"
            )
            self.assertEqual(
                peer.execute(
                    "SELECT analyze_to_tsvector('UPPER', %s)::text", (analyzer,)
                ).fetchone()[0],
                "'upper':1",
            )

            self.db.execute("CREATE TABLE cache_documents (body text)")
            self.db.execute("INSERT INTO cache_documents VALUES ('HELLO')")
            self.db.execute(f"SELECT drop_text_analyzer('{analyzer}')")
            self.db.execute(f"SELECT create_text_analyzer('{analyzer}', '')")
            self.db.execute(
                "SELECT create_custom_model(%s, %s)",
                (
                    model,
                    "table = 'cache_documents'\n"
                    "column = 'body'\n"
                    f"text_analyzer = '{analyzer}'\n",
                ),
            )
            self.db.execute(
                "SELECT create_tokenizer(%s, %s)",
                (tokenizer, f'text_analyzer = "{analyzer}"\nmodel = "{model}"'),
            )
            self.assertEqual(
                peer.execute("SELECT tokenize('HELLO', %s)", (tokenizer,)).fetchone()[0],
                [1],
            )

            # Rebuilding an analyzer invalidates tokenizers that captured its
            # old Arc, even when the tokenizer catalog row itself is unchanged.
            self.db.execute(f"SELECT drop_text_analyzer('{analyzer}')")
            self.db.execute(
                f"SELECT create_text_analyzer('{analyzer}', 'character_filters = [\"to_lowercase\"]')"
            )
            self.assertEqual(
                peer.execute("SELECT tokenize('HELLO', %s)", (tokenizer,)).fetchone()[0],
                [],
            )

        with self.connect() as rollback:
            rollback.execute("BEGIN")
            rollback.execute(
                "SELECT create_text_analyzer('rolled_back_analyzer', 'character_filters = [\"to_lowercase\"]')"
            )
            self.assertEqual(
                rollback.execute(
                    "SELECT apply_text_analyzer('UPPER', 'rolled_back_analyzer')"
                ).fetchone()[0],
                ["upper"],
            )
            rollback.execute("ROLLBACK")
            with self.assertRaises(psycopg.Error):
                rollback.execute(
                    "SELECT apply_text_analyzer('UPPER', 'rolled_back_analyzer')"
                )

            self.db.execute("SELECT create_text_analyzer('rollback_drop_analyzer', '')")
            rollback.execute("SELECT apply_text_analyzer('UPPER', 'rollback_drop_analyzer')")
            rollback.execute("BEGIN")
            rollback.execute("SELECT drop_text_analyzer('rollback_drop_analyzer')")
            rollback.execute("ROLLBACK")
            self.assertEqual(
                rollback.execute(
                    "SELECT apply_text_analyzer('UPPER', 'rollback_drop_analyzer')"
                ).fetchone()[0],
                ["UPPER"],
            )

    def test_database_owner_permissions(self):
        self.db.execute(
            "CREATE TABLE tokenizer_catalog.future_table(id int GENERATED ALWAYS AS IDENTITY, token text)"
        )
        with self.connect(user=self.owner) as owner:
            self.assertFalse(
                owner.execute(
                    "SELECT rolsuper FROM pg_roles WHERE rolname = current_user"
                ).fetchone()[0]
            )
            owner.execute(
                "INSERT INTO tokenizer_catalog.future_table(token) VALUES ('defaults') RETURNING id"
            ).fetchone()
            self.assertFalse(owner.execute("SELECT has_schema_privilege(current_user, 'tokenizer_catalog', 'CREATE')").fetchone()[0])
            self.assertTrue(owner.execute("SELECT has_schema_privilege(current_user, 'tokenizer_data', 'USAGE,CREATE')").fetchone()[0])
            self.make_model(owner)
            owner.execute("SELECT create_stopwords('custom_stopwords', 'the')")
            owner.execute("SELECT create_synonym('custom_synonyms', 'pg postgres')")
            owner.execute("INSERT INTO race(body) VALUES ('alpha beta')")
            self.assert_embeddings(owner)
            owner.execute("SELECT drop_custom_model('race')")
            owner.execute("SELECT drop_tokenizer('race')")
            owner.execute("SELECT drop_text_analyzer('race')")
            owner.execute("SELECT drop_stopwords('custom_stopwords')")
            owner.execute("SELECT drop_synonym('custom_synonyms')")
        with self.connect(user=self.outsider) as outsider:
            self.assertFalse(
                outsider.execute(
                    "SELECT has_schema_privilege(current_user, 'tokenizer_catalog', 'USAGE')"
                ).fetchone()[0]
            )
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                outsider.execute("SELECT * FROM tokenizer_catalog.model")
        # Grants track the database owner rather than one named login.
        self.db.execute(
            sql.SQL("ALTER DATABASE {} OWNER TO {}").format(
                sql.Identifier(self.dbname), sql.Identifier(self.outsider)
            )
        )
        with self.connect(user=self.outsider) as owner:
            owner.execute("SELECT create_text_analyzer('new_owner', '')")
            owner.execute("SELECT drop_text_analyzer('new_owner')")

    def test_identity_sequence_name_collision(self):
        # An existing relation forces PostgreSQL to choose a suffixed sequence.
        # Remove installer defaults so this checks the explicit model grants.
        self.db.execute(
            "ALTER DEFAULT PRIVILEGES IN SCHEMA tokenizer_data REVOKE USAGE, SELECT ON SEQUENCES FROM pg_database_owner"
        )
        self.db.execute("CREATE TABLE tokenizer_data.model_race_id_seq(dummy int)")
        self.make_model(self.db)
        sequence = self.db.execute(
            "SELECT pg_get_serial_sequence('model_race', 'id')"
        ).fetchone()[0]
        self.assertNotEqual(sequence, "tokenizer_data.model_race_id_seq")
        with self.connect(user=self.owner) as owner:
            self.assertTrue(
                owner.execute(
                    "SELECT has_sequence_privilege(current_user, %s, 'USAGE')",
                    (sequence,),
                ).fetchone()[0]
            )

    def test_qualified_table_with_literal_shadow(self):
        self.db.execute("CREATE SCHEMA corpus")
        self.db.execute("CREATE TABLE corpus.docs(body text, embedding int[])")
        self.db.execute('CREATE TABLE "corpus.docs"(body text, embedding int[])')
        self.db.execute("INSERT INTO corpus.docs(body) VALUES ('correct')")
        self.db.execute("INSERT INTO \"corpus.docs\"(body) VALUES ('wrong')")
        self.db.execute("SELECT create_text_analyzer('qualified', '')")
        self.db.execute(
            "SELECT create_custom_model_tokenizer_and_trigger('qualified', 'qualified', 'qualified', 'corpus.docs', 'body', 'embedding')"
        )
        self.assertEqual(
            self.db.execute("SELECT token FROM model_qualified").fetchall(),
            [("correct",)],
        )
        self.db.execute("INSERT INTO corpus.docs(body) VALUES ('new')")
        self.assertTrue(
            self.db.execute(
                "SELECT bool_and(embedding = tokenize(body, 'qualified')) FROM corpus.docs"
            ).fetchone()[0]
        )
        self.db.execute("SELECT drop_custom_model('qualified')")
        self.assertEqual(
            self.db.execute(
                "SELECT count(*) FROM pg_trigger WHERE tgname LIKE 'model_qualified%'"
            ).fetchone()[0],
            0,
        )

    def test_concurrent_strong_isolation_requires_retry(self):
        self.make_model(self.db)
        for isolation in ["REPEATABLE READ", "SERIALIZABLE"]:
            with (
                self.subTest(isolation=isolation),
                self.connect() as a,
                self.connect() as b,
            ):
                word = isolation.replace(" ", "").lower()
                a.execute("BEGIN")
                a.execute("INSERT INTO race(body) VALUES (%s)", (word,))
                b.execute(sql.SQL("BEGIN ISOLATION LEVEL " + isolation))
                b.execute("SELECT count(*) FROM race")  # Fix the old snapshot.
                with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                    future = pool.submit(
                        b.execute, "INSERT INTO race(body) VALUES (%s)", (word,)
                    )
                    try:
                        deadline = time.monotonic() + 5
                        wait = None
                        while time.monotonic() < deadline:
                            wait = self.db.execute(
                                "SELECT wait_event FROM pg_stat_activity WHERE pid = %s",
                                (b.info.backend_pid,),
                            ).fetchone()[0]
                            if wait == "advisory" or future.done():
                                break
                            time.sleep(0.02)
                        self.assertEqual(wait, "advisory")
                    finally:
                        a.execute("COMMIT")
                    with self.assertRaises(psycopg.errors.SerializationFailure):
                        future.result(timeout=10)
                b.execute("ROLLBACK")
                b.execute("INSERT INTO race(body) VALUES (%s)", (word,))
        self.assert_embeddings(self.db)

    def test_native_vector_limits_and_cached_plans(self):
        self.db.execute("SELECT create_text_analyzer('raw', '')")
        self.assertEqual(
            self.db.execute("SELECT analyze_to_tsvector(repeat('x', 2048), 'raw')::text").fetchone()[0],
            "",
        )
        self.db.execute("SET plan_cache_mode = force_generic_plan")
        self.db.execute(
            "PREPARE fts_cached AS SELECT analyze_to_tsvector('UPPER', 'raw')::text"
        )
        self.assertEqual(
            self.db.execute("EXECUTE fts_cached").fetchone()[0], "'UPPER':1"
        )
        self.db.execute("SELECT drop_text_analyzer('raw')")
        self.db.execute(
            "SELECT create_text_analyzer('raw', 'character_filters = [\"to_lowercase\"]')"
        )
        self.assertEqual(
            self.db.execute("EXECUTE fts_cached").fetchone()[0], "'upper':1"
        )
        self.make_model(self.db)
        self.db.execute("INSERT INTO race(body) VALUES ('alpha alpha beta')")
        self.assertEqual(
            self.db.execute(
                "SELECT tokenize_to_tsvector('alpha alpha beta', 'race')::text"
            ).fetchone()[0],
            "'1':1,2 '2':3",
        )
        self.assertEqual(
            self.db.execute(
                "SELECT tokenize_to_tsvector('unseen', 'race')::text"
            ).fetchone()[0],
            "",
        )
        dictionary = os.environ.get("PG_TOKENIZER_TEST_LINDERA_DICTIONARY", "embedded://ipadic")
        self.db.execute(
            "SELECT create_lindera_model('ipadic', %s)",
            (f"[segmenter]\nmode = \"normal\"\ndictionary = {json.dumps(dictionary)}",),
        )
        self.db.execute("SELECT create_tokenizer('ipadic', 'model = \"ipadic\"')")
        self.assertTrue(
            self.db.execute(
                "SELECT cardinality(tsvector_to_array(tokenize_to_tsvector('東京東京', 'ipadic'))) > 0"
            ).fetchone()[0]
        )

    def test_lindera_file_dictionaries_require_privileges(self):
        # A new backend loads models from the catalog. Only superusers and roles
        # with privileges of pg_read_server_files may make Lindera open files.
        path = self.admin.execute(
            "SELECT current_setting('data_directory') || '/pg_tokenizer/test_user_dictionary.csv'"
        ).fetchone()[0]
        self.admin.execute(
            sql.SQL("COPY (SELECT {}) TO {}").format(
                sql.Literal("東京スカイツリー,カスタム名詞,トウキョウスカイツリー"),
                sql.Literal(path),
            )
        )
        dictionary = os.environ.get("PG_TOKENIZER_TEST_LINDERA_DICTIONARY", "embedded://ipadic")
        config = (
            f"[segmenter]\ndictionary = {json.dumps(dictionary)}\n"
            f"user_dictionary = {json.dumps(path)}"
        )
        self.db.execute("SELECT create_lindera_model('user_dictionary', %s)", (config,))
        self.db.execute("SELECT create_tokenizer('user_dictionary', 'model = \"user_dictionary\"')")
        query = "SELECT tokenize('東京スカイツリー', 'user_dictionary')"
        self.assertEqual(self.db.execute(query).fetchone()[0], [1 << 30])
        embedded = dictionary.startswith("embedded://")
        legacy = json.dumps({"lindera": {"segmenter": {
            "dictionary": {"kind": "ipadic"} if embedded else {"path": dictionary},
            "user_dictionary": {"path": path},
        }}})
        with self.connect(user=self.owner) as owner:
            with self.assertRaises(psycopg.errors.InsufficientPrivilege) as refused:
                owner.execute(query)
            self.assertNotIn(path, str(refused.exception))
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                owner.execute("SELECT create_lindera_model('owner_dictionary', %s)", (config,))
            # Writing the catalog directly does not bypass the check.
            owner.execute("INSERT INTO tokenizer_catalog.model VALUES ('owner_dml', %s)", (legacy,))
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                owner.execute("SELECT create_tokenizer('owner_dml', 'model = \"owner_dml\"')")
            if embedded:
                owner.execute(
                    "SELECT create_lindera_model('owner_embedded', %s)",
                    (f"[segmenter]\ndictionary = {json.dumps(dictionary)}",),
                )
        readers = f"tokenizer_readers_{self.dbname[-12:]}"
        self.admin.execute(sql.SQL("CREATE ROLE {}").format(sql.Identifier(readers)))
        self.addCleanup(
            self.admin.execute, sql.SQL("DROP ROLE {}").format(sql.Identifier(readers))
        )
        self.admin.execute(
            sql.SQL("GRANT pg_read_server_files TO {}").format(sql.Identifier(readers))
        )
        self.admin.execute(
            sql.SQL("GRANT {} TO {}").format(sql.Identifier(readers), sql.Identifier(self.owner))
        )
        # Inherited membership applies to the next backend's loads.
        with self.connect(user=self.owner) as owner:
            self.assertEqual(owner.execute(query).fetchone()[0], [1 << 30])
            owner.execute("SELECT create_tokenizer('owner_dml', 'model = \"owner_dml\"')")
            self.assertEqual(
                owner.execute("SELECT tokenize('東京スカイツリー', 'owner_dml')").fetchone()[0],
                [1 << 30],
            )

    def test_filter_validation_on_empty_corpus(self):
        self.db.execute("CREATE TABLE empty_corpus (body text, tenant int)")
        self.db.execute("SELECT create_text_analyzer('empty_analyzer', '')")
        for predicate in [
            "",
            "tenant",
            "missing_column = 1",
            "tenant =",
            "ctid IS NOT NULL",
            "empty_corpus.tenant = 1",
        ]:
            with self.subTest(predicate=predicate), self.assertRaises(psycopg.Error):
                # JSON strings are valid TOML basic strings for these values.
                config = (
                    'table = "empty_corpus"\ncolumn = "body"\n'
                    'text_analyzer = "empty_analyzer"\nfilter = '
                    + json.dumps(predicate)
                )
                self.db.execute(
                    "SELECT create_custom_model('invalid_filter', %s)", (config,)
                )
        self.assertIsNone(
            self.db.execute(
                "SELECT to_regclass('tokenizer_data.model_invalid_filter')"
            ).fetchone()[0]
        )

    def test_upgrade_from_010(self):
        self.make_model(self.db)
        self.db.execute("INSERT INTO race(body) VALUES ('alpha beta')")
        original = self.db.execute(
            "SELECT id, token FROM model_race ORDER BY id"
        ).fetchall()
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(
            self.db.execute("SELECT id, token FROM model_race ORDER BY id").fetchall(),
            original,
        )
        self.db.execute("INSERT INTO race(body) VALUES ('gamma alpha')")
        self.assert_embeddings(self.db)

    def test_upgrade_preserves_vocabulary_and_function_dependencies(self):
        self.make_model(self.db)
        # The current library creates four-argument vocabulary triggers even
        # when installed with old SQL. Reproduce the actual legacy trigger ABI.
        self.db.execute("DROP TRIGGER model_race_trigger ON race")
        self.db.execute("DROP TRIGGER model_race_trigger_insert ON race")
        self.db.execute("""CREATE TRIGGER model_race_trigger BEFORE INSERT OR UPDATE OF body ON race
            FOR EACH ROW EXECUTE FUNCTION custom_model_insert_trigger('race', 'body', 'race')""")
        self.db.execute("""CREATE TRIGGER model_race_trigger_insert BEFORE INSERT OR UPDATE OF body ON race
            FOR EACH ROW EXECUTE FUNCTION custom_model_tokenizer_set_target_column_trigger('race', 'body', 'embedding')""")
        self.db.execute("INSERT INTO race(body) VALUES ('alpha beta alpha')")
        self.db.execute("REVOKE ALL ON model_race FROM pg_database_owner")
        self.db.execute(
            "REVOKE ALL ON SEQUENCE model_race_id_seq FROM pg_database_owner"
        )
        original = self.db.execute(
            "SELECT id, token FROM model_race ORDER BY id"
        ).fetchall()
        signature = "tokenizer_catalog.create_custom_model_tokenizer_and_trigger(text,text,text,text,text,text)"
        oid = self.db.execute("SELECT %s::regprocedure::oid", (signature,)).fetchone()[
            0
        ]
        self.db.execute("""CREATE VIEW helper_dependency AS
            SELECT create_custom_model_tokenizer_and_trigger('viewtok', 'viewmodel', 'race', 'race', 'body', 'embedding') IS NULL AS called""")
        # Simulate old stored configs that lack the optional filter field.
        self.db.execute(
            "UPDATE tokenizer_catalog.model SET config = jsonb_set(config::jsonb #- '{custom,filter}', '{custom,table}', '\"race\"')::text"
        )
        with self.connect(user=self.owner) as owner:
            self.assertFalse(
                owner.execute(
                    "SELECT has_schema_privilege(current_user, 'tokenizer_catalog', 'USAGE')"
                ).fetchone()[0]
            )
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(
            self.db.execute(
                "SELECT extversion FROM pg_extension WHERE extname = 'pg_tokenizer'"
            ).fetchone()[0],
            "1.1.0",
        )
        self.assertEqual(
            self.db.execute("SELECT id, token FROM model_race ORDER BY id").fetchall(),
            original,
        )
        self.assertEqual(
            self.db.execute("SELECT %s::regprocedure::oid", (signature,)).fetchone()[0],
            oid,
        )
        self.assertIn(
            "pg_advisory_xact_lock",
            self.db.execute(
                "SELECT prosrc FROM pg_proc WHERE oid = 'custom_model_insert_trigger()'::regprocedure"
            ).fetchone()[0],
        )
        with self.connect(user=self.owner) as owner:
            owner.execute("SELECT create_text_analyzer('owner_analyzer', '')")
            self.assertEqual(
                len(
                    owner.execute("SELECT tokenize('alpha beta', 'race')").fetchone()[0]
                ),
                2,
            )
            self.assertTrue(
                owner.execute(
                    "SELECT has_table_privilege(current_user, 'tokenizer_data.model_race', 'INSERT')"
                ).fetchone()[0]
            )
            self.assertTrue(
                owner.execute(
                    "SELECT has_sequence_privilege(current_user, 'tokenizer_data.model_race_id_seq', 'USAGE')"
                ).fetchone()[0]
            )
            self.make_model(owner, "after_upgrade")
            owner.execute("INSERT INTO after_upgrade(body) VALUES ('new words')")
            self.assert_embeddings(owner, "after_upgrade")
            self.assertEqual(
                owner.execute(
                    "SELECT analyze_to_tsvector('hello', 'owner_analyzer')::text"
                ).fetchone()[0],
                "'hello':1",
            )
        self.db.execute("INSERT INTO race(body) VALUES ('gamma alpha')")
        self.assert_embeddings(self.db)
        # All supported old versions have an upgrade path in standard pgrx packages.
        path = self.db.execute(
            "SELECT path FROM pg_extension_update_paths('pg_tokenizer') WHERE source = '0.1.0' AND target = '1.1.0'"
        ).fetchone()[0]
        self.assertEqual(path, "0.1.0--0.1.1--1.0.0--1.1.0")

        # A fresh installation and an upgraded installation must expose the same
        # function signatures, defaults, volatility, language and implementation.
        catalog_sql = """
            SELECT p.oid::regprocedure::text, p.proargnames, p.pronargdefaults,
                   p.provolatile, p.proparallel, p.proisstrict, p.prosecdef, l.lanname, p.prosrc, p.proacl
            FROM pg_proc p JOIN pg_language l ON l.oid = p.prolang
            JOIN pg_depend d ON d.objid = p.oid AND d.classid = 'pg_proc'::regclass
            JOIN pg_extension e ON d.refobjid = e.oid AND d.refclassid = 'pg_extension'::regclass
            WHERE e.extname = 'pg_tokenizer' AND d.deptype = 'e'
            ORDER BY p.oid::regprocedure::text
        """
        fresh_name = self.dbname + "_fresh"
        self.admin.execute(
            sql.SQL("CREATE DATABASE {}").format(sql.Identifier(fresh_name))
        )
        try:
            with psycopg.connect(self.dsn, dbname=fresh_name, autocommit=True) as fresh:
                fresh.execute("SET search_path = public, tokenizer_catalog, tokenizer_data")
                fresh.execute("CREATE EXTENSION pg_tokenizer")
                self.assertEqual(
                    self.db.execute(catalog_sql).fetchall(),
                    fresh.execute(catalog_sql).fetchall(),
                )
        finally:
            self.admin.execute(
                sql.SQL("DROP DATABASE {} WITH (FORCE)").format(
                    sql.Identifier(fresh_name)
                )
            )


    def test_lookups_survive_concurrent_invalidation(self):
        # A lookup that loads dependencies while another backend publishes an
        # invalidation must not deadlock on its own cache map.
        self.db.execute(
            "SELECT create_text_analyzer('churn_analyzer', 'pre_tokenizer = \"unicode_segmentation\"')"
        )
        self.db.execute(
            "SELECT create_tokenizer('churn_tokenizer', 'text_analyzer = \"churn_analyzer\"\nmodel = \"bert_base_uncased\"')"
        )
        stop = time.monotonic() + 3

        def churn():
            with self.connect() as conn:
                conn.execute("SET client_min_messages = error")
                while time.monotonic() < stop:
                    conn.execute("SELECT drop_text_analyzer('missing_analyzer')")

        def reader():
            with self.connect() as conn:
                count = 0
                while time.monotonic() < stop:
                    conn.execute("SELECT tokenize('hello world', 'churn_tokenizer')")
                    count += 1
                return count

        with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool:
            futures = [pool.submit(churn)] + [pool.submit(reader) for _ in range(4)]
            for future in futures:
                future.result(timeout=20)

    def test_cache_not_retained_from_older_snapshot(self):
        self.db.execute("SELECT create_tokenizer('snap_tokenizer', 'model = \"bert_base_uncased\"')")
        with self.connect() as reader:
            reader.execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
            reader.execute("SELECT count(*) FROM tokenizer_catalog.tokenizer")
            self.db.execute("SELECT drop_tokenizer('snap_tokenizer')")
            self.db.execute("SELECT create_tokenizer('snap_tokenizer', 'model = \"llmlingua2\"')")
            expected = self.db.execute("SELECT tokenize('hello', 'snap_tokenizer')").fetchone()[0]
            old = reader.execute("SELECT tokenize('hello', 'snap_tokenizer')").fetchone()[0]
            self.assertNotEqual(old, expected)  # the old snapshot still sees bert
            reader.execute("COMMIT")
            self.assertEqual(
                reader.execute("SELECT tokenize('hello', 'snap_tokenizer')").fetchone()[0],
                expected,
            )

    def test_prepare_transaction_with_definition_change_is_rejected(self):
        if self.db.execute("SHOW max_prepared_transactions").fetchone()[0] == "0":
            self.skipTest("max_prepared_transactions = 0")
        with self.connect() as conn:
            conn.execute("BEGIN")
            conn.execute("SELECT create_text_analyzer('prepared_analyzer', '')")
            with self.assertRaises(psycopg.Error):
                conn.execute("PREPARE TRANSACTION 'pg_tokenizer_test'")
            conn.execute("ROLLBACK")
        self.assertEqual(
            self.db.execute(
                "SELECT count(*) FROM tokenizer_catalog.text_analyzer WHERE name = 'prepared_analyzer'"
            ).fetchone()[0],
            0,
        )

    def test_direct_catalog_prepare_cache_cleanup(self):
        if self.db.execute("SHOW max_prepared_transactions").fetchone()[0] == "0":
            self.skipTest("max_prepared_transactions = 0")
        query = "SELECT apply_text_analyzer('Hello World', %s)"
        version_query = (
            "SELECT version FROM tokenizer_catalog.definition_version WHERE singleton"
        )
        update = "UPDATE tokenizer_catalog.text_analyzer SET config = %s WHERE name = %s"
        prepared_config = json.dumps({
            "character_filters": ["to_lowercase"],
            "pre_tokenizer": "unicode_segmentation",
            "token_filters": [],
        })
        replacement_config = json.dumps({
            "character_filters": [],
            "pre_tokenizer": {"regex": "[A-Z]"},
            "token_filters": [],
        })
        for completion in ("ROLLBACK", "COMMIT"):
            with self.subTest(completion=completion), self.connect() as writer:
                analyzer = "prepared_" + completion.lower()
                gid = self.dbname + "_" + completion.lower()
                self.db.execute(
                    "SELECT create_text_analyzer(%s, 'pre_tokenizer = \"unicode_segmentation\"')",
                    (analyzer,),
                )
                original_version = self.db.execute(version_query).fetchone()[0]
                writer.execute("BEGIN")
                writer.execute(update, (prepared_config, analyzer))
                prepared_version = writer.execute(version_query).fetchone()[0]
                self.assertEqual(prepared_version, original_version + 1)
                self.assertEqual(
                    writer.execute(query, (analyzer,)).fetchone()[0], ["hello", "world"]
                )
                writer.execute(sql.SQL("PREPARE TRANSACTION {}").format(sql.Literal(gid)))
                pending = True
                try:
                    self.db.execute(
                        sql.SQL("{} PREPARED {}").format(sql.SQL(completion), sql.Literal(gid))
                    )
                    pending = False
                    if completion == "ROLLBACK":
                        self.assertEqual(
                            self.db.execute(version_query).fetchone()[0], original_version
                        )
                    else:
                        self.assertEqual(
                            self.db.execute(query, (analyzer,)).fetchone()[0],
                            ["hello", "world"],
                        )
                        self.assertEqual(
                            writer.execute(query, (analyzer,)).fetchone()[0],
                            ["hello", "world"],
                        )
                    self.db.execute(update, (replacement_config, analyzer))
                    self.assertEqual(
                        self.db.execute(version_query).fetchone()[0],
                        prepared_version + (completion == "COMMIT"),
                    )
                    self.assertEqual(
                        self.db.execute(query, (analyzer,)).fetchone()[0], ["H", "W"]
                    )
                    for _ in range(2):
                        with writer.transaction():
                            for _ in range(2):
                                self.assertEqual(
                                    writer.execute(query, (analyzer,)).fetchone()[0],
                                    ["H", "W"],
                                )
                finally:
                    if pending:
                        self.db.execute(
                            sql.SQL("ROLLBACK PREPARED {}").format(sql.Literal(gid))
                        )

    def test_drop_custom_model_name_handling(self):
        # Invalid and reserved names keep the 1.0.0 warning; a valid name that
        # does not exist now warns instead of panicking. Tokenizers are kept.
        self.db.execute("SELECT create_tokenizer('kept_tokenizer', 'model = \"bert_base_uncased\"')")
        notices = []
        self.db.add_notice_handler(
            lambda d: notices.append((d.severity_nonlocalized, d.message_primary))
        )
        for name in ("1invalid", "bad-name", "llmlingua2", "missing_model"):
            self.db.execute("SELECT drop_custom_model(%s)", (name,))
        self.assertEqual(notices, [
            ("WARNING", "Invalid model name: 1invalid, Details: Name must start with a letter"),
            ("WARNING", "Invalid model name: bad-name, Details: Invalid character: -"),
            ("WARNING", "Invalid model name: llmlingua2, Details: The name is reserved, please choose another name"),
            ("WARNING", "Model not found: missing_model"),
        ])
        self.assertEqual(
            self.db.execute("SELECT tokenize('hello', 'kept_tokenizer')").fetchone()[0], [7592]
        )

    def test_drop_custom_model_after_source_table_drop(self):
        self.make_model(self.db, "dropped_source")
        self.db.execute("DROP TABLE dropped_source")
        self.db.execute("SELECT drop_custom_model('dropped_source')")
        self.assertIsNone(
            self.db.execute("SELECT to_regclass('tokenizer_data.model_dropped_source')").fetchone()[0]
        )
        # Older versions stored literal (not SQL-quoted) table names.
        self.db.execute('CREATE TABLE "O\'Reilly docs" (body text)')
        self.db.execute(
            "SELECT create_custom_model('legacy_source', %s)",
            ('table = "O\'Reilly docs"\ncolumn = "body"\ntext_analyzer = "dropped_source"',),
        )
        self.db.execute(
            "UPDATE tokenizer_catalog.model SET config = "
            "jsonb_set(config::jsonb, '{custom,table}', to_jsonb(%s::text))::text "
            "WHERE name = 'legacy_source'", ("O'Reilly docs",),
        )
        self.db.execute('DROP TABLE "O\'Reilly docs"')
        self.db.execute("SELECT drop_custom_model('legacy_source')")
        self.assertIsNone(self.db.execute(
            "SELECT to_regclass('tokenizer_data.model_legacy_source')"
        ).fetchone()[0])

    def test_writers_without_new_tokens_do_not_wait(self):
        self.make_model(self.db)
        self.db.execute("INSERT INTO race(body) VALUES ('alpha beta')")
        with self.connect() as a, self.connect() as b:
            a.execute("BEGIN")
            a.execute("INSERT INTO race(body) VALUES ('gamma')")
            b.execute("SET lock_timeout = '1s'")
            b.execute("INSERT INTO race(body) VALUES ('beta alpha')")
            with self.assertRaises(psycopg.errors.LockNotAvailable):
                b.execute("INSERT INTO race(body) VALUES ('delta')")
            a.execute("COMMIT")
        self.assert_embeddings(self.db)

    def test_owner_cannot_capture_trigger_calls_or_preload_list(self):
        self.admin.execute(
            sql.SQL("ALTER DATABASE {} OWNER TO {}").format(
                sql.Identifier(self.dbname), sql.Identifier(self.owner)
            )
        )
        self.db.execute("CREATE TABLE typed_docs (body varchar, embedding int[])")
        self.db.execute("SELECT create_text_analyzer('typed_docs', 'pre_tokenizer = \"unicode_segmentation\"')")
        with self.connect(user=self.owner) as owner:
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                owner.execute("""CREATE FUNCTION tokenizer_catalog.apply_text_analyzer_for_custom_model(v varchar, a text)
                    RETURNS text[] LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'captured'; END $$""")
            owner.execute("""CREATE FUNCTION public.apply_text_analyzer_for_custom_model(v varchar, a text)
                RETURNS text[] LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'captured'; END $$""")
            for function in ("add_preload_model(text)", "remove_preload_model(text)"):
                self.assertFalse(
                    owner.execute(
                        "SELECT has_function_privilege(%s, 'EXECUTE')",
                        (f"tokenizer_catalog.{function}",),
                    ).fetchone()[0]
                )
        self.db.execute("INSERT INTO typed_docs(body) VALUES ('hello world')")
        self.db.execute(
            "SELECT create_custom_model_tokenizer_and_trigger('typed_docs', 'typed_docs', 'typed_docs', 'typed_docs', 'body', 'embedding')"
        )
        self.db.execute("INSERT INTO typed_docs(body) VALUES ('hello world')")
        self.assertEqual(
            self.db.execute("SELECT count(*) FROM tokenizer_data.model_typed_docs").fetchone()[0],
            2,
        )

    def test_native_vector_skips_overlong_lexemes(self):
        self.db.execute("SELECT create_text_analyzer('long_words', 'pre_tokenizer = \"unicode_segmentation\"')")
        self.assertEqual(
            self.db.execute(
                "SELECT analyze_to_tsvector(repeat('a', 3000) || ' b c', 'long_words')::text"
            ).fetchone()[0],
            "'b':1 'c':2",
        )

    def test_analyzer_cached_plan_observes_redefinition(self):
        self.db.execute("SELECT create_text_analyzer('plan_analyzer', '')")
        self.db.execute("SET plan_cache_mode = force_generic_plan")
        self.db.execute("PREPARE analyzer_plan AS SELECT apply_text_analyzer('UPPER', 'plan_analyzer')")
        self.assertEqual(self.db.execute("EXECUTE analyzer_plan").fetchone()[0], ["UPPER"])
        with self.connect() as writer, writer.transaction():
            writer.execute("SELECT drop_text_analyzer('plan_analyzer')")
            writer.execute("SELECT create_text_analyzer('plan_analyzer', 'character_filters = [\"to_lowercase\"]')")
        self.assertEqual(self.db.execute("EXECUTE analyzer_plan").fetchone()[0], ["upper"])

    def test_read_committed_refreshes_after_long_statement(self):
        self.db.execute("SELECT create_tokenizer('slow_snapshot', 'model = \"bert_base_uncased\"')")
        with self.connect() as reader:
            reader.execute("BEGIN")
            # pgrx select() otherwise silently takes fresh snapshots after an
            # XID is assigned. Definitions must still use this statement's snapshot.
            reader.execute("CREATE TEMP TABLE snapshot_write (n integer)")
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(
                    lambda: reader.execute(
                        "WITH slept AS MATERIALIZED (SELECT pg_sleep(2)) "
                        "SELECT tokenize('hello', 'slow_snapshot') FROM slept; "
                        "SELECT tokenize('hello', 'slow_snapshot')"
                    )
                )
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    wait = self.db.execute(
                        "SELECT wait_event FROM pg_stat_activity WHERE pid = %s",
                        (reader.info.backend_pid,),
                    ).fetchone()[0]
                    if wait == "PgSleep":
                        break
                    time.sleep(0.01)
                self.assertEqual(wait, "PgSleep")
                with self.db.transaction():
                    self.db.execute("SELECT drop_tokenizer('slow_snapshot')")
                    self.db.execute("SELECT create_tokenizer('slow_snapshot', 'model = \"llmlingua2\"')")
                expected = self.db.execute("SELECT tokenize('hello', 'slow_snapshot')").fetchone()[0]
                result = future.result(timeout=10)
                self.assertNotEqual(result.fetchone()[0], expected)
                self.assertTrue(result.nextset())
                self.assertEqual(result.fetchone()[0], expected)
            # A new READ COMMITTED statement has a new snapshot, even before COMMIT.
            self.assertEqual(reader.execute("SELECT tokenize('hello', 'slow_snapshot')").fetchone()[0], expected)
            reader.execute("COMMIT")
            self.assertEqual(reader.execute("SELECT tokenize('hello', 'slow_snapshot')").fetchone()[0], expected)

    def test_upgrade_hardens_privileges_and_volatility(self):
        self.make_model(self.db)
        self.db.execute("INSERT INTO race(body) VALUES ('alpha beta')")
        original = self.db.execute("SELECT id, token FROM model_race ORDER BY id").fetchall()
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT id, token FROM model_race ORDER BY id").fetchall(), original)
        self.db.execute("INSERT INTO race(body) VALUES ('gamma alpha')")
        self.assert_embeddings(self.db)
        self.assertEqual(self.db.execute(
            "SELECT provolatile FROM pg_proc WHERE oid = 'apply_text_analyzer(text,text)'::regprocedure"
        ).fetchone()[0], "s")
        with self.connect(user=self.owner) as owner:
            for function in ("add_preload_model", "remove_preload_model"):
                self.assertFalse(owner.execute(
                    "SELECT has_function_privilege(%s, 'EXECUTE')",
                    (f"tokenizer_catalog.{function}(text)",),
                ).fetchone()[0])

    def test_prepare_read_only_cache_cleanup(self):
        if self.db.execute("SHOW max_prepared_transactions").fetchone()[0] == "0":
            self.skipTest("max_prepared_transactions = 0")
        self.db.execute("SELECT create_tokenizer('prepared_read', 'model = \"bert_base_uncased\"')")
        gid = self.dbname + "_read"
        with self.connect() as reader:
            reader.execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
            reader.execute("SELECT count(*) FROM tokenizer_catalog.tokenizer")
            with self.db.transaction():
                self.db.execute("SELECT drop_tokenizer('prepared_read')")
                self.db.execute("SELECT create_tokenizer('prepared_read', 'model = \"llmlingua2\"')")
            expected = self.db.execute("SELECT tokenize('hello', 'prepared_read')").fetchone()[0]
            self.assertNotEqual(reader.execute("SELECT tokenize('hello', 'prepared_read')").fetchone()[0], expected)
            reader.execute(sql.SQL("PREPARE TRANSACTION {}").format(sql.Literal(gid)))
            try:
                self.assertEqual(reader.execute("SELECT tokenize('hello', 'prepared_read')").fetchone()[0], expected)
            finally:
                self.db.execute(sql.SQL("ROLLBACK PREPARED {}").format(sql.Literal(gid)))

    def test_parallel_query_after_invalidation(self):
        # Parallel workers start with a stale local epoch and must not try to
        # mark command IDs used while handling it.
        self.db.execute("SELECT create_text_analyzer('parallel_words', 'pre_tokenizer = \"unicode_segmentation\"')")
        self.db.execute("SELECT create_tokenizer('parallel_tokenizer', 'model = \"bert_base_uncased\"')")
        self.db.execute("CREATE TABLE parallel_docs AS SELECT 'hello world ' || g AS body FROM generate_series(1, 1000) g")
        self.db.execute("SET client_min_messages = error")
        self.db.execute("SELECT drop_text_analyzer('missing_analyzer')")
        query = "SELECT sum(cardinality(tokenize(body, 'parallel_tokenizer'))) FROM parallel_docs"
        expected = self.db.execute(query).fetchone()[0]
        with self.connect() as conn:
            # Renamed from force_parallel_mode in PostgreSQL 16.
            setting = "debug_parallel_query" if conn.info.server_version >= 160000 else "force_parallel_mode"
            conn.execute(f"SET {setting} = on")
            self.assertEqual(conn.execute(query).fetchone()[0], expected)
            self.assertEqual(conn.execute(
                "SELECT count(*) FROM parallel_docs WHERE analyze_to_tsvector(body, 'parallel_words') @@ 'hello'"
            ).fetchone()[0], 1000)

    def test_parallel_leader_custom_model_after_writes(self):
        # Once a transaction has an XID, pgrx SPI is read-write. A leader that
        # executes part of a parallel plan must still read vocabularies with
        # read-only SPI: PostgreSQL forbids advancing the command counter there.
        self.db.execute("CREATE TABLE parallel_vocab AS SELECT g AS id, 'w' || (g % 50) || ' x' || (g % 7) AS body FROM generate_series(1, 30000) g")
        self.db.execute("ANALYZE parallel_vocab")
        self.db.execute("SELECT create_text_analyzer('parallel_vocab', 'pre_tokenizer = \"unicode_segmentation\"')")
        self.db.execute("SELECT create_custom_model('parallel_vocab', 'table = \"parallel_vocab\"\ncolumn = \"body\"\ntext_analyzer = \"parallel_vocab\"')")
        self.db.execute("SELECT create_tokenizer('parallel_vocab', 'text_analyzer = \"parallel_vocab\"\nmodel = \"parallel_vocab\"')")
        query = "SELECT count(*), sum(cardinality(tokenize(body, 'parallel_vocab'))) FROM parallel_vocab"
        expected = self.db.execute(query).fetchone()
        self.assertEqual(expected, (30000, 60000))
        with self.connect() as conn:
            for setting in ["parallel_setup_cost = 0", "parallel_tuple_cost = 0",
                            "min_parallel_table_scan_size = 0", "max_parallel_workers_per_gather = 2",
                            "parallel_leader_participation = on"]:
                conn.execute("SET " + setting)
            plan = conn.execute("EXPLAIN (FORMAT JSON) " + query).fetchone()[0]
            self.assertIn("Gather", json.dumps(plan))
            with conn.transaction():
                conn.execute("SELECT pg_current_xact_id()")
                self.assertEqual(conn.execute(query).fetchone(), expected)
            # CREATE TABLE AS and REFRESH mark the command ID used themselves.
            conn.execute("CREATE TABLE parallel_copy AS SELECT body, tokenize(body, 'parallel_vocab') AS ids FROM parallel_vocab")
            conn.execute("CREATE MATERIALIZED VIEW parallel_view AS SELECT body, tokenize(body, 'parallel_vocab') AS ids FROM parallel_vocab WITH NO DATA")
            conn.execute("REFRESH MATERIALIZED VIEW parallel_view")
            for relation in ["parallel_copy", "parallel_view"]:
                self.assertEqual(conn.execute(sql.SQL("SELECT count(*), sum(cardinality(ids)), count(*) FILTER (WHERE ids IS DISTINCT FROM tokenize(body, 'parallel_vocab')) FROM {}").format(sql.Identifier(relation))).fetchone(),
                                 (30000, 60000, 0))

    def test_catalog_writers_are_parallel_unsafe(self):
        writers = self.db.execute("""
            SELECT proname, proparallel FROM pg_proc
            WHERE pronamespace = 'tokenizer_catalog'::regnamespace
              AND (proname LIKE 'create_%' OR proname LIKE 'drop_%'
                   OR proname IN ('add_preload_model', 'remove_preload_model',
                                  'custom_model_insert_trigger',
                                  'custom_model_tokenizer_set_target_column_trigger'))
            ORDER BY proname
        """).fetchall()
        self.assertGreaterEqual(len(writers), 19)
        self.assertEqual([(name, flag) for name, flag in writers if flag != 'u'], [])
        self.db.execute("CREATE TABLE writer_plan AS SELECT g FROM generate_series(1, 10) g")
        self.db.execute("SET min_parallel_table_scan_size = 0")
        self.db.execute("SET parallel_setup_cost = 0")
        self.db.execute("SET parallel_tuple_cost = 0")
        setting = "debug_parallel_query" if self.db.info.server_version >= 160000 else "force_parallel_mode"
        self.db.execute(f"SET {setting} = on")
        plan = self.db.execute("""EXPLAIN (FORMAT JSON)
            SELECT create_stopwords('parallel_' || g, 'the') FROM writer_plan""").fetchone()[0]
        self.assertNotIn('Gather', json.dumps(plan))

    def test_pg_dict_pins_schema(self):
        self.db.execute("CREATE SCHEMA dictionary_a; CREATE SCHEMA dictionary_b")
        self.db.execute("CREATE TEXT SEARCH DICTIONARY dictionary_a.words (TEMPLATE = snowball, LANGUAGE = english)")
        self.db.execute("CREATE TEXT SEARCH DICTIONARY dictionary_b.words (TEMPLATE = simple)")
        self.db.execute("SET search_path = dictionary_a, tokenizer_catalog")
        self.db.execute("SELECT create_text_analyzer('dict_bound', '[[token_filters]]\npg_dict = \"words\"')")
        query = "SELECT tokenizer_catalog.apply_text_analyzer('running', 'dict_bound')"
        self.assertEqual(self.db.execute(query).fetchone()[0], ['run'])
        with self.connect() as reader:
            reader.execute("SET search_path = dictionary_b, tokenizer_catalog")
            self.assertEqual(reader.execute(query).fetchone()[0], ['run'])

    def test_pg_dict_refreshes_oid_after_ddl(self):
        self.db.execute("CREATE SCHEMA dictionary_a")
        self.db.execute("CREATE TEXT SEARCH DICTIONARY dictionary_a.words (TEMPLATE = snowball, LANGUAGE = english)")
        self.db.execute("SELECT create_text_analyzer('dict_bound', '[[token_filters]]\npg_dict = \"dictionary_a.words\"')")
        query = "SELECT tokenizer_catalog.apply_text_analyzer('running', 'dict_bound')"
        with self.connect() as reader:
            self.assertEqual(reader.execute(query).fetchone()[0], ['run'])
            # The legacy epoch path discards objects after the first observed
            # epoch. Warm a second statement so DDL encounters a retained OID.
            self.assertEqual(reader.execute(query).fetchone()[0], ['run'])
            with self.db.transaction():
                self.db.execute("DROP TEXT SEARCH DICTIONARY dictionary_a.words")
                self.db.execute("CREATE TEXT SEARCH DICTIONARY dictionary_a.words (TEMPLATE = simple)")
            self.assertEqual(reader.execute(query).fetchone()[0], ['running'])
            self.db.execute("ALTER TEXT SEARCH DICTIONARY dictionary_a.words (ACCEPT = false)")
            self.assertEqual(reader.execute(query.replace('running', 'UPPER')).fetchone()[0], ['UPPER'])

    def test_analysis_budgets(self):
        self.db.execute("SELECT create_text_analyzer('bounded_plain', '')")
        self.db.execute("SELECT create_text_analyzer('bounded_grams', '[[token_filters]]\nngram = {min_gram = 1, max_gram = 2}')")
        self.db.execute("SELECT create_text_analyzer('bounded_regex', 'pre_tokenizer = {regex = \".\"}')")
        cases = [
            ('max_input_bytes', 1024, 'bounded_plain', 'a' * 1025),
            ('max_output_tokens', 8, 'bounded_grams', 'abcdef'),
            ('max_output_tokens', 8, 'bounded_regex', 'abcdefghi'),
            ('max_output_bytes', 64, 'bounded_plain', 'a' * 65),
            ('max_piece_bytes', 8, 'bounded_regex', 'abcdefghi'),
        ]
        for setting, limit, analyzer, text in cases:
            with self.subTest(setting=setting, analyzer=analyzer):
                self.db.execute(f"SET pg_tokenizer.{setting} = {limit}")
                try:
                    with self.assertRaises(psycopg.errors.ProgramLimitExceeded):
                        self.db.execute("SELECT apply_text_analyzer(%s, %s)", (text, analyzer))
                    self.assertEqual(self.db.execute('SELECT 1').fetchone()[0], 1)
                finally:
                    self.db.execute(f"RESET pg_tokenizer.{setting}")

    def test_huggingface_padding_budget(self):
        config = json.dumps({
            'model': {'type': 'WordLevel', 'vocab': {'[UNK]': 0, 'hello': 1}, 'unk_token': '[UNK]'},
            'padding': {'strategy': {'Fixed': 1000}, 'direction': 'Right',
                        'pad_to_multiple_of': None, 'pad_id': 0, 'pad_type_id': 0, 'pad_token': '[PAD]'},
        })
        self.db.execute("SELECT create_huggingface_model('pad_budget', %s)", (config,))
        self.db.execute("SELECT create_tokenizer('pad_budget', 'model = \"pad_budget\"')")
        self.db.execute("SET pg_tokenizer.max_output_tokens = 8")
        with self.assertRaises(psycopg.errors.ProgramLimitExceeded):
            self.db.execute("SELECT tokenize('hello', 'pad_budget')")

    def test_upgrade_from_100_moves_runtime_objects(self):
        with self.connect(user=self.owner) as owner:
            self.make_model(owner, 'owned')
            owner.execute("INSERT INTO owned(body) VALUES ('alpha beta')")
            owner.execute(sql.SQL("GRANT SELECT ON tokenizer_catalog.model_owned TO {}").format(sql.Identifier(self.outsider)))
        self.db.execute(sql.SQL("GRANT USAGE, CREATE ON SCHEMA tokenizer_catalog TO {} WITH GRANT OPTION").format(sql.Identifier(self.outsider)))
        self.db.execute("CREATE VIEW vocabulary_view AS SELECT id, token FROM tokenizer_catalog.model_owned")
        before = self.db.execute("SELECT 'tokenizer_catalog.model_owned'::regclass::oid, pg_get_serial_sequence('tokenizer_catalog.model_owned','id')::regclass::oid").fetchone()
        identity = self.db.execute("SELECT oid, relowner, relacl FROM pg_class WHERE oid = ANY(%s) ORDER BY oid", (list(before),)).fetchall()
        vocabulary = self.db.execute("SELECT * FROM vocabulary_view ORDER BY id").fetchall()
        with self.connect(user=self.owner) as reader:
            tokens = reader.execute("SELECT tokenize('alpha beta','owned')").fetchone()[0]
            self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
            self.assertEqual(reader.execute("SELECT tokenize('alpha beta','owned')").fetchone()[0], tokens)
            reader.execute("INSERT INTO owned(body) VALUES ('gamma alpha')")
            self.assert_embeddings(reader, 'owned')
            self.assertFalse(reader.execute("SELECT has_schema_privilege(current_user,'tokenizer_catalog','CREATE')").fetchone()[0])
        after = self.db.execute("SELECT 'tokenizer_data.model_owned'::regclass::oid, pg_get_serial_sequence('tokenizer_data.model_owned','id')::regclass::oid").fetchone()
        self.assertEqual(before, after)
        self.assertEqual(self.db.execute("SELECT oid, relowner, relacl FROM pg_class WHERE oid = ANY(%s) ORDER BY oid", (list(after),)).fetchall(), identity)
        self.assertEqual(self.db.execute("SELECT * FROM vocabulary_view WHERE token <> 'gamma' ORDER BY id").fetchall(), vocabulary)
        self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_catalog.model_owned')").fetchone()[0])
        with self.connect(user=self.outsider) as reader:
            self.assertEqual(reader.execute("SELECT count(*) FROM tokenizer_data.model_owned").fetchone()[0], 3)
            self.assertFalse(reader.execute("SELECT has_schema_privilege(current_user,'tokenizer_catalog','CREATE')").fetchone()[0])
            self.assertTrue(reader.execute("SELECT has_schema_privilege(current_user,'tokenizer_data','CREATE WITH GRANT OPTION')").fetchone()[0])
        self.db.execute("DROP VIEW vocabulary_view")
        with self.connect(user=self.owner) as owner:
            owner.execute("SELECT drop_custom_model('owned')")
        self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_data.model_owned')").fetchone()[0])

    def test_upgrade_from_100_runtime_schema_collision_rolls_back(self):
        self.make_model(self.db)
        self.db.execute("INSERT INTO race(body) VALUES ('alpha beta')")
        self.db.execute("CREATE SCHEMA tokenizer_data; CREATE TABLE tokenizer_data.marker(id int)")
        with self.assertRaises(psycopg.errors.DuplicateSchema):
            self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT extversion FROM pg_extension WHERE extname='pg_tokenizer'").fetchone()[0], '1.0.0')
        self.assertEqual(self.db.execute("SELECT tokenize('alpha beta','race')").fetchone()[0], [1, 2])
        self.assertIsNotNone(self.db.execute("SELECT to_regclass('tokenizer_data.marker')").fetchone()[0])
        self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_catalog.definition_version')").fetchone()[0])
        self.db.execute("DROP SCHEMA tokenizer_data CASCADE")
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT tokenize('alpha beta','race')").fetchone()[0], [1, 2])

    def test_upgrade_from_100_serializes_custom_model_creation(self):
        self.creation_during_upgrade()

    def test_upgrade_from_100_retries_repeatable_creator(self):
        self.creation_during_upgrade('REPEATABLE READ')

    def test_upgrade_from_100_retries_serializable_creator(self):
        self.creation_during_upgrade('SERIALIZABLE')

    def creation_during_upgrade(self, isolation=None):
        self.db.execute("CREATE TABLE during_upgrade(body text)")
        self.db.execute("SELECT create_text_analyzer('during_upgrade','')")
        with self.connect() as creator:
            if isolation:
                creator.execute(f'BEGIN ISOLATION LEVEL {isolation}')
                creator.execute('SELECT count(*) FROM tokenizer_catalog.model')
            self.db.execute('BEGIN')
            self.db.execute('LOCK TABLE tokenizer_catalog.model IN SHARE ROW EXCLUSIVE MODE')
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(creator.execute, "SELECT create_custom_model('during_upgrade', %s)",
                                     ('table = "during_upgrade"\ncolumn = "body"\ntext_analyzer = "during_upgrade"',))
                try:
                    deadline = time.monotonic() + 5
                    wait = None
                    while time.monotonic() < deadline:
                        wait = self.db.execute('SELECT wait_event FROM pg_stat_activity WHERE pid=%s', (creator.info.backend_pid,)).fetchone()[0]
                        if wait == 'relation' or future.done():
                            break
                        time.sleep(0.02)
                    self.assertEqual(wait, 'relation')
                    self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
                    self.db.execute('COMMIT')
                    with self.assertRaises(psycopg.errors.SerializationFailure):
                        future.result(timeout=10)
                finally:
                    self.db.execute('ROLLBACK')
            self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_data.model_during_upgrade')").fetchone()[0])
            if isolation:
                creator.execute('ROLLBACK')
            creator.execute("SELECT create_custom_model('during_upgrade', %s)",
                            ('table = "during_upgrade"\ncolumn = "body"\ntext_analyzer = "during_upgrade"',))
        self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_catalog.model_during_upgrade')").fetchone()[0])
        self.assertIsNotNone(self.db.execute("SELECT to_regclass('tokenizer_data.model_during_upgrade')").fetchone()[0])
        self.db.execute("INSERT INTO during_upgrade VALUES ('new token')")
        self.assertEqual(self.db.execute("SELECT token FROM tokenizer_data.model_during_upgrade").fetchall(), [('new token',)])

    def test_upgrade_from_100_requires_read_committed(self):
        # Under REPEATABLE READ/SERIALIZABLE the migration's catalog scan would
        # reuse a snapshot taken before its lock wait and miss models committed
        # meanwhile, leaving them in tokenizer_catalog. The upgrade refuses.
        self.make_model(self.db, 'before_upgrade')
        version = "SELECT extversion FROM pg_extension WHERE extname = 'pg_tokenizer'"
        for isolation in ['REPEATABLE READ', 'SERIALIZABLE']:
            with self.subTest(isolation=isolation):
                self.db.execute(f'BEGIN ISOLATION LEVEL {isolation}')
                try:
                    with self.assertRaisesRegex(psycopg.errors.InvalidTransactionState, 'requires READ COMMITTED'):
                        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
                finally:
                    self.db.execute('ROLLBACK')
                self.assertEqual(self.db.execute(version).fetchone()[0], '1.0.0')
        # A database default isolation level is refused the same way.
        self.admin.execute(sql.SQL("ALTER DATABASE {} SET default_transaction_isolation = 'serializable'").format(
            sql.Identifier(self.dbname)))
        with self.connect() as upgrader:
            with self.assertRaises(psycopg.errors.InvalidTransactionState):
                upgrader.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
            self.assertEqual(upgrader.execute(version).fetchone()[0], '1.0.0')
            # READ COMMITTED takes a new snapshot after the lock, so a model
            # committed after the transaction's first snapshot is still moved.
            upgrader.execute('BEGIN ISOLATION LEVEL READ COMMITTED')
            upgrader.execute('SELECT count(*) FROM tokenizer_catalog.model')
            self.make_model(self.db, 'after_snapshot')
            upgrader.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
            upgrader.execute('COMMIT')
        self.assertEqual(self.db.execute(version).fetchone()[0], '1.1.0')
        self.assertEqual(self.db.execute("""SELECT relname FROM pg_class
            WHERE relnamespace = 'tokenizer_catalog'::regnamespace AND relkind = 'r'
              AND relname LIKE 'model\\_%'""").fetchall(), [])
        for name in ['before_upgrade', 'after_snapshot']:
            self.assertIsNotNone(self.db.execute("SELECT to_regclass(%s)", (f'tokenizer_data.model_{name}',)).fetchone()[0])
            self.db.execute(sql.SQL("INSERT INTO {}(body) VALUES ('fresh words')").format(sql.Identifier(name)))
            self.assert_embeddings(self.db, name)

    def test_upgrade_from_100_retries_old_snapshot(self):
        self.db.execute("SELECT create_text_analyzer('before_upgrade', '')")
        with self.connect() as reader:
            reader.execute('BEGIN ISOLATION LEVEL REPEATABLE READ')
            self.assertEqual(reader.execute("SELECT apply_text_analyzer('hello', 'before_upgrade')").fetchone()[0], ['hello'])
            self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
            # The new relation is name-resolvable, but neither its catalog row
            # nor its singleton is visible to this pre-upgrade snapshot.
            self.assertEqual(reader.execute("SELECT count(*) FROM tokenizer_catalog.definition_version").fetchone()[0], 0)
            self.assertEqual(reader.execute("SELECT oid FROM pg_class WHERE relname='definition_version'").fetchall(), [])
            with self.assertRaises(psycopg.errors.SerializationFailure):
                reader.execute("SELECT apply_text_analyzer('hello', 'before_upgrade')")
            reader.execute('ROLLBACK')
            self.assertEqual(reader.execute("SELECT apply_text_analyzer('hello', 'before_upgrade')").fetchone()[0], ['hello'])
        # A genuinely missing singleton is still an error, not a legacy fallback.
        self.db.execute('DELETE FROM tokenizer_catalog.definition_version')
        with self.assertRaisesRegex(psycopg.errors.InternalError_, 'definition version is missing'):
            self.db.execute("SELECT apply_text_analyzer('hello', 'before_upgrade')")

    def test_upgrade_from_100_waits_for_existing_creator(self):
        with self.connect() as creator:
            creator.execute('BEGIN')
            self.make_model(creator)
            creator.execute("INSERT INTO race(body) VALUES ('alpha beta')")
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                future = pool.submit(self.db.execute, "ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
                try:
                    deadline = time.monotonic() + 5
                    wait = None
                    while time.monotonic() < deadline:
                        creator.execute('SELECT pg_stat_clear_snapshot()')
                        wait = creator.execute('SELECT wait_event FROM pg_stat_activity WHERE pid=%s', (self.db.info.backend_pid,)).fetchone()[0]
                        if wait == 'relation' or future.done():
                            break
                        time.sleep(0.02)
                    self.assertEqual(wait, 'relation')
                    creator.execute('COMMIT')
                    future.result(timeout=10)
                finally:
                    creator.execute('ROLLBACK')
            self.assert_embeddings(creator)
        self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_catalog.model_race')").fetchone()[0])
        self.assertEqual(self.db.execute("SELECT token FROM tokenizer_data.model_race ORDER BY id").fetchall(), [('alpha',), ('beta',)])

    def test_upgrade_from_100_rolls_back_partial_moves(self):
        self.make_model(self.db, 'a_valid')
        self.make_model(self.db, 'z_broken')
        before = self.db.execute("SELECT 'tokenizer_catalog.model_a_valid'::regclass::oid").fetchone()[0]
        self.db.execute('DROP TABLE tokenizer_catalog.model_z_broken')
        with self.assertRaises(psycopg.errors.UndefinedTable):
            self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT extversion FROM pg_extension WHERE extname='pg_tokenizer'").fetchone()[0], '1.0.0')
        self.assertEqual(self.db.execute("SELECT 'tokenizer_catalog.model_a_valid'::regclass::oid").fetchone()[0], before)
        self.assertIsNone(self.db.execute("SELECT to_regnamespace('tokenizer_data')").fetchone()[0])
        self.assertIsNone(self.db.execute("SELECT to_regclass('tokenizer_catalog.definition_version')").fetchone()[0])
        self.db.execute("SELECT drop_custom_model('z_broken')")
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT 'tokenizer_data.model_a_valid'::regclass::oid").fetchone()[0], before)

    def test_upgrade_from_100_pins_dictionary_names(self):
        self.db.execute("CREATE SCHEMA dictionary_upgrade")
        self.db.execute("CREATE TEXT SEARCH DICTIONARY dictionary_upgrade.words (TEMPLATE = simple)")
        # Simulate a pre-1.1.0 catalog without relying on the new create API.
        self.db.execute("INSERT INTO tokenizer_catalog.text_analyzer VALUES ('legacy_dict', %s)",
                        (json.dumps({'token_filters': [{'pg_dict': 'words'}]}),))
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT config::jsonb->'token_filters'->0->>'pg_dict' FROM tokenizer_catalog.text_analyzer WHERE name='legacy_dict'").fetchone()[0], 'dictionary_upgrade.words')
        self.assertEqual(self.db.execute("SELECT apply_text_analyzer('UPPER','legacy_dict')").fetchone()[0], ['upper'])
        self.assertEqual(self.db.execute("SELECT count(*) FROM tokenizer_catalog.definition_version").fetchone()[0], 1)
        self.test_catalog_writers_are_parallel_unsafe()

    def test_upgrade_from_100_rejects_ambiguous_dictionary(self):
        self.db.execute("CREATE SCHEMA dictionary_a; CREATE SCHEMA dictionary_b")
        for schema in ('dictionary_a', 'dictionary_b'):
            self.db.execute(sql.SQL("CREATE TEXT SEARCH DICTIONARY {}.words (TEMPLATE = simple)").format(sql.Identifier(schema)))
        self.db.execute("INSERT INTO tokenizer_catalog.text_analyzer VALUES ('ambiguous_dict', %s)",
                        (json.dumps({'token_filters': [{'pg_dict': 'words'}]}),))
        with self.assertRaisesRegex(psycopg.errors.RaiseException, 'missing or ambiguous'):
            self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT extversion FROM pg_extension WHERE extname = 'pg_tokenizer'").fetchone()[0], '1.0.0')
        self.db.execute("UPDATE tokenizer_catalog.text_analyzer SET config=%s WHERE name='ambiguous_dict'",
                        (json.dumps({'token_filters': [{'pg_dict': 'dictionary_a.words'}]}),))
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")

    def test_upgrade_from_100_rejects_shadowed_builtin_dictionary(self):
        self.db.execute("CREATE SCHEMA dictionary_shadow")
        self.db.execute("CREATE TEXT SEARCH DICTIONARY dictionary_shadow.simple (TEMPLATE = simple, ACCEPT = false)")
        model = {'model': {'type': 'WordLevel', 'vocab': {'[UNK]': 0, 'UPPER': 7, 'upper': 8}, 'unk_token': '[UNK]'}}
        self.db.execute("SELECT create_huggingface_model('shadow_model', %s)", (json.dumps(model),))
        analyzer = {'token_filters': [{'pg_dict': 'simple'}]}
        tokenizer = dict(analyzer, model='shadow_model')
        for table, config in [('text_analyzer', analyzer), ('tokenizer', tokenizer)]:
            self.db.execute(sql.SQL("INSERT INTO tokenizer_catalog.{} VALUES ('shadowed_builtin', %s)").format(sql.Identifier(table)),
                            (json.dumps(config),))
        # Both lazy runtime loads must refuse to guess the old creator's path.
        for function in ('apply_text_analyzer', 'tokenize'):
            with self.subTest(function=function):
                with self.assertRaisesRegex(psycopg.errors.InvalidParameterValue, 'does not identify one dictionary'):
                    self.db.execute(sql.SQL("SELECT {}('UPPER', 'shadowed_builtin')").format(sql.Identifier(function)))
        # Check each migration branch independently; the analyzer must not mask
        # a missed ambiguity in an inline tokenizer definition.
        for table, config in [('text_analyzer', analyzer), ('tokenizer', tokenizer)]:
            with self.subTest(table=table):
                with self.assertRaisesRegex(psycopg.errors.RaiseException, f'in {table} shadowed_builtin is missing or ambiguous'):
                    self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
                self.assertEqual(self.db.execute("SELECT extversion FROM pg_extension WHERE extname = 'pg_tokenizer'").fetchone()[0], '1.0.0')
                config['token_filters'] = [{'pg_dict': 'dictionary_shadow.simple'}]
                self.db.execute(sql.SQL("UPDATE tokenizer_catalog.{} SET config=%s WHERE name='shadowed_builtin'").format(sql.Identifier(table)),
                                (json.dumps(config),))
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(self.db.execute("SELECT apply_text_analyzer('UPPER', 'shadowed_builtin')").fetchone()[0], ['UPPER'])
        self.assertEqual(self.db.execute("SELECT tokenize('UPPER', 'shadowed_builtin')").fetchone()[0], [7])

    def test_version_counter_permissions_and_direct_dml(self):
        self.db.execute("SELECT create_text_analyzer('versioned', '')")
        with self.connect() as reader, self.connect(user=self.owner) as owner:
            query = "SELECT apply_text_analyzer('UPPER','versioned')"
            self.assertEqual(reader.execute(query).fetchone()[0], ['UPPER'])
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                owner.execute("UPDATE tokenizer_catalog.definition_version SET version = 0")
            self.assertFalse(owner.execute("SELECT has_function_privilege('tokenizer_catalog._bump_definition_version()', 'EXECUTE')").fetchone()[0])
            owner.execute("UPDATE tokenizer_catalog.text_analyzer SET config=%s WHERE name='versioned'",
                          (json.dumps({'character_filters': ['to_lowercase']}),))
            self.assertEqual(reader.execute(query).fetchone()[0], ['upper'])
            owner.execute('BEGIN')
            owner.execute("SAVEPOINT definition_change")
            owner.execute("DELETE FROM tokenizer_catalog.text_analyzer WHERE name='versioned'")
            owner.execute("ROLLBACK TO definition_change")
            owner.execute('COMMIT')
            self.assertEqual(reader.execute(query).fetchone()[0], ['upper'])

    def test_direct_dml_abort_does_not_reuse_a_version(self):
        self.db.execute("SELECT create_text_analyzer('version_reuse', '')")
        query = "SELECT apply_text_analyzer('UPPER', 'version_reuse')"
        with self.connect() as reader:
            self.assertEqual(reader.execute(query).fetchone()[0], ['UPPER'])
            reader.execute('BEGIN')
            reader.execute("UPDATE tokenizer_catalog.text_analyzer SET config=%s WHERE name='version_reuse'",
                           (json.dumps({'character_filters': ['to_lowercase']}),))
            self.assertEqual(reader.execute(query).fetchone()[0], ['upper'])
            aborted_version = reader.execute('SELECT version FROM tokenizer_catalog.definition_version').fetchone()[0]
            reader.execute('ROLLBACK')
            # Reuse the rolled-back counter value with a different definition.
            self.db.execute("UPDATE tokenizer_catalog.text_analyzer SET config=%s WHERE name='version_reuse'",
                            (json.dumps({'token_filters': [{'ngram': {'min_gram': 1, 'max_gram': 1}}]}),))
            self.assertEqual(self.db.execute('SELECT version FROM tokenizer_catalog.definition_version').fetchone()[0], aborted_version)
            self.assertEqual(reader.execute(query).fetchone()[0], list('UPPER'))

    def test_upgrade_refuses_immutable_dependents(self):
        self.db.execute("SELECT create_text_analyzer('indexed_words', 'pre_tokenizer = \"unicode_segmentation\"')")
        self.db.execute("CREATE TABLE indexed_docs (body text)")
        self.db.execute("CREATE INDEX indexed_terms ON indexed_docs USING gin (apply_text_analyzer(body, 'indexed_words'))")
        with self.assertRaises(psycopg.errors.RaiseException):
            self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(
            self.db.execute("SELECT extversion FROM pg_extension WHERE extname = 'pg_tokenizer'").fetchone()[0],
            "0.1.1",
        )
        self.db.execute("DROP INDEX indexed_terms")
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")


    def test_logical_dump_restores_definitions(self):
        if not (shutil.which("pg_dump") and shutil.which("psql")):
            self.skipTest("pg_dump/psql not found")
        client = subprocess.run(["pg_dump", "--version"], check=True, capture_output=True, text=True).stdout
        if int(re.search(r"\) (\d+)", client).group(1)) != self.db.info.server_version // 10000:
            self.skipTest("pg_dump major version differs from the server")
        self.make_model(self.db)
        self.db.execute("INSERT INTO race(body) VALUES ('alpha beta')")
        self.db.execute("SELECT create_stopwords('custom_stop', 'beta')")
        self.db.execute(
            "SELECT create_text_analyzer('stop_words', 'pre_tokenizer = \"unicode_segmentation\"\n[[token_filters]]\nstopwords = \"custom_stop\"')"
        )
        expected = self.db.execute("SELECT tokenize('alpha beta', 'race')").fetchone()[0]
        vocabulary = self.db.execute("SELECT id, token FROM tokenizer_data.model_race ORDER BY id").fetchall()
        restored = self.dbname + "_restored"
        self.admin.execute(sql.SQL("CREATE DATABASE {}").format(sql.Identifier(restored)))
        try:
            dump = subprocess.run(
                ["pg_dump", "--dbname", f"{self.dsn} dbname={self.dbname}"],
                check=True, capture_output=True, text=True,
            ).stdout
            subprocess.run(
                ["psql", "-X", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", f"{self.dsn} dbname={restored}"],
                input=dump, check=True, capture_output=True, text=True,
            )
            with psycopg.connect(self.dsn, dbname=restored, autocommit=True) as conn:
                conn.execute("SET search_path = public, tokenizer_catalog, tokenizer_data")
                self.assertEqual(conn.execute("SELECT tokenize('alpha beta', 'race')").fetchone()[0], expected)
                self.assertEqual(conn.execute("SELECT apply_text_analyzer('alpha beta', 'stop_words')").fetchone()[0], ["alpha"])
                self.assertEqual(conn.execute(
                    "SELECT count(*) FROM tokenizer_catalog.stopwords WHERE name IN ('lucene_english', 'nltk_english', 'iso_english')"
                ).fetchone()[0], 3)
                self.assertEqual(conn.execute(
                    "SELECT id, token FROM tokenizer_data.model_race ORDER BY id"
                ).fetchall(), vocabulary)
                conn.execute("INSERT INTO race(body) VALUES ('gamma alpha')")
                self.assert_embeddings(conn)
                # The identity sequence continues instead of reusing an ID.
                self.assertEqual(conn.execute(
                    "SELECT id FROM tokenizer_data.model_race WHERE token = 'gamma'"
                ).fetchone()[0], max(row[0] for row in vocabulary) + 1)
        finally:
            self.admin.execute(sql.SQL("DROP DATABASE {} WITH (FORCE)").format(sql.Identifier(restored)))

    def analyze(self, text, analyzer):
        return self.db.execute("SELECT apply_text_analyzer(%s, %s)", (text, analyzer)).fetchone()[0]

    def test_ngram_counts_characters(self):
        # Byte slicing failed on multibyte text, and tokens shorter than
        # min_gram underflowed: a panic in debug builds, an endless,
        # uncancellable loop in release builds.
        self.db.execute("""SELECT create_text_analyzer('grams', $$
[[token_filters]]
ngram = { min_gram = 2, max_gram = 3 }
$$)""")
        self.db.execute("""SELECT create_text_analyzer('grams_words', $$
pre_tokenizer = "unicode_segmentation"
[[token_filters]]
ngram = { min_gram = 2, max_gram = 2, preserve_original = true }
$$)""")
        self.assertEqual(self.analyze("中文分词", "grams"), ["中文", "中文分", "文分", "文分词", "分词"])
        self.assertEqual(self.analyze("été", "grams"), ["ét", "été", "té"])
        self.assertEqual(self.analyze("🦀🦀", "grams"), ["🦀🦀"])
        self.assertEqual(self.analyze("a", "grams"), [])
        self.assertEqual(self.analyze("", "grams"), [])
        self.assertEqual(self.analyze("a bcd 中文", "grams_words"), ["a", "bc", "cd", "bcd", "中", "文"])

    def test_pg_dict_matches_ts_lexize(self):
        # ts_lexize() passes a fourth DictSubState argument; thesaurus refuses
        # calls without it. Each pg_dict token must match ts_lexize().
        self.db.execute(
            "CREATE TEXT SEARCH DICTIONARY synonym_test (TEMPLATE = synonym, SYNONYMS = synonym_sample)"
        )
        self.db.execute(
            "CREATE TEXT SEARCH DICTIONARY ispell_test (TEMPLATE = ispell, DictFile = ispell_sample, AffFile = ispell_sample)"
        )
        self.db.execute(
            "CREATE TEXT SEARCH DICTIONARY thesaurus_test (TEMPLATE = thesaurus, DictFile = thesaurus_sample, Dictionary = pg_catalog.english_stem)"
        )
        dictionaries = ["simple", "english_stem", "synonym_test", "ispell_test", "thesaurus_test"]
        if self.db.execute("SELECT count(*) FROM pg_available_extensions WHERE name = 'unaccent'").fetchone()[0]:
            self.db.execute("CREATE EXTENSION unaccent")
            dictionaries.append("unaccent")
        words = ["the", "running", "postgresql", "supernovae", "one", "bookings", "skies", "Crème"]
        for dictionary in dictionaries:
            self.db.execute(
                "SELECT create_text_analyzer(%s, %s)",
                (f"dict_{dictionary}", f'pre_tokenizer = "unicode_segmentation"\n[[token_filters]]\npg_dict = "{dictionary}"'),
            )
            expected = []
            for word in words:
                lexemes = self.db.execute("SELECT ts_lexize(%s::regdictionary, %s)", (dictionary, word)).fetchone()[0]
                expected.extend([word] if lexemes is None else lexemes)
            self.assertEqual(self.analyze(" ".join(words), f"dict_{dictionary}"), expected, dictionary)
        self.assertEqual(self.analyze("supernovae one", "dict_thesaurus_test"), ["sn", "1"])

    def test_builtin_stopwords_are_reserved(self):
        # pg_dump skips the shipped lists; replacing one would revert on restore.
        with self.assertRaisesRegex(psycopg.Error, "Built-in stopwords cannot be dropped"):
            self.db.execute("SELECT drop_stopwords('lucene_english')")
        with self.assertRaisesRegex(psycopg.Error, "reserved for a built-in list"):
            self.db.execute("SELECT create_stopwords('nltk_english', 'x')")
        self.assertEqual(self.db.execute(
            "SELECT count(*) FROM tokenizer_catalog.stopwords WHERE name IN ('lucene_english', 'nltk_english', 'iso_english')"
        ).fetchone()[0], 3)

    def test_upgrade_warns_about_replaced_stopwords(self):
        # Older binaries allowed drop_stopwords() and create_stopwords() to
        # replace a shipped list; the new library refuses, so edit the row.
        self.db.execute("UPDATE tokenizer_catalog.stopwords SET config = 'custom' WHERE name = 'lucene_english'")
        notices = []
        self.db.add_notice_handler(lambda d: notices.append((d.severity_nonlocalized, d.message_primary)))
        self.db.execute("ALTER EXTENSION pg_tokenizer UPDATE TO '1.1.0'")
        self.assertEqual(
            [n for n in notices if n[0] == "WARNING"],
            [("WARNING", "stopwords lucene_english differ from the shipped lists")],
        )
        self.assertEqual(self.db.execute(
            "SELECT config FROM tokenizer_catalog.stopwords WHERE name = 'lucene_english'"
        ).fetchone()[0], "custom")

if __name__ == "__main__":
    unittest.main(verbosity=2)
