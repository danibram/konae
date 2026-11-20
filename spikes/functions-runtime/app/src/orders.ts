import { Hono } from 'hono';
import { orderIdSchema, quantitySchema } from './schema.js';
import type { FunctionContext, FunctionEnvironment } from './contract.js';

const app = new Hono<FunctionEnvironment>();
app.onError((_, c) => c.json({ error: 'internal_error' }, 500));
app.notFound((c) => c.json({ error: 'not_found' }, 404));
app.on(['GET', 'POST'], '/:app/orders/:id', async (c) => {
  const parsedId = orderIdSchema.safeParse(c.req.param('id'));
  if (!parsedId.success) return c.json({ error: 'invalid_input' }, 400);
  const id = parsedId.data;
  if (c.req.method === 'GET') {
    const order = await c.env.data.readOrder(id);
    return order ? c.json(order) : c.json({ error: 'not_found' }, 404);
  }
  let body: unknown;
  try {
    body = await c.req.json();
  } catch {
    return c.json({ error: 'invalid_input' }, 400);
  }
  const parsed = quantitySchema.safeParse(body);
  if (!parsed.success) return c.json({ error: 'invalid_input' }, 400);
  const order = await c.env.data.updateOrder(id, parsed.data.quantity);
  return order ? c.json(order) : c.json({ error: 'not_found' }, 404);
});

export async function handle(request: Request, context: FunctionContext): Promise<Response> {
  return app.fetch(request, context);
}
