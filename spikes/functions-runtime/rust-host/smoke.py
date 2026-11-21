#!/usr/bin/env python3
"""Comprueba HTTP y SQLite en procesos propios, sin medir rendimiento."""
import contextlib
import http.client
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

BINARY = Path(sys.argv[1] if len(sys.argv) > 1 else '/tmp/konae-deno-core-target/release/konae-functions-spike')
ASSERTIONS = 0


def equal(actual, expected):
    global ASSERTIONS
    assert actual == expected, (actual, expected)
    ASSERTIONS += 1


@contextlib.contextmanager
def server(mode, **settings):
    with tempfile.TemporaryDirectory(prefix='konae-deno-smoke-') as directory:
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        env = dict(os.environ, MODE=mode, PORT=str(port), DATA_DIR=directory, **settings)
        with open(Path(directory) / 'server.log', 'w+') as log:
            process = subprocess.Popen([str(BINARY)], env=env, stdout=log, stderr=log)
            try:
                for _ in range(200):
                    if process.poll() is not None:
                        raise AssertionError('El servidor terminó durante el arranque')
                    try:
                        with socket.create_connection(('127.0.0.1', port), timeout=.1):
                            break
                    except OSError:
                        time.sleep(.02)
                else:
                    raise AssertionError('El servidor no abrió el puerto')
                yield port, Path(directory)
            except BaseException:
                log.seek(0)
                print(log.read(), file=sys.stderr)
                raise
            finally:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def request(port, path, method='GET', body=None, cookie='session=a-alice', headers=None):
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=3)
    try:
        fields = dict(headers or {})
        if cookie is not None:
            fields['Cookie'] = cookie
        if isinstance(body, dict):
            body = json.dumps(body)
            fields['Content-Type'] = 'application/json'
        conn.request(method, path, body=body, headers=fields)
        out = conn.getresponse()
        return out.status, json.loads(out.read()), dict(out.getheaders())
    finally:
        conn.close()


def check_http(mode):
    with server(mode) as (port, directory):
        initial = request(port, '/__stats')[1]
        equal(initial['runtimesCreated'], 0)
        equal(initial['engineInitialized'], False)
        equal(request(port, '/health')[:2], (200, {'ok': True}))
        equal(request(port, '/a/orders/1')[:2], (200, {'id': 1, 'ownerId': 'alice', 'quantity': 2}))
        for cookie in [None, 'session=unknown', 'session=a-expired', 'session=b-alice']:
            equal(request(port, '/a/orders/nope', cookie=cookie)[:2], (401, {'error': 'unauthorized'}))
        equal(request(port, '/a/orders/2')[:2], (404, {'error': 'not_found'}))
        equal(request(port, '/a/orders/2', cookie='session=a-bob')[1]['ownerId'], 'bob')
        equal(request(port, '/a/orders/1')[1]['ownerId'], 'alice')
        for raw in ['', '{', '{"quantity":2,"extra":true}', '{"quantity":1.5}', '{"quantity":11}', '[]']:
            equal(request(port, '/a/orders/1', 'POST', raw)[:2], (400, {'error': 'invalid_input'}))
        equal(request(port, '/a/orders/1', 'POST', 'x' * 16385)[:2], (413, {'error': 'body_too_large'}))
        equal(request(port, '/a/orders/1', 'POST', {'quantity': 7})[:2], (200, {'id': 1, 'ownerId': 'alice', 'quantity': 7}))
        equal(request(port, '/b/orders/1', cookie='session=b-alice')[1]['quantity'], 2)
        prefs = request(port, '/a/preferences', cookie='session=a-alice; theme=dark', headers={'accept-language': 'es'})
        equal(prefs[:2], (200, {'theme': 'dark', 'language': 'es', 'userId': 'alice'}))
        equal(prefs[2]['set-cookie'], 'seen=1; Path=/; HttpOnly; SameSite=Lax')
        with sqlite3.connect(directory / 'a.sqlite') as db:
            equal(db.execute('SELECT order_id,actor_id,quantity FROM audit').fetchall(), [(1, 'alice', 7)])
            db.execute("CREATE TRIGGER fail_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT, 'smoke'); END")
        equal(request(port, '/a/orders/1', 'POST', {'quantity': 9})[:2], (500, {'error': 'internal_error'}))
        equal(request(port, '/a/orders/1')[1]['quantity'], 7)
        stats = request(port, '/__stats')[1]
        equal(stats['runtimesCreated'], 3 if mode == 'deno' else 0)
    print(f'{mode}: HTTP, actor y rollback correctos')


def check_lifecycle():
    with server('deno', CACHE_CAPACITY='1', CACHE_TTL_MS='300') as (port, _):
        equal(request(port, '/a/orders/1')[0], 200)
        equal(request(port, '/a/orders/1')[0], 200)
        equal(request(port, '/__stats')[1]['cacheHits'], 1)
        equal(request(port, '/a/preferences')[0], 200)
        stats = request(port, '/__stats')[1]
        equal(stats['runtimesLive'], 1)
        equal(stats['evictions'], 1)
        equal(stats['instances'][0]['function'], 'preferences')
        equal(request(port, '/__control/version', 'POST', {'app': 'a', 'function': 'preferences', 'version': 'v2'})[0], 200)
        equal(request(port, '/__stats')[1]['runtimesLive'], 0)
        equal(request(port, '/a/preferences')[0], 200)
        equal(request(port, '/__stats')[1]['instances'][0]['version'], 'v2')
        time.sleep(.6)
        equal(request(port, '/__stats')[1]['runtimesLive'], 0)
        equal(request(port, '/__stats')[1]['instances'], [])
    with server('deno', FUNCTIONS_ENABLED='0') as (port, _):
        equal(request(port, '/a/orders/1')[0], 404)
        equal(request(port, '/__control/loop')[0], 404)
        equal(request(port, '/__stats')[1]['runtimesCreated'], 0)
        equal(request(port, '/__stats')[1]['engineInitialized'], False)
    with server('deno', JS_TIMEOUT_MS='100') as (port, _):
        equal(request(port, '/__control/loop')[:2], (500, {'error': 'internal_error'}))
        equal(request(port, '/__stats')[1]['timeouts'], 1)
        equal(request(port, '/__stats')[1]['runtimesLive'], 0)
        equal(request(port, '/a/orders/1')[0], 200)
        equal(request(port, '/__stats')[1]['runtimesCreated'], 2)
    print('deno: capacidad, TTL sin tráfico, versión, off y watchdog correctos')


if __name__ == '__main__':
    check_http('native')
    check_http('deno')
    check_lifecycle()
    print(f'{ASSERTIONS} comprobaciones correctas; todos los procesos de prueba terminados')
