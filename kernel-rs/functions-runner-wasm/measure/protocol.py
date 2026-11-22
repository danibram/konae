#!/usr/bin/env python3
"""Measure process startup, a fresh instance and warm IPC round trips."""
import argparse
import json
import os
import select
import sqlite3
from pathlib import Path
import statistics
import struct
import subprocess
import tempfile
import time

LIMITS = dict(heap_bytes=64 << 20, response_bytes=2 << 20, rss_bytes=256 << 20, operation_limit=64)
TRIVIAL = "export function handle() { return new Response('ok'); }"


def rss(pid):
    status = Path(f'/proc/{pid}/status')
    if status.exists():
        return int(next(line for line in status.read_text().splitlines() if line.startswith('VmRSS:')).split()[1])
    return int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(pid)]))


class Worker:
    def __init__(self, binary, source):
        self.started = time.perf_counter_ns()
        self.process = subprocess.Popen([binary, '--function-worker'], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.request = 0
        self.send(dict(kind='Init', abi='konae-functions-ipc/2', function='probe',
                       source=source, generation='bench', limits=LIMITS))
        try:
            ready = self.read()
            assert ready['kind'] == 'Ready', ready
        except BaseException:
            self.process.kill()
            self.process.wait()
            raise
        self.ready_ms = self.elapsed(self.started)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self.process.poll() is None:
            if exc[0] is not None:
                self.process.kill()
                self.process.wait()
                return
            try:
                self.send(dict(kind='Shutdown', grace_ms=250))
                self.read(timeout=2)
                self.process.wait(timeout=2)
            except (OSError, RuntimeError, subprocess.TimeoutExpired):
                self.process.kill()
                self.process.wait()

    @staticmethod
    def elapsed(start):
        return (time.perf_counter_ns() - start) / 1e6

    def send(self, message):
        payload = json.dumps(message).encode()
        self.process.stdin.write(struct.pack('!I', len(payload)) + payload)
        self.process.stdin.flush()

    def read(self, timeout=10):
        deadline = time.monotonic() + timeout
        fd = self.process.stdout.fileno()

        def exact(size):
            message = bytearray()
            while len(message) < size:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
                    raise TimeoutError('Worker did not reply within the probe deadline')
                chunk = os.read(fd, size - len(message))
                if not chunk:
                    raise RuntimeError('Worker stopped before replying')
                message.extend(chunk)
            return message

        header = exact(4)
        return json.loads(exact(struct.unpack('!I', header)[0]))

    def get(self, path='/', headers=None):
        return self.invoke(path=path, headers=headers)

    def invoke(self, path='/', headers=None, method='GET', body=b'', operation=None):
        self.request += 1
        capability = dict(generation='bench', request=self.request, token=f'request-{self.request}')
        self.send(dict(kind='Invoke', capability=capability,
                       input=dict(url='http://example.test' + path, method=method,
                                  headers=headers or [], body_len=len(body), actor='')))
        offset = 0
        while True:
            reply = self.read()
            assert reply['capability'] == capability, reply
            if reply['kind'] == 'BodyRead':
                chunk = body[offset:offset + reply['maximum']]
                offset += len(chunk)
                self.send(dict(kind='BodyChunk', capability=capability, sequence=reply['sequence'],
                               body_len=len(chunk), bytes=list(chunk), done=not chunk))
            elif reply['kind'] == 'Operation':
                assert reply['operation'] == 'action' and operation is not None, reply
                self.send(dict(kind='OperationResult', capability=capability, sequence=reply['sequence'],
                               result={'Ok': operation(**reply['args'])}))
            else:
                return reply


def summary(samples):
    return dict(n=len(samples), p50=round(statistics.median(samples), 3),
                p95=round(statistics.quantiles(samples, n=100, method='inclusive')[94], 3),
                max=round(max(samples), 3))


def measure_post(binary, source, runs):
    with tempfile.TemporaryDirectory(prefix='konae-wasm-post-') as directory:
        db = sqlite3.connect(Path(directory) / 'rows.sqlite')
        try:
            db.execute('CREATE TABLE counter(id INTEGER PRIMARY KEY, n INTEGER NOT NULL)')
            with Worker(binary, source) as worker:
                before = rss(worker.process.pid)

                def action(name, input):
                    if name == 'counter.add':
                        db.execute('INSERT INTO counter VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET n=n+excluded.n',
                                   (input['amount'],))
                        return []
                    assert name == 'counter.read' and input == {}, (name, input)
                    n, = db.execute('SELECT n FROM counter WHERE id=1 LIMIT 1').fetchone()
                    return [{'n': n}]

                durations = []
                for n in range(1, runs + 2):
                    started = time.perf_counter_ns()
                    with db:
                        db.execute('BEGIN IMMEDIATE')
                        result = worker.invoke(method='POST', body=b'{"amount":1}', operation=action)
                        assert result['output']['status'] == 200, result
                        assert json.loads(result['output']['body']) == {'n': n}, result
                    if n > 1:
                        durations.append(worker.elapsed(started))
                after = rss(worker.process.pid)
                return dict(warm_ipc_sqlite_post_ms=summary(durations),
                            rss_before_kib=before, rss_after_kib=after)
        finally:
            db.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runner', type=Path)
    parser.add_argument('--runs', type=int, default=30)
    parser.add_argument('--post', type=Path)
    parser.add_argument('--skip-engine-size', action='store_true')
    args = parser.parse_args()
    binary = str(args.runner.resolve())
    if args.post:
        print(json.dumps(measure_post(binary, args.post.read_text(), args.runs)))
        return
    cold = []
    ready = []
    first = []
    for index in range(args.runs + 1):
        with Worker(binary, TRIVIAL) as worker:
            started = time.perf_counter_ns()
            assert worker.get()['output']['body'] == 'ok'
            if index:
                first.append(worker.elapsed(started))
                cold.append(worker.elapsed(worker.started))
                ready.append(worker.ready_ms)
    with Worker(binary, TRIVIAL) as worker:
        before = rss(worker.process.pid)
        assert worker.get()['output']['body'] == 'ok'
        warm = []
        for _ in range(args.runs):
            started = time.perf_counter_ns()
            assert worker.get()['output']['body'] == 'ok'
            warm.append(worker.elapsed(started))
        after = rss(worker.process.pid)
    engine_size = None if args.skip_engine_size else int(subprocess.check_output([binary, '--engine-size']))
    print(json.dumps(dict(cold_first_reply_ms=summary(cold), init_ms=summary(ready),
                          first_invoke_ms=summary(first), warm_ipc_get_ms=summary(warm),
                          rss_before_kib=before, rss_after_kib=after,
                          binary_bytes=os.stat(binary).st_size,
                          compiled_engine_bytes=engine_size)))


if __name__ == '__main__':
    main()
