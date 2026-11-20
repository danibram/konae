import { parse } from 'hono/utils/cookie';
import { orderIdSchema, quantitySchema } from '../../app/dist/schema.js';
import { jsonError } from '../lib/host.mjs';

export async function orders(request, context) {
  const segment = new URL(request.url).pathname.split('/').at(-1);
  let decoded;
  try { decoded = decodeURIComponent(segment); }
  catch { return jsonError('invalid_input', 400); }
  const parsedId = orderIdSchema.safeParse(decoded);
  if (!parsedId.success) return jsonError('invalid_input', 400);
  const id = Number(parsedId.data);
  if (request.method === 'GET') {
    const row = await context.data.readOrder(id);
    return row ? Response.json(row) : jsonError('not_found', 404);
  }
  let body;
  try { body = await request.json(); }
  catch { return jsonError('invalid_input', 400); }
  const parsed = quantitySchema.safeParse(body);
  if (!parsed.success) return jsonError('invalid_input', 400);
  const row = await context.data.updateOrder(id, parsed.data.quantity);
  return row ? Response.json(row) : jsonError('not_found', 404);
}

export async function preferences(request, context) {
  return Response.json({
    theme: parse(request.headers.get('cookie') ?? '').theme ?? 'system',
    language: request.headers.get('accept-language'),
    userId: context.actor.userId,
  }, { headers: { 'set-cookie': 'seen=1; Path=/; HttpOnly; SameSite=Lax' } });
}
