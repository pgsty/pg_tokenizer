"""Startup recovery and database isolation tests in a private, disposable cluster.

Uses pg_config from PATH. Optional PG_TOKENIZER_TEST_LIBRARY and
PG_TOKENIZER_TEST_SHAREDIR select a staged PG18+ package without installing it.
"""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import psycopg


class PreloadTests(unittest.TestCase):
    def test_startup_recovery_and_catalog_identity(self):
        bindir = Path(subprocess.check_output(["pg_config", "--bindir"], text=True).strip())
        with tempfile.TemporaryDirectory(prefix="tokenizer-preload-") as temporary:
            root = Path(temporary)
            data = root / "data"
            subprocess.run([
                str(bindir / "initdb"), "-D", str(data), "--no-locale", "-E", "UTF8",
                "-A", "trust", "-U", "postgres",
            ], check=True, capture_output=True)
            library = os.environ.get("PG_TOKENIZER_TEST_LIBRARY", "pg_tokenizer")
            config = (
                f"\nlisten_addresses = ''\nunix_socket_directories = '{root}'\n"
                f"shared_preload_libraries = '{library}'\n"
            )
            if share := os.environ.get("PG_TOKENIZER_TEST_SHAREDIR"):
                config += f"extension_control_path = '{share}:$system'\n"
            with (data / "postgresql.conf").open("a") as file:
                file.write(config)
            preload = data / "pg_tokenizer/preload_models"
            preload.mkdir(parents=True)
            (preload / "bad-name").write_text("")
            (preload / "broken_json").write_text("not JSON")
            (preload / "legacy_dictionary").write_text(json.dumps({
                "lindera": {"segmenter": {"dictionary": {"path": str(root / "missing-dictionary")}}}
            }))
            (preload / "unreadable_entry").mkdir()
            log = data / "server.log"

            def ctl(action):
                subprocess.run([
                    str(bindir / "pg_ctl"), "-D", str(data), "-l", str(log),
                    "-w", "-t", "30", action,
                ], check=True, capture_output=True)

            def connect(database="postgres"):
                return psycopg.connect(host=str(root), user="postgres", dbname=database, autocommit=True)

            def create_model(conn, number):
                config = json.dumps({
                    "model": {"type": "WordLevel", "vocab": {"[UNK]": 0, "hello": number}, "unk_token": "[UNK]"}
                })
                conn.execute("SELECT tokenizer_catalog.create_huggingface_model('shared_model', %s)", (config,))
                conn.execute("SELECT tokenizer_catalog.create_tokenizer('shared_tokenizer', 'model = \"shared_model\"')")

            ctl("start")
            try:
                self.assertIn("skipping preload model", log.read_text())
                with connect() as conn:
                    conn.execute("CREATE EXTENSION pg_tokenizer")
                    create_model(conn, 11)
                    conn.execute("SELECT tokenizer_catalog.add_preload_model('shared_model')")
                    conn.execute("CREATE DATABASE other_database")
                ctl("restart")
                with connect("other_database") as conn:
                    conn.execute("CREATE EXTENSION pg_tokenizer")
                    create_model(conn, 22)
                    self.assertEqual(conn.execute(
                        "SELECT tokenizer_catalog.tokenize('hello', 'shared_tokenizer')"
                    ).fetchone()[0], [22])
                # New backends see a nonzero epoch after the other database's DDL.
                for _ in range(2):
                    with connect() as conn:
                        self.assertEqual(conn.execute(
                            "SELECT tokenizer_catalog.tokenize('hello', 'shared_tokenizer')"
                        ).fetchone()[0], [11])
                with connect() as conn:
                    conn.execute("SELECT tokenizer_catalog.drop_huggingface_model('shared_model')")
                    with self.assertRaises(psycopg.Error):
                        conn.execute("SELECT tokenizer_catalog.tokenize('hello', 'shared_tokenizer')")
            finally:
                ctl("stop")


if __name__ == "__main__":
    unittest.main(verbosity=2)
