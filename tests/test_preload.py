"""Preload startup, file publication and database isolation in private clusters.

Uses pg_config from PATH. Optional PG_TOKENIZER_TEST_LIBRARY and
PG_TOKENIZER_TEST_SHAREDIR select a staged PG18+ package without installing it.
"""

from concurrent.futures import ThreadPoolExecutor
import errno
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import unittest

import psycopg


class PreloadTests(unittest.TestCase):
    def setUp(self):
        bindir = Path(subprocess.check_output(["pg_config", "--bindir"], text=True).strip())
        self.pg_ctl = bindir / "pg_ctl"
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory(prefix="tokenizer-preload-")))
        self.data = self.root / "data"
        self.preload = self.data / "pg_tokenizer/preload_models"
        self.log = self.root / "server.log"
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            self.port = sock.getsockname()[1]
        subprocess.run([
            str(bindir / "initdb"), "-D", str(self.data), "--no-locale", "-E", "UTF8",
            "-A", "trust", "-U", "postgres",
        ], check=True, capture_output=True)
        library = os.environ.get("PG_TOKENIZER_TEST_LIBRARY", "pg_tokenizer")
        with (self.data / "postgresql.conf").open("a") as file:
            file.write(f"\nlisten_addresses = ''\nport = {self.port}\nunix_socket_directories = '{self.root}'\n")
            file.write(f"shared_preload_libraries = '{library}'\n")
            if share := os.environ.get("PG_TOKENIZER_TEST_SHAREDIR"):
                file.write(f"extension_control_path = '{share}:$system'\n")
            if "PG_TOKENIZER_TEST_LIBRARY" in os.environ:
                file.write(f"dynamic_library_path = '{Path(library).parent}:$libdir'\n")
        self.addCleanup(self.stop)

    def ctl(self, action):
        result = subprocess.run([
            str(self.pg_ctl), "-D", str(self.data), "-l", str(self.log),
            "-w", "-t", "30", action,
        ], capture_output=True, text=True)
        if result.returncode:
            self.fail(f"pg_ctl {action}: {result.stderr}\n{self.log.read_text(errors='replace')[-3000:]}")

    def stop(self):
        if (self.data / "postmaster.pid").exists():
            self.ctl("stop")

    def connect(self, database="postgres", user="postgres"):
        return psycopg.connect(host=str(self.root), port=self.port, user=user, dbname=database, autocommit=True)

    def create_model(self, conn, name, number, padding=0):
        config = json.dumps({
            "model": {"type": "WordLevel", "vocab": {"[UNK]": 0, "hello": number}, "unk_token": "[UNK]"}
        }) + " " * padding
        conn.execute("SELECT tokenizer_catalog.create_huggingface_model(%s, %s)", (name, config))
        conn.execute("SELECT tokenizer_catalog.create_tokenizer(%s, %s)", (name, f'model = "{name}"'))
        return conn.execute("SELECT config FROM tokenizer_catalog.model WHERE name = %s", (name,)).fetchone()[0]

    def listed(self, conn):
        return conn.execute("SELECT tokenizer_catalog.list_preload_models()").fetchone()[0]

    def test_startup_recovery_and_catalog_identity(self):
        self.preload.mkdir(parents=True)
        (self.preload / "bad-name").touch()
        (self.preload / ".preload-abandoned").write_text('{"custom":')
        (self.preload / "é").touch()
        try:
            with open(os.fsencode(self.preload) + b"/\xff", "xb"):
                pass
        except OSError as error:
            if error.errno not in (errno.EILSEQ, errno.EINVAL):
                raise
        (self.preload / "bad_utf8").write_bytes(b"\xff")
        (self.preload / "broken_json").write_text('{"hugging_face":')
        (self.preload / "llmlingua2").write_text("invalid built-in marker")
        old_dictionary = Path(__file__).resolve().parent / "fixtures/lindera-v2"
        old_config = json.dumps({"lindera": {"segmenter": {"dictionary": {"path": str(old_dictionary)}}}})
        (self.preload / "legacy_dictionary").write_text(old_config)
        (self.preload / "unreadable_entry").mkdir()
        (self.preload / "linked_model").symlink_to(self.preload / "broken_json")
        os.mkfifo(self.preload / "named_pipe")
        with (self.preload / "too_large").open("wb") as file:
            file.truncate(1 << 30)
        user_dictionary = self.root / "user_dictionary.csv"
        user_dictionary.write_text("東京スカイツリー,カスタム名詞,トウキョウスカイツリー\n")
        lindera = (
            "[segmenter]\ndictionary = "
            + json.dumps(os.environ.get("PG_TOKENIZER_TEST_LINDERA_DICTIONARY", "embedded://ipadic"))
            + "\nuser_dictionary = " + json.dumps(str(user_dictionary))
        )
        self.ctl("start")
        log = self.log.read_text()
        self.assertNotIn(".preload-abandoned", log)
        for name in ("bad-name", "bad_utf8", "broken_json", "legacy_dictionary", "linked_model",
                     "llmlingua2", "named_pipe", "too_large", "unreadable_entry"):
            self.assertIn(f'skipping preload model "{name}"', log)
        with self.connect() as conn:
            conn.execute("CREATE EXTENSION pg_tokenizer")
            self.assertEqual(self.listed(conn), ["bad_utf8", "broken_json", "legacy_dictionary", "llmlingua2", "too_large"])
            conn.execute("INSERT INTO tokenizer_catalog.model(name, config) VALUES ('legacy_dictionary', %s)", (old_config,))
            with self.assertRaises(psycopg.errors.InvalidParameterValue) as refused:
                conn.execute("SELECT tokenizer_catalog.create_tokenizer('legacy_dictionary', 'model=\"legacy_dictionary\"')")
            self.assertRegex(refused.exception.diag.message_detail, r"format version 2.*format version 3")
            self.create_model(conn, "shared_model", 11)
            conn.execute("SELECT tokenizer_catalog.add_preload_model('shared_model')")
            conn.execute("CREATE DATABASE other_database")
            conn.execute("CREATE ROLE lindera_owner LOGIN")
            conn.execute("CREATE DATABASE lindera_database OWNER lindera_owner")
        with self.connect("lindera_database") as conn:
            conn.execute("CREATE EXTENSION pg_tokenizer")
            conn.execute("SELECT tokenizer_catalog.create_lindera_model('user_lindera', %s)", (lindera,))
            conn.execute("SELECT tokenizer_catalog.create_tokenizer('user_lindera', 'model = \"user_lindera\"')")
            stored = conn.execute(
                "UPDATE tokenizer_catalog.model SET config = '  ' || config WHERE name = 'user_lindera' RETURNING config"
            ).fetchone()[0]
            conn.execute("SELECT tokenizer_catalog.add_preload_model('user_lindera')")
            self.assertEqual((self.preload / "user_lindera").read_text(), stored)
        self.ctl("restart")
        with self.connect("lindera_database", "lindera_owner") as conn:
            self.assertIn("user_lindera", self.listed(conn))
            self.assertEqual(conn.execute(
                "SELECT tokenizer_catalog.tokenize('東京スカイツリー', 'user_lindera')"
            ).fetchone()[0], [1 << 30])
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                conn.execute("SELECT tokenizer_catalog.create_lindera_model('owner_lindera', %s)", (lindera,))
            for function in ("add_preload_model", "remove_preload_model"):
                with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                    conn.execute(f"SELECT tokenizer_catalog.{function}('user_lindera')")
        with self.connect("lindera_database") as conn:
            conn.execute("GRANT EXECUTE ON FUNCTION tokenizer_catalog.remove_preload_model(text) TO lindera_owner")
        with self.connect("lindera_database", "lindera_owner") as conn:
            conn.execute("SELECT tokenizer_catalog.remove_preload_model('user_lindera')")
            self.assertNotIn("user_lindera", self.listed(conn))
            with self.assertRaises(psycopg.errors.InsufficientPrivilege):
                conn.execute("SELECT tokenizer_catalog.add_preload_model('user_lindera')")
        with self.connect("lindera_database") as conn:
            conn.execute("GRANT EXECUTE ON FUNCTION tokenizer_catalog.add_preload_model(text) TO lindera_owner")
        with self.connect("lindera_database", "lindera_owner") as conn:
            conn.execute("SELECT tokenizer_catalog.add_preload_model('user_lindera')")
            self.assertIn("user_lindera", self.listed(conn))
        with self.connect("other_database") as conn:
            conn.execute("CREATE EXTENSION pg_tokenizer")
            self.create_model(conn, "shared_model", 22)
            self.assertEqual(conn.execute(
                "SELECT tokenizer_catalog.tokenize('hello', 'shared_model')"
            ).fetchone()[0], [22])
        for _ in range(2):
            with self.connect() as conn:
                self.assertEqual(conn.execute(
                    "SELECT tokenizer_catalog.tokenize('hello', 'shared_model')"
                ).fetchone()[0], [11])
        with self.connect() as conn:
            conn.execute("SELECT tokenizer_catalog.drop_huggingface_model('shared_model')")
            with self.assertRaises(psycopg.Error):
                conn.execute("SELECT tokenizer_catalog.tokenize('hello', 'shared_model')")

    def test_concurrent_publication(self):
        name = "RaceModel_0123456789"
        path = self.preload / name
        self.preload.mkdir(parents=True)
        self.ctl("start")
        add = "SELECT tokenizer_catalog.add_preload_model(%s)"
        remove = "SELECT tokenizer_catalog.remove_preload_model(%s)"
        with self.connect() as conn:
            conn.execute("CREATE DATABASE other_database")
        definitions = []
        for number, database in enumerate(("postgres", "other_database"), 1):
            with self.connect(database) as conn:
                conn.execute("CREATE EXTENSION pg_tokenizer")
                definitions.append(self.create_model(conn, name, number, number * 65536))
        with self.connect() as first, self.connect("other_database") as second:
            first.execute(add, (name,))
            with path.open() as old:
                second.execute(add, (name,))
                self.assertEqual(old.read(), definitions[0])
                self.assertEqual(path.read_text(), definitions[1])
        barrier = threading.Barrier(4, timeout=15)

        def write(database, statement):
            with self.connect(database) as conn:
                barrier.wait()
                for _ in range(25):
                    conn.execute(statement, (name,))

        def observe():
            with self.connect() as conn:
                barrier.wait()
                for _ in range(80):
                    self.assertIn(self.listed(conn), ([], [name]))
                    try:
                        content = path.read_text()
                    except FileNotFoundError:
                        continue
                    self.assertIn(content, definitions)

        with ThreadPoolExecutor(max_workers=4) as pool:
            futures = [pool.submit(write, "postgres", add), pool.submit(write, "other_database", add),
                       pool.submit(write, "postgres", remove), pool.submit(observe)]
            for future in futures:
                future.result(timeout=30)
        with self.connect() as conn:
            conn.execute(add, (name,))
            self.assertEqual(self.listed(conn), [name])
            self.assertEqual(path.read_text(), definitions[0])
            self.assertEqual(list(self.preload.iterdir()), [path])

    def test_directory_defaults_and_paths(self):
        self.ctl("start")
        with self.connect() as conn:
            conn.execute("CREATE EXTENSION pg_tokenizer")
            self.assertEqual(self.listed(conn), ["llmlingua2"])
            conn.execute("SELECT tokenizer_catalog.add_preload_model('llmlingua2')")
            self.assertEqual((self.preload / "llmlingua2").read_bytes(), b"")
            conn.execute("BEGIN")
            conn.execute("SELECT tokenizer_catalog.remove_preload_model('llmlingua2')")
            conn.execute("ROLLBACK")
        self.ctl("restart")
        with self.connect() as conn:
            self.assertEqual(self.listed(conn), [])
            self.preload.rmdir()
            self.assertEqual(self.listed(conn), [])
            conn.execute("SELECT tokenizer_catalog.remove_preload_model('llmlingua2')")
            conn.execute("SELECT tokenizer_catalog.add_preload_model('bert_base_uncased')")
            marker = self.preload / "bert_base_uncased"
            self.assertEqual(marker.read_bytes(), b"")
            marker.unlink()
            outside = self.root / "outside"
            outside.mkdir()
            sentinel = outside / "bert_base_uncased"
            sentinel.write_text("unchanged")
            marker.symlink_to(sentinel)
            self.assertEqual(self.listed(conn), [])
            conn.execute("SELECT tokenizer_catalog.add_preload_model('bert_base_uncased')")
            self.assertFalse(marker.is_symlink())
            self.assertEqual(marker.read_bytes(), b"")
            self.assertEqual(sentinel.read_text(), "unchanged")
            marker.unlink()
            marker.mkdir()
            with self.assertRaisesRegex(psycopg.Error, "Failed to save preload model"):
                conn.execute("SELECT tokenizer_catalog.add_preload_model('bert_base_uncased')")
            self.assertTrue(marker.is_dir())
            self.assertEqual(list(self.preload.iterdir()), [marker])
        self.stop()
        for component in (self.preload, self.preload.parent):
            with self.subTest(component=component.name):
                saved = component.with_suffix(".saved")
                component.rename(saved)
                component.symlink_to(outside, target_is_directory=True)
                self.ctl("start")
                with self.connect() as conn:
                    self.assertEqual(conn.execute("SELECT 1").fetchone()[0], 1)
                    for call in ("list_preload_models()", "add_preload_model('bert_base_uncased')",
                                 "remove_preload_model('bert_base_uncased')"):
                        with self.assertRaises(psycopg.Error):
                            conn.execute("SELECT tokenizer_catalog." + call)
                self.assertEqual(list(outside.iterdir()), [sentinel])
                self.assertEqual(sentinel.read_text(), "unchanged")
                self.stop()
                component.unlink()
                saved.rename(component)


if __name__ == "__main__":
    unittest.main(verbosity=2)
