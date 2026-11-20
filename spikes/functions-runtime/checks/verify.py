#!/usr/bin/env python3
"""Comprueba HTTP y efectos SQLite del SPEC sin dependencias externas."""
import argparse
import copy
from contextlib import closing
import hashlib
import http.client
import json
from pathlib import Path
import sqlite3
import sys
import time
from urllib.parse import urlsplit


class Report:
    def __init__(self, name):
        self.name = name
        self.results = []

    def add(self, criterion, ok=None, **detail):
        self.results.append(dict(criterion=criterion,
                                 status='skip' if ok is None else 'pass' if ok else 'fail',
                                 detail=detail))

    def document(self):
        return dict(server=self.name, results=self.results,
                    summary={s: sum(r['status'] == s for r in self.results)
                             for s in ('pass', 'fail', 'skip')})


def request(base, method, path, body=None, headers=None, timeout=5):
    url = urlsplit(base)
    if url.scheme != 'http' or url.hostname not in ('127.0.0.1', 'localhost', '::1'):
        raise ValueError('El verificador admite solo HTTP loopback')
    conn = http.client.HTTPConnection(url.hostname, url.port, timeout=timeout)
    sent = dict(headers or {})
    if body is not None:
        sent.setdefault('Content-Type', 'application/json')
    try:
        conn.request(method, path, body=body, headers=sent)
        response = conn.getresponse()
        raw = response.read(1024 * 1024 + 1)
        if len(raw) > 1024 * 1024:
            raise ValueError('La respuesta supera el límite de lectura de 1 MiB')
        text = raw.decode('utf-8', errors='replace')
        try:
            value = json.loads(text)
        except ValueError:
            value = None
        return dict(status=response.status, body=value, raw=text,
                    headers=response.getheaders())
    finally:
        conn.close()


def snapshot(data_dir):
    out = {}
    for app in ('a', 'b'):
        path = Path(data_dir).resolve() / f'{app}.sqlite'
        with closing(sqlite3.connect(path.as_uri() + '?mode=ro', uri=True, timeout=2)) as db:
            db.execute('PRAGMA query_only=ON')
            db.execute('BEGIN')
            out[app] = {table: db.execute(f'SELECT * FROM {table} ORDER BY {key}').fetchall()
                        for table, key in (('orders', 'id'), ('audit', 'id'), ('sessions', 'token'))}
    return out


def fingerprint(state):
    return hashlib.sha256(json.dumps(state, sort_keys=True).encode()).hexdigest()


def json_equal(left, right):
    # JSON distingue un booleano de un número aunque Python los compare iguales.
    return json.dumps(left, sort_keys=True, separators=(',', ':')) == json.dumps(
        right, sort_keys=True, separators=(',', ':'))


class Verify:
    def __init__(self, base, data_dir, report, timeout=5):
        self.base, self.data_dir, self.report, self.timeout = base, data_dir, report, timeout

    def check(self, name, method, path, status, expected=None, token=None, body=None,
              headers=None, mutation=None, cookie=False):
        sent = dict(headers or {})
        if token is not None:
            sent['Cookie'] = f'session={token}' + ('; ' + sent['Cookie'] if 'Cookie' in sent else '')
        response = None
        before = None
        started = time.monotonic()
        try:
            before = snapshot(self.data_dir)
            response = request(self.base, method, path, body, sent, self.timeout)
            after = snapshot(self.data_dir)
            expected_state = copy.deepcopy(before)
            db_ok = before == after
            if mutation:
                app, order_id, actor, quantity = mutation
                expected_state[app]['orders'] = [
                    (row[0], row[1], quantity) if row[0] == order_id else row
                    for row in before[app]['orders']]
                old_audit, new_audit = before[app]['audit'], after[app]['audit']
                added = [row for row in new_audit if row not in old_audit]
                audit_ok = (len(added) == 1 and tuple(added[0][1:]) == (order_id, actor, quantity)
                            and all(row in new_audit for row in old_audit)
                            and len(new_audit) == len(old_audit) + 1)
                expected_state[app]['audit'] = new_audit
                db_ok = audit_ok and after == expected_state
            header_map = {k.lower(): v for k, v in response['headers']}
            body_ok = expected is None or json_equal(response['body'], expected)
            content_type_ok = expected is None or header_map.get('content-type', '').split(';')[0] == 'application/json'
            cookies = [v for k, v in response['headers'] if k.lower() == 'set-cookie']
            cookie_ok = not cookie or cookies == ['seen=1; Path=/; HttpOnly; SameSite=Lax']
            ok = response['status'] == status and body_ok and db_ok and content_type_ok and cookie_ok
            detail = dict(method=method, path=path, expected_status=status, expected_body=expected,
                          response=response, database_ok=db_ok, body_ok=body_ok,
                          content_type_ok=content_type_ok, cookie_ok=cookie_ok,
                          before_sha256=fingerprint(before), after_sha256=fingerprint(after),
                          elapsed_ms=round((time.monotonic() - started) * 1000, 2))
            if not db_ok:
                detail.update(before=before, after=after)
            self.report.add(name, ok, **detail)
        except Exception as exc:
            detail = dict(error=f'{type(exc).__name__}: {exc}', response=response)
            if before is not None:
                try:
                    after = snapshot(self.data_dir)
                    detail.update(database_unchanged=before == after,
                                  before_sha256=fingerprint(before), after_sha256=fingerprint(after))
                except Exception as db_exc:
                    detail['database_error'] = str(db_exc)
            self.report.add(name, False, **detail)

    def run(self):
        initial = snapshot(self.data_dir)
        for app in ('a', 'b'):
            rows = initial[app]['orders']
            sessions = {row[0]: row[1:] for row in initial[app]['sessions']}
            self.report.add(f'{app}.fixture',
                [(r[0], r[1]) for r in rows] == [(1, 'alice'), (2, 'bob')]
                and sessions.get(f'{app}-alice') == ('alice', 4102444800)
                and sessions.get(f'{app}-bob') == ('bob', 4102444800)
                and sessions.get(f'{app}-expired', (None, None))[1] == 1,
                orders=rows, session_tokens=sorted(sessions))
        self.check('health', 'GET', '/health', 200, {'ok': True})
        for app, other in (('a', 'b'), ('b', 'a')):
            unauthorized = {'error': 'unauthorized'}
            for label, token in (('missing', None), ('invalid', 'unknown'),
                                 ('expired', f'{app}-expired'), ('cross-app', f'{other}-alice')):
                for method, suffix, body in (
                    ('GET', 'orders/1', None), ('GET', 'orders/invalid', None),
                    ('POST', 'orders/1', b'{"quantity":3}'),
                    ('POST', 'orders/invalid', b'{'),
                    ('POST', 'orders/1', b' ' * 16385),
                    ('GET', 'preferences', None)):
                    self.check(f'{app}.auth.{label}.{method}.{suffix}.{len(body or b"")}',
                               method, f'/{app}/{suffix}', 401, unauthorized, token, body)
            for method in ('GET', 'POST'):
                body = b'{"quantity":3}' if method == 'POST' else None
                for actor, foreign_id in (('alice', 2), ('bob', 1)):
                    self.check(f'{app}.authorization.{actor}.{method}', method,
                               f'/{app}/orders/{foreign_id}', 404, {'error': 'not_found'},
                               f'{app}-{actor}', body)
                self.check(f'{app}.absent.{method}', method, f'/{app}/orders/999999999',
                           404, {'error': 'not_found'}, f'{app}-alice', body)
                for invalid in ('0', '-1', 'abc', '1.5', '1e1', '1000000000', '%20', '%27', '%C3%B1', '%ZZ'):
                    self.check(f'{app}.id.{method}.{invalid}', method, f'/{app}/orders/{invalid}',
                               400, {'error': 'invalid_input'}, f'{app}-alice', body)
            invalid_bodies = [b'', b'{', b'null', b'[]', b'1', b'"a"', b'{}',
                              b'{"quantity":0}', b'{"quantity":11}', b'{"quantity":-1}',
                              b'{"quantity":1.5}', b'{"quantity":"3"}', b'{"quantity":true}',
                              b'{"quantity":null}', b'{"quantity":[]}', b'{"quantity":{}}',
                              b'{"quantity":3,"ownerId":"bob"}', b'{"quantity":3,"extra":null}',
                              b'{"quantity":NaN}', b'{"quantity":Infinity}', b'\xff']
            for index, body in enumerate(invalid_bodies):
                self.check(f'{app}.body.invalid.{index}', 'POST', f'/{app}/orders/1', 400,
                           {'error': 'invalid_input'}, f'{app}-alice', body)
            self.check(f'{app}.body.oversized', 'POST', f'/{app}/orders/1', 413,
                       {'error': 'body_too_large'}, f'{app}-alice', b' ' * 16385)
            for actor, order_id in (('alice', 1), ('bob', 2)):
                for quantity in (1, 10, 3):
                    expected = dict(id=order_id, ownerId=actor, quantity=quantity)
                    self.check(f'{app}.update.{actor}.{quantity}', 'POST', f'/{app}/orders/{order_id}',
                               200, expected, f'{app}-{actor}', json.dumps({'quantity': quantity}).encode(),
                               mutation=(app, order_id, actor, quantity))
                    self.check(f'{app}.readback.{actor}.{quantity}', 'GET', f'/{app}/orders/{order_id}',
                               200, expected, f'{app}-{actor}')
            body = b'{"quantity":2}'
            self.check(f'{app}.body.exact-limit', 'POST', f'/{app}/orders/1', 200,
                       dict(id=1, ownerId='alice', quantity=2), f'{app}-alice',
                       body + b' ' * (16384 - len(body)), mutation=(app, 1, 'alice', 2))
        for actor, order_id, quantity in (('alice', 1, 5), ('bob', 2, 8)):
            self.check(f'b.distinct-data.{actor}', 'POST', f'/b/orders/{order_id}', 200,
                       dict(id=order_id, ownerId=actor, quantity=quantity), f'b-{actor}',
                       json.dumps({'quantity': quantity}).encode(), mutation=('b', order_id, actor, quantity))
        self.check('actor.cannot-be-overridden-by-header', 'GET', '/a/orders/2', 404,
                   {'error': 'not_found'}, 'a-alice', headers={'X-User-Id': 'bob', 'X-App-Id': 'b'})
        for index, (app, actor, theme, language) in enumerate((
            ('a', 'alice', 'dark', 'es-ES,es;q=0.9'), ('a', 'bob', None, None),
            ('b', 'alice', 'light', 'en-GB'), ('b', 'bob', None, 'fr'),
            ('a', 'bob', 'dark', None), ('b', 'alice', None, None),
            ('a', 'alice', None, None), ('b', 'bob', 'light', 'de'))):
            headers = {}
            if theme is not None:
                headers['Cookie'] = f'theme={theme}'
            if language is not None:
                headers['Accept-Language'] = language
            self.check(f'alternation.preferences.{index}', 'GET', f'/{app}/preferences', 200,
                       dict(theme=theme or 'system', language=language, userId=actor),
                       f'{app}-{actor}', headers=headers, cookie=True)
            order_id = 1 if actor == 'alice' else 2
            quantity = snapshot(self.data_dir)[app]['orders'][order_id - 1][2]
            self.check(f'alternation.orders.{index}', 'GET', f'/{app}/orders/{order_id}', 200,
                       dict(id=order_id, ownerId=actor, quantity=quantity), f'{app}-{actor}')
        for path in ('/unknown', '/a/unknown', '/c/orders/1', '/a/orders/1/extra'):
            self.check(f'route.{path}', 'GET', path, 404, token='a-alice')
        self.report.add('internal-error.rollback', None,
                        reason='verify.py no modifica el esquema; run_all.py prueba rollback con un fixture aparte')
        for criterion, reason in (
            ('production.export-import', 'No se exporta ni importa una carpeta Kernel real'),
            ('production.gate-runtime', 'El experimento no integra Gate ni límites SQL del Kernel'),
            ('production.owner-tools', 'No expone el MCP real ni credenciales owner'),
            ('production.native-cancellation', 'El timeout JS no demuestra interrupción SQLite'),
            ('production.idempotency', 'El SPEC no implementa idempotencia'),
            ('production.os-isolation', 'No se prueba una sandbox del sistema operativo'),
            ('production.arbitrary-global-state', 'Alternar handlers concretos no prueba todo código posible')):
            self.report.add(criterion, None, reason=reason)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', required=True)
    parser.add_argument('--data-dir', required=True)
    parser.add_argument('--name', default='server')
    parser.add_argument('--timeout', type=float, default=5)
    parser.add_argument('--output')
    args = parser.parse_args()
    report = Report(args.name)
    try:
        Verify(args.url, args.data_dir, report, args.timeout).run()
    except Exception as exc:
        report.add('verification.setup', False, error=f'{type(exc).__name__}: {exc}')
    result = report.document()
    result.update(url=args.url, data_dir=str(Path(args.data_dir).resolve()))
    encoded = json.dumps(result, ensure_ascii=False, indent=2) + '\n'
    if args.output:
        Path(args.output).write_text(encoded)
    else:
        sys.stdout.write(encoded)
    return int(result['summary']['fail'] > 0)


if __name__ == '__main__':
    sys.exit(main())
