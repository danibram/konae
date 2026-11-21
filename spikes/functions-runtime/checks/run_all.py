#!/usr/bin/env python3
"""Arranca procesos desechables y verifica los cuatro servidores en serie."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

from verify import Report, Verify, request

HERE = Path(__file__).resolve().parent
SPIKE = HERE.parent


def record_artifacts(output, binary, node='node', filename='artifacts.json'):
    paths = [Path(binary), SPIKE / 'rust-host/Cargo.lock', SPIKE / 'baselines/package-lock.json']
    paths += sorted((SPIKE / 'rust-host/src').glob('*'))
    paths += sorted((SPIKE / 'app/dist').glob('*.js'))
    try:
        node_version = subprocess.check_output([node, '--version'], text=True, timeout=2).strip()
    except (OSError, subprocess.SubprocessError):
        node_version = None
    document = dict(captured_at=datetime.now(timezone.utc).isoformat(), python=sys.version.split()[0],
                    node=node_version, artifacts={str(p): hashlib.sha256(p.read_bytes()).hexdigest()
                                                  for p in paths if p.is_file()})
    (output / filename).write_text(json.dumps(document, indent=2) + '\n')


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


@contextmanager
def server(command, environment, output, label, fault=False):
    with tempfile.TemporaryDirectory(prefix=f'{label}-', dir=output) as directory:
        directory = Path(directory)
        data_dir = directory / 'data'
        data_dir.mkdir()
        if fault:
            for app in ('a', 'b'):
                with sqlite3.connect(data_dir / f'{app}.sqlite') as db:
                    db.execute('CREATE TABLE audit (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, '
                               'actor_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK (quantity != 7))')
        port = free_port()
        base = f'http://127.0.0.1:{port}'
        env = {**os.environ, **environment, 'PORT': str(port), 'DATA_DIR': str(data_dir),
               'NEXT_TELEMETRY_DISABLED': '1'}
        with (output / f'{label}.log').open('w') as log:
            process = subprocess.Popen(command, cwd=directory, env=env, stdout=log, stderr=log,
                                       start_new_session=True)
            try:
                deadline = time.monotonic() + 15
                while True:
                    if process.poll() is not None:
                        raise RuntimeError(f'{label} terminó con código {process.returncode}')
                    try:
                        response = request(base, 'GET', '/health', timeout=.5)
                        if response['status'] == 200 and response['body'] == {'ok': True}:
                            break
                    except (OSError, TimeoutError):
                        pass
                    if time.monotonic() >= deadline:
                        raise TimeoutError(f'{label} no responde a /health en 15 s')
                    time.sleep(.05)
                yield base, data_dir
            finally:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
                # Next puede tener hijos que siguen vivos tras terminar su padre.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=2)


def stats(base):
    response = request(base, 'GET', '/__stats')
    if response['status'] != 200 or not isinstance(response['body'], dict):
        raise RuntimeError(f'/__stats inesperado: {response}')
    return response['body']


def call(base, app='a', function='orders', actor='alice'):
    path = f'/{app}/orders/{1 if actor == "alice" else 2}' if function == 'orders' else f'/{app}/preferences'
    response = request(base, 'GET', path, headers={'Cookie': f'session={app}-{actor}'})
    if response['status'] != 200:
        raise RuntimeError(f'Petición fallida: {response}')
    return response


def keys(value):
    return sorted((x['app'], x['function'], x['version']) for x in value['instances'])


def lifecycle(binary, output):
    report = Report('deno-lifecycle')
    env = dict(MODE='deno', FUNCTIONS_ENABLED='1', CACHE_CAPACITY='4', CACHE_TTL_MS='60000',
               JS_TIMEOUT_MS='100', APP_BUNDLE_DIR=str(SPIKE / 'app/dist'))
    scenarios = ('reuse-version', 'capacity', 'ttl', 'disabled', 'sync-loop')
    for scenario in scenarios:
        config = dict(env)
        if scenario == 'capacity':
            config['CACHE_CAPACITY'] = '2'
        if scenario == 'ttl':
            config['CACHE_TTL_MS'] = '200'
        if scenario == 'disabled':
            config['FUNCTIONS_ENABLED'] = '0'
        try:
            with server([binary], config, output, f'deno-{scenario}') as (base, data_dir):
                before = stats(base)
                report.add(f'{scenario}.no-calls', before['runtimesCreated'] == 0
                           and before['runtimesLive'] == 0 and not before['instances'], stats=before)
                if scenario == 'disabled':
                    for path in ('/a/orders/1', '/b/preferences', '/__control/loop'):
                        response = request(base, 'GET', path, headers={'Cookie': 'session=a-alice'})
                        report.add(f'disabled.refusal.{path}', response['status'] == 404, response=response)
                    after = stats(base)
                    report.add('disabled.no-runtime', not after['functionsEnabled']
                               and after['runtimesCreated'] == 0 and after['runtimesLive'] == 0
                               and not after['instances'], stats=after)
                    continue
                call(base)
                first = stats(base)
                report.add(f'{scenario}.first-call', first['runtimesCreated'] == 1
                           and keys(first) == [('a', 'orders', 'v1')], stats=first)
                if scenario == 'sync-loop':
                    started = time.monotonic()
                    # Este endpoint solo se llama al proceso creado en este bloque.
                    loop = request(base, 'GET', '/__control/loop', timeout=2)
                    elapsed = time.monotonic() - started
                    after = stats(base)
                    report.add('sync-loop.watchdog', loop['status'] == 500
                               and loop['body'] == {'error': 'internal_error'}
                               and after['timeouts'] == first['timeouts'] + 1 and elapsed < 2,
                               response=loop, elapsed_ms=round(elapsed * 1000, 2), stats=after)
                    recovered = call(base, actor='bob')
                    final = stats(base)
                    report.add('sync-loop.recovery', recovered['body'] == {'id': 2, 'ownerId': 'bob', 'quantity': 4}
                               and final['runtimesCreated'] == first['runtimesCreated'] + 1, stats=final)
                    continue
                if scenario == 'ttl':
                    deadline = time.monotonic() + 2
                    after = stats(base)
                    while after['runtimesLive'] and time.monotonic() < deadline:
                        time.sleep(.05)
                        after = stats(base)
                    report.add('ttl.eviction', not after['instances'] and after['runtimesLive'] == 0
                               and after['evictions'] > first['evictions'], stats=after)
                    call(base)
                    final = stats(base)
                    report.add('ttl.recreation', final['runtimesCreated'] == 2, stats=final)
                    continue
                if scenario == 'capacity':
                    call(base, function='preferences')
                    call(base, app='b')
                    after = stats(base)
                    report.add('capacity.eviction', len(after['instances']) == 2
                               and ('a', 'orders', 'v1') not in keys(after)
                               and after['runtimesCreated'] == 3 and after['evictions'] == 1,
                               stats=after)
                    call(base)
                    final = stats(base)
                    report.add('capacity.recreation', len(final['instances']) == 2
                               and final['runtimesCreated'] == 4 and final['evictions'] == 2, stats=final)
                    continue
                call(base, actor='bob')
                reused = stats(base)
                report.add('reuse.same-function', reused['runtimesCreated'] == 1
                           and reused['cacheHits'] == first['cacheHits'] + 1,
                           stats=reused)
                call(base, function='preferences')
                independent = stats(base)
                report.add('independent.preferences', independent['runtimesCreated'] == 2
                           and keys(independent) == [('a', 'orders', 'v1'), ('a', 'preferences', 'v1')],
                           stats=independent)
                call(base, app='b')
                other = stats(base)
                report.add('independent.app', other['runtimesCreated'] == 3
                           and ('b', 'orders', 'v1') in keys(other), stats=other)
                control = request(base, 'POST', '/__control/version',
                                  b'{"app":"a","function":"orders","version":"v2"}')
                invalidated = stats(base)
                report.add('version.invalidation', control['status'] == 200
                           and ('a', 'orders', 'v1') not in keys(invalidated)
                           and ('a', 'preferences', 'v1') in keys(invalidated)
                           and ('b', 'orders', 'v1') in keys(invalidated), stats=invalidated)
                call(base)
                versioned = stats(base)
                report.add('version.new-instance', versioned['runtimesCreated'] == 4
                           and ('a', 'orders', 'v2') in keys(versioned), stats=versioned)
                request(base, 'POST', '/__control/evict', b'')
                deadline = time.monotonic() + 2
                after = stats(base)
                while after['runtimesLive'] and time.monotonic() < deadline:
                    time.sleep(.05)
                    after = stats(base)
                report.add('manual.eviction', not after['instances'] and after['runtimesLive'] == 0,
                           stats=after)
        except Exception as exc:
            report.add(f'{scenario}.execution', False, error=f'{type(exc).__name__}: {exc}')
    for criterion, reason in (
        ('version.changed-code', 'El control cambia la clave, no aporta otro bundle con comportamiento diferente'),
        ('capacity.busy-eviction', 'No se ejercita desalojo concurrente de instancias ocupadas'),
        ('engine.global-initialization', 'Los contadores describen runtimes, no RSS ni inicialización global de V8'),
        ('snapshot.trust', 'El experimento no carga snapshots externos'),
        ('native.cancellation', 'El bucle prueba JS síncrono; no interrumpe SQLite'),
        ('startup.deadline', 'El control loop se ejecuta después de evaluar el módulo')):
        report.add(criterion, None, reason=reason)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='/tmp/konae-deno-core-target/release/konae-functions-spike')
    parser.add_argument('--node', default='node')
    parser.add_argument('--only', choices=('all', 'native', 'deno', 'node', 'next', 'lifecycle'), default='all')
    parser.add_argument('--output-dir', default=str(HERE / 'results'))
    args = parser.parse_args()
    output = Path(args.output_dir).resolve()
    if not output.is_relative_to(HERE):
        parser.error('--output-dir debe estar dentro de checks/')
    output.mkdir(parents=True, exist_ok=True)
    record_artifacts(output, args.binary, args.node)
    reports = []
    for name in ('native', 'deno', 'node', 'next'):
        if args.only not in ('all', name):
            continue
        if name in ('native', 'deno'):
            command = [str(Path(args.binary).resolve())]
            env = dict(MODE=name, FUNCTIONS_ENABLED='1', CACHE_CAPACITY='4', CACHE_TTL_MS='60000',
                       JS_TIMEOUT_MS='100', APP_BUNDLE_DIR=str(SPIKE / 'app/dist'))
        else:
            command = [args.node, str(SPIKE / f'baselines/{name}/' / ('server.mjs' if name == 'node' else 'start.mjs'))]
            env = {}
        report = Report(name)
        try:
            with server(command, env, output, name) as (base, data_dir):
                Verify(base, data_dir, report).run()
            report.results = [r for r in report.results if r['criterion'] != 'internal-error.rollback']
            with server(command, env, output, f'{name}-rollback', fault=True) as (base, data_dir):
                verify = Verify(base, data_dir, report)
                for app in ('a', 'b'):
                    verify.check(f'{app}.internal-error.rollback-after-update', 'POST', f'/{app}/orders/1',
                                 500, {'error': 'internal_error'}, f'{app}-alice', b'{"quantity":7}')
                    verify.check(f'{app}.internal-error.recovery', 'POST', f'/{app}/orders/1',
                                 200, {'id': 1, 'ownerId': 'alice', 'quantity': 6}, f'{app}-alice',
                                 b'{"quantity":6}', mutation=(app, 1, 'alice', 6))
        except Exception as exc:
            report.add('server.execution', False, error=f'{type(exc).__name__}: {exc}')
        reports.append(report)
        print(f'{name} {report.document()["summary"]}', flush=True)
    if args.only in ('all', 'lifecycle'):
        report = lifecycle(str(Path(args.binary).resolve()), output)
        reports.append(report)
        print(f'lifecycle {report.document()["summary"]}', flush=True)
    for report in reports:
        (output / f'{report.name}.json').write_text(json.dumps(report.document(), ensure_ascii=False, indent=2) + '\n')
    return int(any(r.document()['summary']['fail'] for r in reports))


if __name__ == '__main__':
    sys.exit(main())
