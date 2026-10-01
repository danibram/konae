// Probe: a small shop with stock that holds expire from. Decides the matrix
// cells for inventory rules under concurrency, delayed work (a hold that
// lapses), payments, and what the page may load from a payment provider.
//
//   LITTLEBOX_PROBE_PORT=23020 bun run examples/probes/shop/replay.ts
import assert from "node:assert/strict";
import { probe } from "../_kit";

const here = import.meta.dir;

await probe("shop", 23020, async (kit) => {
  const { url, owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_shop", visibility: "public", signup: { mode: "open" } });
  await owner.call("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: "INSERT INTO items (id, name, stock, hold_ms) VALUES (1, 'Print, signed', 3, 1500), (2, 'Postcard', 100, 900000)" } });
  const visitor = kit.runtime(app.host);
  const available = async () => (await visitor.call<any[]>("actions_invoke", { name: "items.list", input: {} })).map((row) => row.available);

  // Ten buyers race for three prints. The Kernel runs one action at a time per
  // Deployment and the INSERT checks the stock in its own WHERE.
  const buyers = await Promise.all(Array.from({ length: 10 }, (_, index) => kit.signup(app.host, `buyer${index}@probe.example`)));
  const clients = buyers.map((cookie) => kit.runtime(app.host, cookie));
  const outcomes = await Promise.all(clients.map((client) => client.call<any>("actions_invoke", { name: "orders.hold", input: { item: 1, qty: 1 } })
    .then((result) => `held ${result.lastInsertId}`, (error: Error) => error.message)));
  const held = outcomes.filter((outcome) => outcome.startsWith("held"));
  const refused = outcomes.filter((outcome) => !outcome.startsWith("held"));
  assert.equal(held.length, 3);
  assert.deepEqual([...new Set(refused)], ["Not enough left, or the quantity is not 1 to 5."]);
  assert.deepEqual(await available(), [0, 100]);
  const tooMany = await kit.errorText(clients[0].call("actions_invoke", { name: "orders.hold", input: { item: 2, qty: 9 } }));
  assert.equal(tooMany, "Not enough left, or the quantity is not 1 to 5.");
  kit.pass("Inventory under concurrency", `10 buyers hold 1 of 3 prints at once: 3 hold, 7 are refused with "${refused[0]}", and items.list shows 0 left. A hold of 9 postcards is refused with the same guard, since one statement cannot say which part of its WHERE said no.`);

  // A program the owner runs confirms a payment with a key: the hold becomes a sale.
  const key = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "payment adapter", actions: ["payments.confirm"], files: [] });
  const confirm = async (order: number) => {
    const answer = await kit.appFetch(app.host)(`${url}/api/payments`, { method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${key.key}` }, body: JSON.stringify({ order }) });
    return { status: answer.status, body: await answer.json() as any };
  };
  const [first, second] = held.map((outcome) => Number(outcome.split(" ")[1]));
  assert.equal((await confirm(first)).body.result.changes, 1);

  // Nothing runs later. The unpaid holds lapse because every statement compares created_at with kernel.now.
  await Bun.sleep(1700);
  assert.deepEqual(await available(), [2, 100]);
  const lapsed = await confirm(second);
  assert.equal(lapsed.status, 403);
  assert.deepEqual(lapsed.body, { error: "That hold is gone or already paid." });
  const again = await clients[9].call<any>("actions_invoke", { name: "orders.hold", input: { item: 1, qty: 2 } });
  assert.equal(again.changes, 1);
  assert.deepEqual(await available(), [0, 100]);
  const rows = await owner.call<any>("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: "SELECT status, count(*) AS n FROM orders GROUP BY status ORDER BY status" } });
  kit.pass("Holds that lapse", `The paid hold stays sold. 1.7 s later the two unpaid holds no longer count, items.list shows 2 left, confirming a lapsed hold answers ${lapsed.status} "${lapsed.body.error}", and another buyer holds the 2. The lapsed rows stay in the table as 'held' (${JSON.stringify(rows.rows ?? rows)}) because no job runs to clear them.`);

  // What the page may load from a payment provider.
  const page = await kit.appFetch(app.host)(`${url}/`);
  const policy = page.headers.get("content-security-policy") ?? "";
  for (const directive of ["default-src 'self'", "connect-src 'self'", "form-action 'self'", "frame-ancestors 'none'"]) assert(policy.includes(directive), `${directive} missing from ${policy}`);
  assert.match(policy, /script-src 'self'( 'sha256-[^']+')*;/);
  assert(!/frame-src/.test(policy));
  assert(policy.includes("default-src 'self'"));
  assert((await page.text()).includes("https://pay.example/link/abc"));
  kit.pass("Payment provider in the page", `The page's policy is "${policy}". A provider's script, its API calls, an embedded checkout frame (frame-src falls back to default-src 'self') and a form that posts to it do not run; a plain link to a hosted payment page publishes and navigates.`);
});
