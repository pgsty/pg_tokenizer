"""Small paired benchmark on the two explicitly owned phase-3 clusters.

Usage: python benchmark.py /tmp/pg-tokenizer-phase3-20261107
Requires psycopg and before-connection.json / connection.json in that root.
Reported times are elapsed/executor times, not isolated CPU measurements.
"""

import concurrent.futures
import json
from pathlib import Path
import statistics
import sys
import time

import psycopg
from psycopg import sql


root = Path(sys.argv[1]).resolve()
configs = {name: json.loads((root / file).read_text()) for name, file in [
    ('baseline', 'before-connection.json'), ('current', 'connection.json')]}
config = 'pre_tokenizer = "unicode_segmentation"\ncharacter_filters = ["to_lowercase"]'
queries = {
    'analyzer_50000_rows': "SELECT sum(cardinality(apply_text_analyzer('Alpha βeta ' || i || ' gamma', 'bench_words'))) FROM generate_series(1, 50000) i",
    'bert_5000_rows': "SELECT sum(cardinality(tokenize('PostgreSQL search document ' || i, 'bench_bert'))) FROM generate_series(1, 5000) i",
    'ngram_10000_rows': "SELECT sum(cardinality(apply_text_analyzer('abcdefghijklmno' || i, 'bench_grams'))) FROM generate_series(1, 10000) i",
}
database = 'phase3_benchmark'


def connect(name, dbname='postgres'):
    options = dict(configs[name], dbname=dbname)
    conn = psycopg.connect(**options, autocommit=True)
    expected = root / ('before-primary' if name == 'baseline' else 'primary')
    actual = Path(conn.execute('SHOW data_directory').fetchone()[0]).resolve()
    assert actual == expected.resolve(), f'refusing unexpected cluster: {actual}'
    conn.execute('SET search_path = public, tokenizer_catalog')
    conn.execute("SET statement_timeout = '30s'")
    conn.execute('SET max_parallel_workers_per_gather = 0')
    conn.execute('SET jit = off')
    return conn


def execution_ms(conn, query):
    return conn.execute('EXPLAIN (ANALYZE, TIMING OFF, FORMAT JSON) ' + query).fetchone()[0][0]['Execution Time']


def repeated_ms(conn):
    start = time.perf_counter()
    for i in range(3000):
        conn.execute("SELECT cardinality(apply_text_analyzer(%s, 'bench_words'))",
                     (f'Alpha βeta {i} gamma',)).fetchone()
    return (time.perf_counter() - start) * 1000


def wal_bytes(conn, statements):
    start = conn.execute('SELECT pg_current_wal_insert_lsn()').fetchone()[0]
    for query, args in statements:
        conn.execute(query, args).fetchall()
    return int(conn.execute('SELECT pg_wal_lsn_diff(pg_current_wal_insert_lsn(), %s::pg_lsn)', (start,)).fetchone()[0])


def contention(name):
    with connect(name, database) as first, connect(name, database) as second, connect(name, database) as observer:
        first.execute('BEGIN')
        first.execute("SELECT create_text_analyzer('bench_lock_a', '')")

        def write():
            start = time.perf_counter()
            second.execute("SELECT create_text_analyzer('bench_lock_b', '')")
            return (time.perf_counter() - start) * 1000

        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            future = pool.submit(write)
            blocked = False
            deadline = time.monotonic() + 5
            try:
                while not future.done():
                    blocked = observer.execute('SELECT %s = ANY(pg_blocking_pids(%s))',
                                               (first.info.backend_pid, second.info.backend_pid)).fetchone()[0]
                    if blocked:
                        # Deliberate hold after observing the dependency.
                        time.sleep(0.3)
                        break
                    if time.monotonic() > deadline:
                        raise RuntimeError('writer neither completed nor reached lock barrier')
                    time.sleep(0.005)
            finally:
                first.execute('COMMIT')
            return {'blocked_on_first_writer': blocked, 'elapsed_ms': future.result(timeout=10)}


connections = {}
results = {name: {} for name in configs}
try:
    for name in configs:
        with connect(name) as admin:
            admin.execute(sql.SQL('CREATE DATABASE {}').format(sql.Identifier(database)))
        conn = connections[name] = connect(name, database)
        conn.execute('CREATE EXTENSION pg_tokenizer')
        conn.execute("SELECT create_text_analyzer('bench_words', %s)", (config,))
        conn.execute("SELECT create_tokenizer('bench_bert', 'model = \"bert_base_uncased\"')")
        conn.execute("SELECT create_text_analyzer('bench_grams', '[[token_filters]]\nngram = {min_gram = 1, max_gram = 3}')")
        results[name]['extversion'] = conn.execute("SELECT extversion FROM pg_extension WHERE extname='pg_tokenizer'").fetchone()[0]
        for query in queries.values():
            for _ in range(3):
                execution_ms(conn, query)

    # Alternate the order to reduce bias from machine load and thermal drift.
    for workload, query in queries.items():
        samples = {name: [] for name in configs}
        for iteration in range(7):
            for name in (list(configs) if iteration % 2 == 0 else list(reversed(configs))):
                samples[name].append(execution_ms(connections[name], query))
        for name, values in samples.items():
            results[name][workload] = {'samples_ms': values, 'median_ms': statistics.median(values)}
    for name, conn in connections.items():
        values = [repeated_ms(conn) for _ in range(3)]
        results[name]['individual_3000_calls'] = {'samples_ms': values, 'median_ms': statistics.median(values)}
        results[name]['read_wal_bytes'] = wal_bytes(conn, [
            ("SELECT cardinality(apply_text_analyzer(%s, 'bench_words'))", (f'read {i}',)) for i in range(300)])
        results[name]['definition_wal_bytes'] = wal_bytes(conn, [
            ("SELECT create_text_analyzer('bench_write', %s)", (config,))])
        results[name]['definition_contention'] = contention(name)
finally:
    for name, conn in connections.items():
        conn.close()
        with connect(name) as admin:
            admin.execute(sql.SQL('DROP DATABASE {}').format(sql.Identifier(database)))

(root / 'logs/benchmark-delivery.json').write_text(json.dumps(results, indent=2) + '\n')
print(json.dumps(results, indent=2))
