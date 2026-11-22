#!/usr/bin/env python3
"""Add the trivial and transaction functions to a freshly initialized app folder."""
import json
from pathlib import Path
import sys

root = Path(sys.argv[1])
settings = root / 'konae.json'
config = json.loads(settings.read_text())
config.update(visibility='public', modules={name: {'on': True} for name in ('actions', 'auth', 'functions')})
settings.write_text(json.dumps(config, indent=2) + '\n')

app = root / 'app'
functions = app / 'functions'
functions.mkdir(exist_ok=True)
(functions / 'hello.js').write_text("export function handle() { return new Response('wasm-ok'); }\n")
(functions / 'count.js').write_text('''
export async function handle(request, context) {
  const { amount } = await request.json();
  await context.actions.call('counter.add', { amount });
  const rows = await context.actions.call('counter.read', {});
  return Response.json({ n: rows[0].n });
}
''')
(app / 'functions.json').write_text(json.dumps({'version': 1, 'functions': {
    'hello': {'mount': '/api/hello', 'entry': 'functions/hello.js', 'access': 'anonymous', 'methods': ['GET']},
    'count': {'mount': '/api/count', 'entry': 'functions/count.js', 'access': 'anonymous',
              'methods': ['POST'], 'actions': ['counter.add', 'counter.read']},
}}, indent=2))
(app / 'actions.json').write_text(json.dumps({'version': 1, 'actions': {
    'counter.add': {'kind': 'mutation', 'access': 'anonymous', 'statement': 'INSERT INTO counter VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET n=n+excluded.n',
            'bindings': [{'source': 'input', 'key': 'amount', 'type': 'number'}]},
    'counter.read': {'kind': 'query', 'access': 'anonymous', 'statement': 'SELECT n FROM counter WHERE id=1 LIMIT 1',
             'bindings': []},
}}, indent=2))
(app / 'schema.sql').write_text('CREATE TABLE IF NOT EXISTS counter(id INTEGER PRIMARY KEY, n INTEGER NOT NULL);\n')
