import { Hono } from "hono";
import { z } from "zod";

type FunctionContext = {
  readonly actor: { readonly userId: string | null };
  readonly actions: { call(name: string, input?: Record<string, unknown>): Promise<unknown> };
};

const input = z.object({ title: z.string().trim().min(1).max(80) }).strict();
const batchInput = z.object({ titles: z.array(z.string().trim().min(1).max(80)).min(1).max(10) }).strict();
const BUILD_VERSION = "v2";
const app = new Hono<{ Bindings: { context: FunctionContext } }>();
app.notFound((c) => c.json({ error: "not_found" }, 404));

app.get("/api/notes/", async (c) => {
  const rows = await c.env.context.actions.call("notes.list");
  return c.json({ version: BUILD_VERSION, notes: rows });
});
app.post("/api/notes/", async (c) => {
  let body: unknown;
  try { body = await c.req.json(); } catch { return c.json({ error: "invalid_input" }, 400); }
  const parsed = input.safeParse(body);
  if (!parsed.success) return c.json({ error: "invalid_input" }, 400);
  const result = await c.env.context.actions.call("notes.create", parsed.data);
  return c.json(result, 201);
});
app.post("/api/notes/batch", async (c) => {
  let body: unknown;
  try { body = await c.req.json(); } catch { return c.json({ error: "invalid_input" }, 400); }
  const parsed = batchInput.safeParse(body);
  if (!parsed.success) return c.json({ error: "invalid_input" }, 400);
  const result = await c.env.context.actions.call("notes.batch", { titlesJson: JSON.stringify(parsed.data.titles) });
  return c.json(result, 201);
});
app.post("/api/notes/batch-guard", async (c) => {
  try {
    const result = await c.env.context.actions.call("notes.batch", { titlesJson: '["synthetic before guard",""]' });
    return c.json(result, 201);
  } catch {
    return c.json({ error: "batch_refused" }, 409);
  }
});
app.get("/api/notes/forbidden", async (c) => {
  const result = await c.env.context.actions.call("notes.delete", { id: 1 });
  return c.json(result);
});
app.onError(() => new Response(JSON.stringify({ error: "internal_error" }), { status: 500 }));

export function handle(request: Request, context: FunctionContext): Response | Promise<Response> {
  return app.fetch(request, { context });
}
