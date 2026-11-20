#!/usr/bin/env python3
"""Comprueba deadlines, replay y capacidades en servidores y datos desechables."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import sqlite3
import tempfile
import time

from run_all import HERE, SPIKE, record_artifacts, server, stats
from verify import Report, request


def state(directory):
    with sqlite3.connect((directory / 'a.sqlite').as_uri() + '?mode=ro', uri=True) as db:
        return {
            'quantity': db.execute('SELECT quantity FROM orders WHERE id=1').fetchone()[0],
            'audit': db.execute('SELECT count(*) FROM audit').fetchone()[0],
            'receipts': db.execute('SELECT count(*) FROM mutation_receipts').fetchone()[0],
        }


def http(base, method='GET', path='/a/orders/1', quantity=7, key=None, actor='alice'):
    headers = {'Cookie': f'session=a-{actor}'}
    if key is not None:
        headers['Idempotency-Key'] = key
    body = json.dumps({'quantity': quantity}).encode() if method == 'POST' else None
    started = time.monotonic()
    response = request(base, method, path, body, headers, timeout=3)
    response['elapsed_ms'] = (time.monotonic() - started) * 1000
    return response


def expected_error(response, status, code):
    return response['status'] == status and response['body'] == {'error': code}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    if not output.is_relative_to(HERE) or output.exists():
        parser.error('--output debe ser un directorio nuevo dentro de checks/')
    output.mkdir(parents=True)
    binary = str(args.binary.resolve())
    record_artifacts(output, binary)
    provenance = output / 'artifacts.json'
    artifacts = json.loads(provenance.read_text())
    executor = SPIKE.parents[1] / 'kernel-rs/functions-executor'
    sources = [Path(__file__).resolve(), executor / 'Cargo.toml']
    sources += sorted((executor / 'src').rglob('*.rs'))
    sources += sorted((SPIKE / 'rust-host/src').rglob('*.rs'))
    for source in sources:
        artifacts['artifacts'][str(source)] = hashlib.sha256(source.read_bytes()).hexdigest()
    provenance.write_text(json.dumps(artifacts, indent=2) + '\n')
    (output / 'tested-script.py').write_bytes(Path(__file__).read_bytes())
    report = Report('hardening-http')
    config = dict(MODE='deno', FUNCTIONS_ENABLED='1', CACHE_CAPACITY='2',
                  CACHE_TTL_MS='60000', JS_TIMEOUT_MS='1000',
                  APP_BUNDLE_DIR=str(SPIKE / 'app/dist'))

    with server([binary], config, output, 'idempotency') as (base, directory):
        initial = stats(base)
        report.add('lazy.no-worker-before-call', initial['initializationAttempts'] == 0,
                   stats=initial)
        before = state(directory)
        bob_before = http(base, path='/a/orders/2', actor='bob')
        foreign = [http(base, 'POST', '/a/orders/2', key='foreign-row') for _ in range(2)]
        bob_after = http(base, path='/a/orders/2', actor='bob')
        after = state(directory)
        report.add('authorization.foreign-order-leaves-no-receipt',
                   all(expected_error(reply, 404, 'not_found') for reply in foreign)
                   and bob_before['status'] == bob_after['status'] == 200
                   and bob_before['body'] == bob_after['body']
                   and before == after == dict(quantity=2, audit=0, receipts=0),
                   responses=foreign, bob_before=bob_before, bob_after=bob_after,
                   before=before, after=after)
        first = http(base, 'POST', key='one')
        second = http(base, 'POST', key='one')
        current = state(directory)
        report.add('idempotency.exact-replay', first['status'] == second['status'] == 200
                   and first['body'] == second['body']
                   and current == dict(quantity=7, audit=1, receipts=1),
                   first=first, second=second, database=current)
        conflict = http(base, 'POST', quantity=8, key='one')
        report.add('idempotency.conflict', expected_error(conflict, 409, 'idempotency_conflict')
                   and state(directory) == current, response=conflict, database=state(directory))
        missing = http(base, 'POST')
        report.add('idempotency.key-required', expected_error(missing, 400, 'idempotency_key_required')
                   and state(directory) == current, response=missing)
        with ThreadPoolExecutor(max_workers=2) as pool:
            replies = list(pool.map(lambda _: http(base, 'POST', quantity=8, key='concurrent'), range(2)))
        report.add('idempotency.concurrent-replay', all(r['status'] == 200 for r in replies)
                   and replies[0]['body'] == replies[1]['body']
                   and state(directory) == dict(quantity=8, audit=2, receipts=2),
                   responses=replies, database=state(directory))

    for mode, budget_ms, lock_seconds in [('native', 100, .35), ('deno', 300, .55)]:
        label = f'{mode}-lock'
        with server([binary], {**config, 'MODE': mode, 'REQUEST_TIMEOUT_MS': str(budget_ms)}, output, label) as (base, directory):
            warmed = http(base)
            report.add(f'{label}.warmed', warmed['status'] == 200, response=warmed)
            before = state(directory)
            with sqlite3.connect(directory / 'a.sqlite') as lock, ThreadPoolExecutor(max_workers=1) as pool:
                lock.execute('BEGIN IMMEDIATE')
                pending = pool.submit(http, base, 'POST', '/a/orders/1', 7, 'locked')
                time.sleep(lock_seconds)
                lock.rollback()
                response = pending.result(timeout=2)
            report.add(f'{label}.no-late-commit', expected_error(response, 504, 'deadline_exceeded')
                       and state(directory) == before and response['elapsed_ms'] < lock_seconds * 1000 - 50,
                       response=response, before=before, after=state(directory),
                       budget_ms=budget_ms, lock_ms=lock_seconds * 1000)
            retry = http(base, 'POST', key='locked')
            report.add(f'{label}.retry-once', retry['status'] == 200
                       and state(directory) == dict(quantity=7, audit=1, receipts=1),
                       response=retry, database=state(directory))

    sources = {
        'initialization': 'while (true) {} export function handle() {}',
        'loop': 'export function handle() { while (true) {} }',
        'pending': 'export function handle() { return new Promise(() => {}); }',
    }
    good = 'export function handle(request) { return Response.json({ok:true,signal:request.signal instanceof AbortSignal}); }'
    for label, source in sources.items():
        with tempfile.TemporaryDirectory(dir=output) as folder:
            bundle = Path(folder) / 'orders.js'
            bundle.write_text(source)
            env = {**config, 'APP_BUNDLE_DIR': folder, 'REQUEST_TIMEOUT_MS': '400'}
            with server([binary], env, output, label) as (base, directory):
                started = time.monotonic()
                failed = http(base)
                elapsed = time.monotonic() - started
                health = request(base, 'GET', '/health')
                lifecycle = stats(base)
                report.add(f'{label}.bounded-and-parent-alive',
                           expected_error(failed, 504, 'deadline_exceeded') and elapsed < 1.5
                           and health['status'] == 200 and state(directory)['audit'] == 0
                           and lifecycle['runtimesCreated'] == (0 if label == 'initialization' else 1),
                           response=failed, elapsed_ms=elapsed * 1000, health=health, stats=lifecycle)
                bundle.write_text(good)
                recovered = http(base)
                report.add(f'{label}.recovery', recovered['status'] == 200
                           and recovered['body'] == {'ok': True, 'signal': True}, response=recovered)

    with tempfile.TemporaryDirectory(dir=output) as folder:
        (Path(folder) / 'orders.js').write_text('''
let retained;
export async function handle(request, context) {
  if (!retained) {
    retained = context.data.readOrder;
    return Response.json({saved:true});
  }
  try {
    const leaked = await retained(2);
    return Response.json({leaked});
  } catch (error) {
    return Response.json({blocked:true, error:String(error)});
  }
}
''')
        with server([binary], {**config, 'APP_BUNDLE_DIR': folder}, output, 'stale-capability') as (base, directory):
            saved = http(base)
            stale = http(base, path='/a/orders/2', actor='bob')
            report.add('capability.cannot-rebind-next-actor', saved['status'] == 200
                       and expected_error(stale, 403, 'capability_expired')
                       and state(directory) == dict(quantity=2, audit=0, receipts=0), saved=saved, stale=stale,
                       database=state(directory))

    document = report.document()
    (output / 'hardening-http.json').write_text(json.dumps(document, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(document['summary']))
    return int(document['summary']['fail'] > 0)


if __name__ == '__main__':
    raise SystemExit(main())
