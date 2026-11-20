#!/usr/bin/env python3
"""Inspecciona ops expuestas en un proceso propio; solo permite destino loopback propio."""
import argparse
import json
from pathlib import Path
import socket
import sqlite3
import tempfile
import threading
import time

from run_all import HERE, SPIKE, call, record_artifacts, server, stats
from verify import Report, request, snapshot


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='/tmp/konae-deno-core-target/release/konae-functions-spike')
    parser.add_argument('--output-dir', default=str(HERE / 'results'))
    args = parser.parse_args()
    output = Path(args.output_dir).resolve()
    if not output.is_relative_to(HERE):
        parser.error('--output-dir debe estar dentro de checks/')
    output.mkdir(parents=True, exist_ok=True)
    record_artifacts(output, args.binary, filename='probe-artifacts.json')
    report = Report('runtime-ops-probe')
    with tempfile.TemporaryDirectory(prefix='probe-bundle-', dir=output) as temp, socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        listener.listen(1)
        port = listener.getsockname()[1]
        source = '''
export function handle(request) {
  const exposed = globalThis.Deno?.core ?? globalThis.__bootstrap?.core;
  const ops = exposed?.ops ?? (typeof core === 'undefined' ? undefined : core.ops);
  if (new URL(request.url).searchParams.has('invoke')) {
    try {
      const value = ops.op_fetch('GET', 'http://127.0.0.1:PORT/', [], null, false, null, null);
      return Response.json({ returned: value });
    } catch (error) {
      return Response.json({ caught: String(error) });
    }
  }
  return Response.json({ fetchGlobal: typeof globalThis.fetch,
    denoCore: !!globalThis.Deno?.core, bootstrapCore: !!globalThis.__bootstrap?.core,
    lexicalCore: typeof core, lexicalReadOrder: typeof readOrder,
    ops: ops ? Object.keys(ops).filter(name => /fetch|net_connect|net_listen/.test(name)).sort() : [] });
}
'''.replace('PORT', str(port))
        (Path(temp) / 'orders.js').write_text(source)
        config = dict(MODE='deno', FUNCTIONS_ENABLED='1', CACHE_CAPACITY='2', CACHE_TTL_MS='60000',
                      JS_TIMEOUT_MS='100', APP_BUNDLE_DIR=temp)
        try:
            with server([str(Path(args.binary).resolve())], config, output, 'runtime-ops-probe') as (base, _):
                inspected = request(base, 'GET', '/a/orders/1', headers={'Cookie': 'session=a-alice'}, timeout=2)
                report.add('production.raw-network-ops', inspected['status'] == 200
                           and not inspected['body'].get('ops'), response=inspected,
                           requirement='Las capacidades de red deben pasar por el control del Kernel')
                if 'op_fetch' in (inspected.get('body') or {}).get('ops', []):
                    try:
                        invoked = request(base, 'GET', '/a/orders/1?invoke=1',
                                          headers={'Cookie': 'session=a-alice'}, timeout=2)
                        denied = 'permission' in str(invoked.get('body', {})).lower()
                        report.add('production.raw-fetch-denial', denied, response=invoked)
                    except Exception as exc:
                        report.add('production.raw-fetch-denial', False,
                                   error=f'{type(exc).__name__}: {exc}',
                                   reason='El proceso propio no devolvió un rechazo de permisos')
                else:
                    report.add('production.raw-fetch-denial', None, reason='op_fetch no está accesible')
        except Exception as exc:
            report.add('probe.execution', False, error=f'{type(exc).__name__}: {exc}')
    config = dict(MODE='deno', FUNCTIONS_ENABLED='1', CACHE_CAPACITY='2', CACHE_TTL_MS='60000',
                  JS_TIMEOUT_MS='100', APP_BUNDLE_DIR=str(SPIKE / 'app/dist'))
    try:
        with server([str(Path(args.binary).resolve())], config, output, 'native-cancellation') as (base, data_dir):
            call(base)
            before = snapshot(data_dir)
            response = {}
            def update():
                try:
                    response['value'] = request(base, 'POST', '/a/orders/1', b'{"quantity":7}',
                                                {'Cookie': 'session=a-alice'}, timeout=2)
                except Exception as exc:
                    response['error'] = f'{type(exc).__name__}: {exc}'
            with sqlite3.connect(data_dir / 'a.sqlite', timeout=1) as blocker:
                blocker.execute('BEGIN IMMEDIATE')
                worker = threading.Thread(target=update, daemon=True)
                worker.start()
                time.sleep(.35)
                blocker.rollback()
            worker.join(timeout=3)
            after = snapshot(data_dir)
            counters = stats(base)
            report.add('native-timeout.no-mutation', before == after and counters['timeouts'] == 1,
                       response=response, before=before, after=after, stats=counters,
                       lock_ms=350, js_budget_ms=100,
                       requirement='Un fallo no modifica las tablas; SPEC Datos y HTTP')
    except Exception as exc:
        report.add('native-timeout.execution', False, error=f'{type(exc).__name__}: {exc}')
    document = report.document()
    (output / 'runtime-ops-probe.json').write_text(json.dumps(document, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps(document, ensure_ascii=False, indent=2))
    return int(document['summary']['fail'] > 0)


if __name__ == '__main__':
    raise SystemExit(main())
