"""Run frozen libraries in six preload orders; never touch a shared cluster.

Usage: python joint_acceptance.py /tmp/pg-tokenizer-phase3-20261107
The root must contain current/lib and current/share/extension.
"""

import hashlib
import itertools
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile


root = Path(sys.argv[1]).resolve()
pg = Path(subprocess.check_output(['pg_config', '--bindir'], text=True).strip())
lib = root / 'current/lib'
share = root / 'current/share'
logs = root / 'logs'
workload = Path(__file__).with_name('combo.sql')
names = ['vchord', 'vchord_bm25', 'pg_tokenizer']
hashes = {name: hashlib.sha256((lib / (name + '.dylib')).read_bytes()).hexdigest() for name in names}
assert hashes['vchord'] == '1294402dfe45899f3f462151557615f7eb2df64c21933553760c75c067a2030e'
assert hashes['vchord_bm25'] == '44a4b29a5783a2b2b1cc89f68f9364a282d4a54f9618d49e2d28e01bda208765'
result = {'sha256': hashes, 'orders': []}


def run(args, log):
    with (logs / log).open('w') as out:
        subprocess.run([str(x) for x in args], stdout=out, stderr=subprocess.STDOUT,
                       check=True, timeout=120)


with tempfile.TemporaryDirectory(prefix='combo-', dir=root) as directory:
    temporary = Path(directory)
    data, sockets = temporary / 'data', temporary / 's'
    sockets.mkdir()
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    run([pg / 'initdb', '-D', data, '-U', 'postgres', '-A', 'trust', '--no-locale', '-E', 'UTF8'],
        'combo-initdb.log')
    with (data / 'postgresql.conf').open('a') as config:
        config.write(f"\nlisten_addresses = ''\nport = {port}\n"
                     f"unix_socket_directories = '{sockets}'\n"
                     f"extension_control_path = '{share}:$system'\n"
                     f"dynamic_library_path = '{lib}:$libdir'\n")
    running = False
    try:
        for i, order in enumerate(itertools.permutations(names)):
            if running:
                run([pg / 'pg_ctl', '-D', data, '-w', 'stop', '-m', 'fast'], f'combo-{i}-stop.log')
                running = False
            run([pg / 'pg_ctl', '-D', data, '-w', '-l', logs / 'combo-server.log',
                 '-o', '-c shared_preload_libraries=' + ','.join(order), 'start'], f'combo-{i}-start.log')
            running = True
            run([pg / 'psql', '-X', '-h', sockets, '-p', port, '-U', 'postgres', '-d', 'postgres',
                 '-v', 'ON_ERROR_STOP=1', '-f', workload], f'combo-{i}.log')
            output = (logs / f'combo-{i}.log').read_text()
            for index in ('combo_terms', 'combo_ids', 'combo_rq', 'combo_g'):
                assert f'Index Scan using {index}' in output, (i, index)
            assert 'combo passed' in output
            result['orders'].append({'order': list(order), 'result': 'pass'})
            print(','.join(order) + ': pass', flush=True)
    finally:
        if running:
            run([pg / 'pg_ctl', '-D', data, '-w', 'stop', '-m', 'fast'], 'combo-final-stop.log')

(logs / 'combo-final.json').write_text(json.dumps(result, indent=2) + '\n')
