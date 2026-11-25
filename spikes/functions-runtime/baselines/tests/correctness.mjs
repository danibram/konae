import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { request as httpRequest } from 'node:http';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const isBun = Boolean(process.versions.bun);
const Database = isBun ? (await import('bun:sqlite')).Database : (await import('node:sqlite')).DatabaseSync;
const testApi = isBun ? await import('bun:test') : await import('node:test');
const { describe, test } = testApi;
const beforeAll = testApi.beforeAll ?? testApi.before;
const afterAll = testApi.afterAll ?? testApi.after;
const node = isBun ? Bun.which('node') : process.execPath;
assert.ok(node, 'The baseline servers require Node on PATH');

const root = fileURLToPath(new URL('..', import.meta.url));
mkdirSync(resolve(root, '.tmp'), { recursive: true });

for (const [name, script, port] of [['node', 'node/server.mjs', 43303], ['next', 'next/start.mjs', 43304]]) {
  describe(`${name} production HTTP and SQLite`, () => {
    const base = `http://127.0.0.1:${port}`;
    let directory;
    let child;
    let db;
    let before;

    beforeAll(async () => {
      directory = mkdtempSync(resolve(root, '.tmp', `${name}-`));
      child = spawn(node, [script], {
        cwd: root,
        env: { ...process.env, NODE_ENV: 'production', PORT: String(port), DATA_DIR: directory, NEXT_TELEMETRY_DISABLED: '1' },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      let output = '';
      child.stdout.on('data', (data) => { output += data; });
      child.stderr.on('data', (data) => { output += data; });
      await once(child, 'spawn');

      let ready = false;
      for (let attempt = 0; attempt < 200; attempt++) {
        if (child.exitCode !== null) throw new Error(`Server exited: ${output}`);
        try {
          const response = await fetch(`${base}/health`);
          if (response.ok) { ready = true; break; }
        } catch {}
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      assert.ok(ready, output);
      db = new Database(resolve(directory, 'a.sqlite'));
      before = rows();
    });

    afterAll(async () => {
      if (child?.pid && child.exitCode === null && child.signalCode === null) {
        const exited = once(child, 'exit');
        child.kill('SIGTERM');
        const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
        await exited;
        clearTimeout(timer);
      }
      try {
        if (isBun) db?.close(true);
        else db?.close();
      } finally {
        if (directory) rmSync(directory, { recursive: true, force: true });
      }
    });

    const request = (path, { session = 'a-alice', method = 'GET', body, headers = {} } = {}) => new Promise((resolve, reject) => {
      const outgoing = httpRequest(`${base}${path}`, {
        method,
        headers: { ...(session ? { cookie: `session=${session}` } : {}), ...headers },
      }, (incoming) => {
        const chunks = [];
        incoming.on('data', (chunk) => chunks.push(chunk));
        incoming.on('end', () => {
          const responseHeaders = new Headers();
          for (let i = 0; i < incoming.rawHeaders.length; i += 2) {
            responseHeaders.append(incoming.rawHeaders[i], incoming.rawHeaders[i + 1]);
          }
          resolve(new Response(Buffer.concat(chunks), { status: incoming.statusCode, headers: responseHeaders }));
        });
      });
      outgoing.on('error', reject);
      if (body !== undefined) outgoing.write(typeof body === 'string' ? body : JSON.stringify(body));
      outgoing.end();
    });
    async function expect(path, options, status, expected) {
      const response = await request(path, options);
      assert.equal(response.status, status, `${path}: ${await response.clone().text()}`);
      assert.match(response.headers.get('content-type'), /^application\/json/);
      assert.deepEqual(await response.json(), expected);
      return response;
    }
    const failure = (error) => ({ error });
    const alice = (quantity) => ({ id: 1, ownerId: 'alice', quantity });
    const query = (sql) => isBun ? db.query(sql) : db.prepare(sql);
    const rows = () => ({
      orders: query('SELECT * FROM orders ORDER BY id LIMIT 100').all(),
      audit: query('SELECT * FROM audit ORDER BY id LIMIT 100').all(),
    });
    test('health and undeclared routes', async () => {
      await expect('/health', {}, 200, { ok: true });
      await expect('/missing', {}, 404, failure('not_found'));
      await expect('/c/orders/1', {}, 404, failure('not_found'));
      await expect('/a/orders/1', { method: 'DELETE' }, 404, failure('not_found'));
    });
    test('authentication precedes ID, JSON and size validation', async () => {
      for (const session of [null, 'unknown', 'a-expired', 'b-alice']) {
        await expect('/a/orders/1', { session }, 401, failure('unauthorized'));
        await expect('/a/orders/no', { session, method: 'POST', body: '{' }, 401, failure('unauthorized'));
        await expect('/a/orders/1', { session, method: 'POST', body: 'x'.repeat(16385) }, 401, failure('unauthorized'));
      }
    });
    test('IDs and own rows', async () => {
      await expect('/a/orders/1', {}, 200, alice(2));
      await expect('/a/orders/2', {}, 404, failure('not_found'));
      await expect('/a/orders/999', {}, 404, failure('not_found'));
      for (const id of ['0', '-1', '1.5', 'abc', '1234567890']) {
        await expect(`/a/orders/${id}`, {}, 400, failure('invalid_input'));
      }
    });
    const malformedUrlTest = name === 'next' ? (test.failing ?? test.todo) : test;
    const malformedUrlLabel = name === 'next'
      ? 'Next returns 500 before the handler. SPEC requires 400 JSON.'
      : 'malformed URL escape returns the SPEC JSON error';
    malformedUrlTest(malformedUrlLabel, async () => {
      await expect('/a/orders/%ZZ', {}, 400, failure('invalid_input'));
    });
    test('strict quantity validation leaves tables unchanged', async () => {
      for (const body of ['{', null, [], {}, { quantity: 0 }, { quantity: 11 }, { quantity: 1.5 }, { quantity: '2' }, { quantity: 2, extra: true }]) {
        await expect('/a/orders/1', { method: 'POST', body }, 400, failure('invalid_input'));
      }
      await expect('/a/orders/2', { method: 'POST', body: { quantity: 7 } }, 404, failure('not_found'));
      await expect('/a/orders/99', { method: 'POST', body: { quantity: 7 } }, 404, failure('not_found'));
      assert.deepEqual(rows(), before);
    });
    test('body size bounds and update plus audit', async () => {
      const payload = JSON.stringify({ quantity: 7 });
      await expect('/a/orders/1', { method: 'POST', body: payload.padEnd(16385) }, 413, failure('body_too_large'));
      assert.deepEqual(rows(), before);
      await expect('/a/orders/1', { method: 'POST', body: payload.padEnd(16384) }, 200, alice(7));
      await expect('/a/orders/1', {}, 200, alice(7));
      assert.deepEqual(rows().audit.map((row) => ({ ...row })), [{ id: 1, order_id: 1, actor_id: 'alice', quantity: 7 }]);
      await expect('/b/orders/1', { session: 'b-alice' }, 200, alice(2));
      await expect('/a/orders/2', { session: 'a-bob' }, 200, { id: 2, ownerId: 'bob', quantity: 4 });
    });
    test('audit failure rolls back update', async () => {
      const snapshot = rows();
      db.exec("CREATE TRIGGER audit_fail BEFORE INSERT ON audit BEGIN SELECT RAISE(FAIL, 'test failure'); END;");
      await expect('/a/orders/1', { method: 'POST', body: { quantity: 8 } }, 500, failure('internal_error'));
      assert.deepEqual(rows(), snapshot);
      db.exec('DROP TRIGGER audit_fail');
    });
    test('preferences carry headers, cookies and actor', async () => {
      const response = await expect('/a/preferences', {}, 200, { theme: 'system', language: null, userId: 'alice' });
      assert.equal(response.headers.get('set-cookie'), 'seen=1; Path=/; HttpOnly; SameSite=Lax');
      await expect('/b/preferences', {
        session: 'b-bob', headers: { cookie: 'session=b-bob; theme=dark', 'accept-language': 'es-ES,es;q=0.9' },
      }, 200, { theme: 'dark', language: 'es-ES,es;q=0.9', userId: 'bob' });
      await expect('/a/preferences', {}, 200, { theme: 'system', language: null, userId: 'alice' });
      assert.equal(query('PRAGMA journal_mode').get().journal_mode, 'wal');
    });
  });
}
