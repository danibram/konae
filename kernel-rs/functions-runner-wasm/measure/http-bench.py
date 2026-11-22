#!/usr/bin/env python3
"""Measure a trivial GET and a real transaction on a running dev Kernel."""
import argparse
from http.client import HTTPConnection
import json
import subprocess
import time
import uuid

from protocol import rss, summary


def children(pid):
    rows = subprocess.check_output(['ps', '-A', '-o', 'pid=,ppid=,comm='], text=True)
    return {int(child): rss(int(child))
            for child, parent, *_ in map(str.split, rows.splitlines()) if int(parent) == pid}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--port', type=int, required=True)
    parser.add_argument('--host', default='wasm-probe.localhost')
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--runs', type=int, default=30)
    args = parser.parse_args()
    conn = HTTPConnection('127.0.0.1', args.port, timeout=10)
    host = f'{args.host}:{args.port}'
    origin = f'http://{host}'
    conn.request('POST', '/_konae/auth/signup', json.dumps({
        'email': f'wasm-{uuid.uuid4().hex}@example.test',
        'password': 'disposable wasm benchmark',
    }), {'Host': host, 'Origin': origin, 'Content-Type': 'application/json'})
    signup = conn.getresponse()
    payload = signup.read()
    assert signup.status == 200, (signup.status, payload)
    cookie = signup.getheader('Set-Cookie').split(';', 1)[0]

    def request(method):
        headers = {'Host': host, 'Origin': origin, 'Cookie': cookie}
        body = None
        path = '/api/hello'
        if method == 'POST':
            headers.update({'Content-Type': 'application/json', 'Idempotency-Key': uuid.uuid4().hex})
            body = '{"amount":1}'
            path = '/api/count'
        started = time.perf_counter_ns()
        conn.request(method, path, body, headers)
        response = conn.getresponse()
        data = response.read().decode()
        elapsed = (time.perf_counter_ns() - started) / 1e6
        assert response.status == 200, (response.status, data)
        return data, elapsed

    first_get, cold_get = request('GET')
    first_post, cold_post = request('POST')
    before = children(args.pid)
    assert first_get == 'wasm-ok'
    start_count = json.loads(first_post)['n']
    times = {'GET': [], 'POST': []}
    last_post = None
    for index in range(args.runs):
        body, elapsed = request('GET')
        assert body == first_get
        times['GET'].append(elapsed)
        last_post, elapsed = request('POST')
        assert json.loads(last_post)['n'] == start_count + index + 1
        times['POST'].append(elapsed)
    after = children(args.pid)
    print(json.dumps(dict(first_get=first_get, first_post=first_post, last_post=last_post,
                          cold_get_ms=cold_get, cold_post_ms=cold_post,
                          warm_get_ms=summary(times['GET']), warm_post_ms=summary(times['POST']),
                          rss_before_kib=before, rss_after_kib=after)))
    if len(before) != 2 or before.keys() != after.keys():
        raise RuntimeError(f'Unexpected worker RSS samples under Kernel PID {args.pid}: {before}, {after}')


if __name__ == '__main__':
    main()
