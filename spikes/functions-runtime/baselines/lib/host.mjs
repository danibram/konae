import { parse } from 'hono/utils/cookie';
import { openDatabases } from './database.mjs';

export const bodyLimit = 16384;
export const jsonError = (error, status) => Response.json({ error }, { status });

async function boundedRequest(request) {
  if (!request.body) return request;
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > bodyLimit) {
      void reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new Request(request.url, { method: request.method, headers: request.headers, body });
}

export function createHost({ directory, orders, preferences }) {
  const databases = openDatabases(directory);
  return {
    databases,
    close() { for (const { db } of databases.values()) db.close(); },
    async fetch(request) {
      try {
        const path = new URL(request.url).pathname;
        if (path === '/health' && request.method === 'GET') return Response.json({ ok: true });
        const match = /^\/(a|b)\/(orders\/[^/]+|preferences)$/.exec(path);
        if (!match) return jsonError('not_found', 404);
        const isOrders = match[2].startsWith('orders/');
        if (!(request.method === 'GET' || (isOrders && request.method === 'POST'))) return jsonError('not_found', 404);
        const database = databases.get(match[1]);
        const token = parse(request.headers.get('cookie') ?? '').session;
        const userId = token ? database.authenticate(token) : null;
        if (!userId) return jsonError('unauthorized', 401);
        const bounded = await boundedRequest(request);
        if (!bounded) return jsonError('body_too_large', 413);
        const context = { actor: { userId }, data: database.forActor(userId) };
        return await (isOrders ? orders : preferences)(bounded, context);
      } catch {
        return jsonError('internal_error', 500);
      }
    },
  };
}
