#!/usr/bin/env python3
"""Probe Hono middleware against any IPC v2 runner."""
import argparse
from pathlib import Path
import json

from protocol import Worker

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('runner', type=Path)
parser.add_argument('bundle', type=Path)
args = parser.parse_args()
with Worker(str(args.runner.resolve()), args.bundle.read_text()) as worker:
    for path in ('/jwt', '/etag', '/etag-custom', '/cache', '/cookie', '/stream'):
        reply = worker.get(path)
        print(json.dumps(dict(path=path, result=reply.get('output', reply.get('code')))))
