#!/usr/bin/env python3
"""Probe Hono middleware against any IPC v2 runner."""
import argparse
import base64
import hmac
from pathlib import Path
import json

from protocol import Worker

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('runner', type=Path)
parser.add_argument('bundle', type=Path)
args = parser.parse_args()
def encode(value):
    return base64.urlsafe_b64encode(value).rstrip(b'=')

unsigned = b'.'.join([encode(b'{"alg":"HS256","typ":"JWT"}'), encode(b'{"sub":"spike-fixture"}')])
signed = unsigned + b'.' + encode(hmac.digest(b'spike-fixture', unsigned, 'sha256'))


bundle = args.bundle.read_text()
for path in ('/jwt', '/etag', '/etag-custom', '/cache', '/cookie', '/stream', '/stream-timer'):
    try:
        with Worker(str(args.runner.resolve()), bundle) as worker:
            headers = [('authorization', 'Bearer ' + signed.decode())] if path == '/jwt' else None
            reply = worker.get(path, headers)
            result = reply.get('output', reply.get('code'))
    except TimeoutError as error:
        result = str(error)
    print(json.dumps(dict(path=path, result=result)), flush=True)
