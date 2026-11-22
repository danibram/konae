#!/usr/bin/env python3
"""Run the existing verifier and rollback fixture against the IPC v2 adapter."""
import argparse
import collections
import json
import os
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2] / 'spikes/functions-runtime/checks'))

from run_all import server  # noqa: E402
from verify import Report, Verify  # noqa: E402


def run(binary, node, directory):
    command = [node, str(HERE / 'legacy-host.mjs')]
    env = dict(KONAE_FUNCTIONS_RUNNER=str(binary))
    report = Report('wasm')
    with server(command, env, directory, 'wasm') as (base, data):
        Verify(base, data, report).run()
    report.results = [r for r in report.results if r['criterion'] != 'internal-error.rollback']
    with server(command, env, directory, 'wasm-rollback', fault=True) as (base, data):
        verify = Verify(base, data, report)
        for app in ('a', 'b'):
            verify.check(f'{app}.internal-error.rollback-after-update', 'POST', f'/{app}/orders/1',
                         500, {'error': 'internal_error'}, f'{app}-alice', b'{"quantity":7}')
            verify.check(f'{app}.internal-error.recovery', 'POST', f'/{app}/orders/1',
                         200, {'id': 1, 'ownerId': 'alice', 'quantity': 6}, f'{app}-alice',
                         b'{"quantity":6}', mutation=(app, 1, 'alice', 6))
    return report.document()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runner', type=Path)
    parser.add_argument('--node', default=os.environ.get('NODE', 'node'))
    parser.add_argument('--runs', type=int, default=30)
    args = parser.parse_args()
    binary = args.runner.resolve()
    all_rounds = []
    with tempfile.TemporaryDirectory(prefix='konae-wasm-suite-') as directory:
        directory = Path(directory)
        for index in range(args.runs + 1):
            document = run(binary, args.node, directory)
            if index:
                all_rounds.append(document)
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
    print(json.dumps(dict(rounds=args.runs, counts=counts, slowest=slowest,
                          selected_case_samples=len(same_case), selected_case_min=min(same_case))))
    return int(counts['fail'] > 0)


if __name__ == '__main__':
    sys.exit(main())
