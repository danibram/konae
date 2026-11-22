#!/usr/bin/env python3
"""Measure process startup, a fresh instance and warm IPC round trips."""
import argparse
import json
import os
from pathlib import Path
import statistics
import struct
import subprocess
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
        ready = self.read()
        assert ready['kind'] == 'Ready', ready
        self.ready_ms = self.elapsed(self.started)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self.process.poll() is None:
            try:
                self.send(dict(kind='Shutdown', grace_ms=250))
                self.read()
            finally:
                self.process.wait(timeout=3)

    @staticmethod
    def elapsed(start):
        return (time.perf_counter_ns() - start) / 1e6

    def send(self, message):
        payload = json.dumps(message).encode()
        self.process.stdin.write(struct.pack('!I', len(payload)) + payload)
        self.process.stdin.flush()

    def read(self):
        header = self.process.stdout.read(4)
        assert len(header) == 4, 'Worker stopped before replying'
        return json.loads(self.process.stdout.read(struct.unpack('!I', header)[0]))

    def get(self, path='/', headers=None):
        self.request += 1
        capability = dict(generation='bench', request=self.request, token=f'request-{self.request}')
        self.send(dict(kind='Invoke', capability=capability,
                       input=dict(url='http://example.test' + path, method='GET',
                                  headers=headers or [], body_len=0, actor='')))
        reply = self.read()
        assert reply['capability'] == capability, reply
        return reply


def summary(samples):
    return dict(n=len(samples), p50=round(statistics.median(samples), 3),
                p95=round(statistics.quantiles(samples, n=100, method='inclusive')[94], 3),
                max=round(max(samples), 3))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runner', type=Path)
    parser.add_argument('--runs', type=int, default=30)
    args = parser.parse_args()
    binary = str(args.runner.resolve())
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
    print(json.dumps(dict(cold_first_reply_ms=summary(cold), init_ms=summary(ready),
                          first_invoke_ms=summary(first), warm_ipc_get_ms=summary(warm),
                          rss_before_kib=before, rss_after_kib=after,
                          binary_bytes=os.stat(binary).st_size,
                          compiled_engine_bytes=int(subprocess.check_output([binary, '--engine-size'])))))


if __name__ == '__main__':
    main()
