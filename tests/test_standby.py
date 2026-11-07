"""Physical replay, old snapshots and promotion in private PostgreSQL clusters.

PG_TOKENIZER_TEST_LIBRARY and PG_TOKENIZER_TEST_SHAREDIR select a staged build
on PG18+, as in test_preload.py. Never installs or restarts a shared server.
"""

import os
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest

import psycopg


class StandbyTests(unittest.TestCase):
    def test_replay_snapshots_and_promotion(self):
        pg = Path(subprocess.check_output(["pg_config", "--bindir"], text=True).strip())
        with tempfile.TemporaryDirectory(prefix="tokenizer-standby-") as temporary:
            root = Path(temporary)
            primary, standby = root / "primary", root / "standby"
            sockets = [root / "p", root / "s"]
            for path in sockets:
                path.mkdir()
            ports = []
            for _ in sockets:
                with socket.socket() as sock:
                    sock.bind(("127.0.0.1", 0))
                    ports.append(sock.getsockname()[1])

            def run(*args):
                return subprocess.run([str(x) for x in args], check=True, capture_output=True, text=True)

            def ctl(data, action, *options):
                return run(pg / "pg_ctl", "-D", data, "-l", root / (data.name + ".log"),
                           "-w", "-t", "30", action, *options)

            def connect(index):
                return psycopg.connect(host=str(sockets[index]), port=ports[index],
                                       user="postgres", dbname="postgres", autocommit=True)

            def wait_for(predicate):
                deadline = time.monotonic() + 30
                while not predicate():
                    if time.monotonic() > deadline:
                        self.fail("replication did not reach the requested state")
                    time.sleep(0.02)

            def analyze(conn):
                return conn.execute("SELECT tokenizer_catalog.apply_text_analyzer('UPPER', 'replayed')").fetchone()[0]

            def redefine(conn, lower):
                with conn.transaction():
                    conn.execute("SELECT tokenizer_catalog.drop_text_analyzer('replayed')")
                    conn.execute("SELECT tokenizer_catalog.create_text_analyzer('replayed', %s)",
                                 ('character_filters = ["to_lowercase"]' if lower else '',))

            def model(conn, number):
                config = json.dumps({'model': {'type': 'WordLevel',
                    'vocab': {'[UNK]': 0, 'hello': number}, 'unk_token': '[UNK]'}})
                conn.execute("SELECT tokenizer_catalog.create_huggingface_model('replayed_model', %s)", (config,))

            def dependencies(conn):
                return conn.execute("""SELECT
                    tokenizer_catalog.tokenize('hello', 'model_reader'),
                    tokenizer_catalog.tokenize('hello', 'replaced_tokenizer'),
                    tokenizer_catalog.apply_text_analyzer('alpha beta', 'stop_reader'),
                    tokenizer_catalog.apply_text_analyzer('alpha', 'synonym_reader')""").fetchone()

            run(pg / "initdb", "-D", primary, "-U", "postgres", "-A", "trust", "--no-locale", "-E", "UTF8")
            config = (f"\nlisten_addresses = ''\nport = {ports[0]}\n"
                      f"unix_socket_directories = '{sockets[0]}'\nwal_level = replica\n"
                      "max_wal_senders = 5\nhot_standby_feedback = on\n"
                      "max_standby_streaming_delay = -1\n"
                      f"shared_preload_libraries = '{os.environ.get('PG_TOKENIZER_TEST_LIBRARY', 'pg_tokenizer')}'\n")
            if share := os.environ.get("PG_TOKENIZER_TEST_SHAREDIR"):
                config += f"extension_control_path = '{share}:$system'\n"
            if library := os.environ.get("PG_TOKENIZER_TEST_LIBRARY"):
                config += f"dynamic_library_path = '{Path(library).parent}:$libdir'\n"
            with (primary / "postgresql.conf").open("a") as config_file:
                config_file.write(config)
            running = []
            try:
                ctl(primary, "start")
                running.append(primary)
                with connect(0) as writer:
                    writer.execute("CREATE EXTENSION pg_tokenizer VERSION '1.0.0'")
                    writer.execute("SELECT tokenizer_catalog.create_text_analyzer('replayed', '')")
                    writer.execute("SELECT tokenizer_catalog.create_text_analyzer('vocab_analyzer', '')")
                    writer.execute("CREATE TABLE public.replayed_corpus (body text, embedding int[])")
                    writer.execute("SELECT tokenizer_catalog.create_custom_model_tokenizer_and_trigger('replayed_custom', 'replayed_custom', 'vocab_analyzer', 'public.replayed_corpus', 'body', 'embedding')")
                    writer.execute("INSERT INTO public.replayed_corpus(body) VALUES ('alpha')")
                    vocabulary_oid = writer.execute("SELECT 'tokenizer_catalog.model_replayed_custom'::regclass::oid").fetchone()[0]
                    model(writer, 11)
                    for name in ('model_reader', 'replaced_tokenizer'):
                        writer.execute("SELECT tokenizer_catalog.create_tokenizer(%s, 'model = \"replayed_model\"')", (name,))
                    writer.execute("SELECT tokenizer_catalog.create_stopwords('replayed_stop', 'beta')")
                    writer.execute("SELECT tokenizer_catalog.create_synonym('replayed_syn', 'first alpha')")
                    writer.execute("SELECT tokenizer_catalog.create_text_analyzer('stop_reader', 'pre_tokenizer = \"unicode_segmentation\"\n[[token_filters]]\nstopwords = \"replayed_stop\"')")
                    writer.execute("SELECT tokenizer_catalog.create_text_analyzer('synonym_reader', '[[token_filters]]\nsynonym = \"replayed_syn\"')")
                    run(pg / "pg_basebackup", "-D", standby, "-h", sockets[0], "-p", ports[0],
                        "-U", "postgres", "-X", "stream", "-R", "--checkpoint=fast")
                    with (standby / "postgresql.conf").open("a") as config_file:
                        config_file.write(f"\nport = {ports[1]}\nunix_socket_directories = '{sockets[1]}'\n")
                    ctl(standby, "start")
                    running.append(standby)
                    with connect(1) as reader, connect(1) as old, connect(1) as monitor:
                        self.assertEqual(analyze(reader), ["UPPER"])
                        initial = ([11], [11], ['alpha'], ['first'])
                        changed = ([22], [7592], ['beta'], ['second'])
                        self.assertEqual(dependencies(reader), initial)
                        self.assertEqual(reader.execute("SELECT tokenizer_catalog.tokenize('alpha', 'replayed_custom')").fetchone()[0], [1])
                        # Replay the schema migration in a session that has
                        # already loaded the old custom-model configuration.
                        writer.execute("ALTER EXTENSION pg_tokenizer UPDATE")
                        upgrade_lsn = writer.execute("SELECT pg_current_wal_lsn()").fetchone()[0]
                        wait_for(lambda: monitor.execute("SELECT pg_last_wal_replay_lsn() >= %s::pg_lsn", (upgrade_lsn,)).fetchone()[0])
                        self.assertEqual(reader.execute("SELECT 'tokenizer_data.model_replayed_custom'::regclass::oid").fetchone()[0], vocabulary_oid)
                        self.assertIsNone(reader.execute("SELECT to_regclass('tokenizer_catalog.model_replayed_custom')").fetchone()[0])
                        self.assertEqual(reader.execute("SELECT tokenizer_catalog.tokenize('alpha', 'replayed_custom')").fetchone()[0], [1])
                        old.execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
                        old.execute("SELECT count(*) FROM tokenizer_catalog.text_analyzer")
                        monitor.execute("SELECT pg_wal_replay_pause()")
                        wait_for(lambda: monitor.execute("SELECT pg_is_wal_replay_paused()").fetchone()[0])
                        redefine(writer, True)
                        with writer.transaction():
                            writer.execute("SELECT tokenizer_catalog.drop_huggingface_model('replayed_model')")
                            model(writer, 22)
                            writer.execute("SELECT tokenizer_catalog.drop_tokenizer('replaced_tokenizer')")
                            writer.execute("SELECT tokenizer_catalog.create_tokenizer('replaced_tokenizer', 'model = \"bert_base_uncased\"')")
                            writer.execute("SELECT tokenizer_catalog.drop_stopwords('replayed_stop')")
                            writer.execute("SELECT tokenizer_catalog.create_stopwords('replayed_stop', 'alpha')")
                            writer.execute("SELECT tokenizer_catalog.drop_synonym('replayed_syn')")
                            writer.execute("SELECT tokenizer_catalog.create_synonym('replayed_syn', 'second alpha')")
                        lsn = writer.execute("SELECT pg_current_wal_lsn()").fetchone()[0]
                        self.assertEqual(analyze(reader), ["UPPER"])
                        self.assertEqual(dependencies(reader), initial)
                        monitor.execute("SELECT pg_wal_replay_resume()")
                        wait_for(lambda: monitor.execute("SELECT pg_last_wal_replay_lsn() >= %s::pg_lsn", (lsn,)).fetchone()[0])
                        self.assertEqual(analyze(reader), ["upper"])
                        self.assertEqual(dependencies(reader), changed)
                        self.assertEqual(analyze(old), ["UPPER"])
                        self.assertEqual(dependencies(old), initial)
                        old.execute("COMMIT")
                        self.assertEqual(analyze(old), ["upper"])
                        self.assertEqual(dependencies(old), changed)

                        # A rollback must not publish a changed definition.
                        writer.execute("BEGIN")
                        redefine(writer, False)
                        writer.execute("ROLLBACK")
                        self.assertEqual(analyze(reader), ["upper"])

                        old.execute("BEGIN ISOLATION LEVEL REPEATABLE READ")
                        self.assertEqual(analyze(old), ["upper"])
                        redefine(writer, False)
                        lsn = writer.execute("SELECT pg_current_wal_lsn()").fetchone()[0]
                        wait_for(lambda: monitor.execute("SELECT pg_last_wal_replay_lsn() >= %s::pg_lsn", (lsn,)).fetchone()[0])
                        ctl(standby, "promote")
                        wait_for(lambda: not monitor.execute("SELECT pg_is_in_recovery()").fetchone()[0])
                        self.assertEqual(analyze(reader), ["UPPER"])
                        self.assertEqual(analyze(old), ["upper"])
                        old.execute("COMMIT")
                        self.assertEqual(analyze(old), ["UPPER"])
                        redefine(monitor, True)
                        self.assertEqual(analyze(reader), ["upper"])
                        monitor.execute("INSERT INTO public.replayed_corpus(body) VALUES ('beta')")
                        self.assertEqual(reader.execute("SELECT embedding = tokenizer_catalog.tokenize(body, 'replayed_custom') FROM public.replayed_corpus ORDER BY body").fetchall(), [(True,), (True,)])

                    # Crash recovery must retain the WAL-backed signal and definitions.
                    ctl(standby, "stop", "-m", "immediate")
                    running.remove(standby)
                    ctl(standby, "start")
                    running.append(standby)
                    with connect(1) as recovered:
                        self.assertEqual(analyze(recovered), ["upper"])
                        self.assertEqual(recovered.execute("SELECT embedding = tokenizer_catalog.tokenize(body, 'replayed_custom') FROM public.replayed_corpus ORDER BY body").fetchall(), [(True,), (True,)])
            finally:
                for data in reversed(running):
                    ctl(data, "stop", "-m", "fast")


if __name__ == "__main__":
    unittest.main(verbosity=2)
