#!/usr/bin/env python3
"""Run the existing verifier and rollback fixture against the IPC/3 adapter."""
import argparse
import collections
from contextlib import contextmanager
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2] / 'spikes/functions-runtime/checks'))

from run_all import free_port  # noqa: E402
from verify import Report, Verify, request, snapshot  # noqa: E402


@contextmanager
def server(command, environment, output, label, fault=False):
    with tempfile.TemporaryDirectory(prefix=f'{label}-', dir=output) as directory:
        data = Path(directory) / 'data'
        data.mkdir()
        if fault:
            for app in ('a', 'b'):
                with sqlite3.connect(data / f'{app}.sqlite') as db:
                    db.execute('CREATE TABLE audit (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, '
                               'actor_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK (quantity != 7))')
        port = free_port()
        base = f'http://127.0.0.1:{port}'
        env = {**os.environ, **environment, 'PORT': str(port), 'DATA_DIR': str(data)}
        with tempfile.TemporaryFile() as log:
            process = subprocess.Popen(command, cwd=directory, env=env, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline and process.poll() is None:
                    try:
                        if request(base, 'GET', '/health', timeout=.5)['body'] == {'ok': True}:
                            break
                    except OSError:
                        pass
                    time.sleep(.05)
                else:
                    log.seek(0)
                    raise RuntimeError(log.read().decode(errors='replace'))
                yield base, data
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=3)


class TimedVerify(Verify):
    def __init__(self, *args, track_slowest=False):
        super().__init__(*args)
        self.track_slowest = track_slowest
        self.slowest = None

    def check(self, *args, **kwargs):
        before = snapshot(self.data_dir) if self.track_slowest else None
        super().check(*args, **kwargs)
        result = self.report.results[-1]
        elapsed = result['detail'].get('elapsed_ms')
        if elapsed is not None and (self.slowest is None or elapsed > self.slowest[0]):
            self.slowest = (elapsed, args, kwargs, before)

    def repeat_slowest(self, runs):
        if not runs or self.slowest is None:
            return None
        _, args, kwargs, before = self.slowest
        repeat = Report('slowest')
        verify = Verify(self.base, self.data_dir, repeat)
        for _ in range(runs + 1):
            for app, tables in before.items():
                with sqlite3.connect(self.data_dir / f'{app}.sqlite') as db:
                    for table, rows in tables.items():
                        db.execute(f'DELETE FROM {table}')
                        if rows:
                            values = ', '.join('?' for _ in rows[0])
                            db.executemany(f'INSERT INTO {table} VALUES ({values})', rows)
            verify.check(*args, **kwargs)
        return dict(criterion=args[0], results=repeat.results[1:])


def run(binary, node, directory, repeat_slowest=0):
    command = [node, str(HERE / 'legacy-host.mjs')]
    env = dict(KONAE_FUNCTIONS_RUNNER=str(binary))
    report = Report('wasm')
    repeated = []
    with tempfile.TemporaryDirectory(prefix='konae-wasm-prepare-') as staging:
        subprocess.run([node, str(HERE / 'legacy-host.mjs'), '--write-bundles', staging], check=True)
        prepare_env = {**os.environ, **env}
        for name in ('orders', 'preferences'):
            prepared = subprocess.run(
                [str(binary), '--prepare', str(Path(staging) / f'{name}.mjs')],
                env=prepare_env, capture_output=True, text=True,
            )
            if prepared.returncode:
                raise RuntimeError(prepared.stderr or prepared.stdout)
    with server(command, env, directory, 'wasm') as (base, data):
        verify = TimedVerify(base, data, report, track_slowest=bool(repeat_slowest))
        verify.run()
        repeated.append(verify.repeat_slowest(repeat_slowest))
    report.results = [r for r in report.results if r['criterion'] != 'internal-error.rollback']
    with server(command, env, directory, 'wasm-rollback', fault=True) as (base, data):
        verify = TimedVerify(base, data, report, track_slowest=bool(repeat_slowest))
        for app in ('a', 'b'):
            verify.check(f'{app}.internal-error.rollback-after-update', 'POST', f'/{app}/orders/1',
                         500, {'error': 'internal_error'}, f'{app}-alice', b'{"quantity":7}')
            verify.check(f'{app}.internal-error.recovery', 'POST', f'/{app}/orders/1',
                         200, {'id': 1, 'ownerId': 'alice', 'quantity': 6}, f'{app}-alice',
                         b'{"quantity":6}', mutation=(app, 1, 'alice', 6))
        repeated.append(verify.repeat_slowest(repeat_slowest))
    return report.document(), [run for run in repeated if run is not None]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runner', type=Path)
    parser.add_argument('--node', default=os.environ.get('NODE', 'node'))
    parser.add_argument('--runs', type=int, default=30)
    parser.add_argument('--repeat-slowest', type=int, default=0)
    args = parser.parse_args()
    binary = args.runner.resolve()
    all_rounds = []
    repetitions = []
    with tempfile.TemporaryDirectory(prefix='konae-wasm-suite-') as directory:
        directory = Path(directory)
        for index in range(1 if args.repeat_slowest else 0, args.runs + 1):
            document, repeated = run(binary, args.node, directory, args.repeat_slowest if index else 0)
            if index:
                all_rounds.append(document)
                repetitions.extend(repeated)
            if document['summary']['fail']:
                for result in document['results']:
                    if result['status'] == 'fail':
                        print(json.dumps(result), file=sys.stderr)
    counts = collections.Counter()
    for document in all_rounds:
        counts.update(document['summary'])
    timed = [(r['detail']['elapsed_ms'], r['criterion'], index + 1)
             for index, document in enumerate(all_rounds) for r in document['results']
             if r['status'] == 'pass' and 'elapsed_ms' in r['detail']]
    slowest = max(timed)
    same_case = [ms for ms, case, _ in timed if case == slowest[1]]
    repeats = []
    for repetition in repetitions:
        times = [case['detail']['elapsed_ms'] for case in repetition['results'] if 'elapsed_ms' in case['detail']]
        fails = sum(case['status'] == 'fail' for case in repetition['results'])
        repeats.append(dict(criterion=repetition['criterion'], n=len(repetition['results']), fail=fails,
                            max_ms=max(times, default=None), min_ms=min(times, default=None)))
    print(json.dumps(dict(rounds=args.runs, counts=counts, slowest=slowest,
                          selected_case_samples=len(same_case), selected_case_min=min(same_case),
                          repeats=repeats)))
    return int(counts['fail'] > 0 or any(repetition['fail'] for repetition in repeats))


if __name__ == '__main__':
    sys.exit(main())
