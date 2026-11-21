import { Hono } from 'hono';
import { getCookie } from 'hono/cookie';
import type { FunctionContext, FunctionEnvironment } from './contract.js';

const app = new Hono<FunctionEnvironment>();
app.onError((_, c) => c.json({ error: 'internal_error' }, 500));
app.notFound((c) => c.json({ error: 'not_found' }, 404));
app.get('/:app/preferences', (c) => {
  c.header('set-cookie', 'seen=1; Path=/; HttpOnly; SameSite=Lax');
  return c.json({
    theme: getCookie(c, 'theme') ?? 'system',
    language: c.req.header('accept-language') ?? null,
    userId: c.env.actor.userId,
  });
});

export async function handle(request: Request, context: FunctionContext): Promise<Response> {
  return app.fetch(request, context);
}
