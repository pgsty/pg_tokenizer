"""Stage a freshly built candidate and validate it in disposable PG18 clusters."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('root', type=Path)
parser.add_argument('repo', type=Path)
parser.add_argument('peers', type=Path)
args = parser.parse_args()
root, repo, peers = (p.resolve() for p in (args.root, args.repo, args.peers))
logs = root / 'logs'
logs.mkdir(exist_ok=True)
lib, share = root / 'current/lib', root / 'current/share/extension'
lib.mkdir(parents=True, exist_ok=True)
share.mkdir(parents=True, exist_ok=True)
pg = Path(subprocess.check_output(['pg_config', '--bindir'], text=True).strip())
manifest = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
            'source_status': subprocess.check_output(['git', 'status', '--porcelain'], cwd=repo, text=True),
            'checks': {}}

def run(name, argv, env=None, timeout=240):
    start = time.monotonic()
    with (logs / (name + '.log')).open('w') as out:
        result = subprocess.run([str(x) for x in argv], cwd=repo, env=env,
                                stdout=out, stderr=subprocess.STDOUT, timeout=timeout)
    manifest['checks'][name] = {'returncode': result.returncode, 'seconds': round(time.monotonic() - start, 3)}
    (logs / 'acceptance.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'{name}: exit {result.returncode}', flush=True)
    result.check_returncode()

for name in ('vchord', 'vchord_bm25'):
    shutil.copy2(peers / 'current/lib' / (name + '.dylib'), lib)
    for file in (peers / 'current/share/extension').glob(name + '*'):
        shutil.copy2(file, share)
    control = share / (name + '.control')
    control.write_text(re.sub(r"^module_pathname\s*=.*$", f"module_pathname = '{lib / (name + '.dylib')}'", control.read_text(), flags=re.M))
shutil.copy2(root / 'target/release/libpg_tokenizer.dylib', lib / 'pg_tokenizer.dylib')
for folder in ('install', 'upgrade'):
    for file in (repo / 'sql' / folder).glob('*.sql'):
        shutil.copy2(file, share)
(share / 'pg_tokenizer.control').write_text((repo / 'pg_tokenizer.control').read_text().replace('@CARGO_VERSION@', '1.0.1').replace('$libdir/pg_tokenizer', str(lib / 'pg_tokenizer.dylib')))
# pgrx may emit its five independent catalog-table declarations in any order.
# Compare their full blocks and require every remaining byte (except terminal
# whitespace) to match, so reordering dependent SQL cannot pass this check.
pattern = re.compile(r'/\* <begin connected objects> \*/\n-- [^\n]+\n\nCREATE TABLE tokenizer_catalog\.(?:text_analyzer|tokenizer|model|stopwords|synonym) \(\n.*?\n\);\n/\* </end connected objects> \*/\n\n', re.S)
generated = (root / 'schema.sql').read_text()
checked = (repo / 'sql/install/pg_tokenizer--1.0.1.sql').read_text()
assert len(pattern.findall(generated)) == len(pattern.findall(checked)) == 5
assert sorted(pattern.findall(generated)) == sorted(pattern.findall(checked))
assert pattern.sub('', generated).rstrip() == pattern.sub('', checked).rstrip()
shutil.copy2(root / 'schema.sql', share / 'pg_tokenizer--1.0.1.sql')
manifest['staged_controls'] = {p.name: p.read_text() for p in share.glob('*.control')}
manifest['generated_sql_matches'] = 'exact except independent catalog-table declaration order and terminal whitespace'
manifest['sha256'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in lib.glob('*.dylib')}
manifest['sql_sha256'] = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in share.glob('pg_tokenizer*')}
env = dict(os.environ, PG_TOKENIZER_TEST_LIBRARY=str(lib / 'pg_tokenizer.dylib'),
           PG_TOKENIZER_TEST_SHAREDIR=str(share.parent))
with tempfile.TemporaryDirectory(prefix='accept-', dir=root) as directory:
    temp = Path(directory)
    data, sockets = temp / 'data', temp / 's'
    sockets.mkdir()
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    run('initdb', [pg / 'initdb', '-D', data, '-U', 'postgres', '-A', 'trust', '--no-locale', '-E', 'UTF8'])
    with (data / 'postgresql.conf').open('a') as config:
        config.write(f"\nlisten_addresses = ''\nport = {port}\nunix_socket_directories = '{sockets}'\n"
                     f"extension_control_path = '{share.parent}:$system'\ndynamic_library_path = '{lib}:$libdir'\n"
                     f"shared_preload_libraries = '{lib / 'pg_tokenizer.dylib'}'\nmax_prepared_transactions = 10\n")
    started = False
    try:
        run('start', [pg / 'pg_ctl', '-D', data, '-w', '-l', logs / 'server.log', 'start'])
        started = True
        dsn = f'host={sockets} port={port} dbname=postgres user=postgres'
        env.update(PG_TOKENIZER_TEST_DSN=dsn, PGHOST=str(sockets), PGPORT=str(port), PGUSER='postgres', PGDATABASE='postgres')
        run('integration', [sys.executable, repo / 'tests/test_integration.py'], env)
        run('slt-setup', [pg / 'psql', '-X', '-h', sockets, '-p', port, '-U', 'postgres', '-d', 'postgres',
                          '-v', 'ON_ERROR_STOP=1', '-c', "CREATE EXTENSION pg_tokenizer; SELECT extversion FROM pg_extension WHERE extname='pg_tokenizer';"], env)
        run('slt', ['sqllogictest', '--host', sockets, '--port', port, '--db', 'postgres', '--user', 'postgres',
                    './tests/sqllogictest/*.slt'], env)
    finally:
        if started:
            run('stop', [pg / 'pg_ctl', '-D', data, '-w', 'stop', '-m', 'fast'])
run('standby', [sys.executable, repo / 'tests/test_standby.py'], env)
run('preload', [sys.executable, repo / 'tests/test_preload.py'], env)
run('combo-orders', [sys.executable, repo / 'reports/phase3-1.0.1/scripts/joint_acceptance.py', root], env)
manifest['clusters_stopped_and_removed'] = True
(logs / 'acceptance.json').write_text(json.dumps(manifest, indent=2) + '\n')
