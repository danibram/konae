// Probe: a small shop with stock that holds expire from. Decides the matrix
// cells for inventory rules under concurrency, delayed work (a hold that
// lapses), payments (Checkout Session via a call, then a Stripe-shaped
// webhook), and what the page may load from a payment provider.
//
//   KONAE_PROBE_PORT=23020 bun run examples/probes/shop/replay.ts
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { probe } from "../_kit";

const here = import.meta.dir;
const paymentHook = "whsec_shop_payment_hook_0123456789";

await probe("shop", 23020, async (kit) => {
  const { url, owner } = kit;
  const port = new URL(url).port;

  const pay = await kit.publish(path.join(here, "pay"), { projectId: "probe_pay", visibility: "public" });
  const payOrigin = `http://${pay.host}:${port}`;
  const payKey = await owner.call<{ key: string }>("api_keys_create", {
    deploymentId: pay.deploymentId, name: "shop checkout", actions: ["sessions.create"], files: [],
  });

  await owner.call("project_create", { projectId: "probe_shop", name: "probe_shop", slug: "probe-shop", manifest: { version: 1, visibility: "public" } });
  await owner.call("secret_set", { projectId: "probe_shop", name: "pay_key", value: payKey.key, use: "call" });
  await owner.call("secret_set", { projectId: "probe_shop", name: "payment_hook", value: paymentHook });
  const shopSource = (await readFile(path.join(here, "store/actions.json"), "utf8")).replaceAll("http://pay.localhost", payOrigin);
  const app = await kit.publish(path.join(here, "store"), {
    projectId: "probe_shop",
    visibility: "public",
    signup: { mode: "open" },
    files: { "actions.json": shopSource },
  });

  await owner.call("database_execute", {
    deploymentId: app.deploymentId,
    operation: "sql",
    input: { sql: "INSERT INTO items (id, name, stock, hold_ms, price) VALUES (1, 'Print, signed', 3, 1500, 'price_print'), (2, 'Postcard', 100, 900000, 'price_card')" },
  });
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

  const until = async <T>(read: () => Promise<T>, done: (value: T) => boolean, what: string) => {
    for (const deadline = Date.now() + 15_000; Date.now() < deadline; await Bun.sleep(100)) {
      const value = await read();
      if (done(value)) return value;
    }
    assert.fail(`Timed out waiting for ${what}.`);
  };

  // Checkout Session: the shop calls the pay app with a nested Stripe-shaped
  // body and Idempotency-Key; the then mutation stores the session id.
  const [first, second, third] = held.map((outcome) => Number(outcome.split(" ")[1]));
  // Price and quantity are constants in actions.json so the page cannot pick
  // another Stripe price. A call still cannot bind the actor, so orderId is
  // trusted the way F25 already documents for call inputs.
  const checkout = await clients[0].call<{ callId: string; queued: boolean }>("actions_invoke", {
    name: "orders.checkout",
    input: { orderId: first, idempotency: `order-${first}` },
  });
  assert.equal(checkout.queued, true);
  const mine = async () => clients[0].call<any[]>("actions_invoke", { name: "orders.mine", input: {} });
  const withSession = await until(mine, (rows) => rows.some((row) => row.id === first && row.session_id != null), "the checkout session");
  const sessionId = withSession.find((row) => row.id === first).session_id;
  assert.equal(typeof sessionId, "number");
  const outbox = await owner.call<{ calls: { action: string; state: string; status: number | null; then: { state: string } | null }[] }>("calls_read", { deploymentId: app.deploymentId });
  assert.deepEqual(outbox.calls.map((call) => [call.action, call.state, call.status, call.then?.state]), [["orders.checkout", "delivered", 200, "done"]]);
  kit.pass("Checkout Session via a call", `orders.checkout queues a POST to ${payOrigin}/v1/checkout/sessions with nested line_items, Idempotency-Key order-${first}, and the pay key. The then mutation stores session_id ${sessionId} on order ${first}.`);

  // A Stripe-shaped webhook marks the hold paid. A second payment for the same order is refused.
  const seconds = () => Math.floor(Date.now() / 1000);
  const stripe = (body: string, t = seconds()) => ({
    "Stripe-Signature": `t=${t},v1=${createHmac("sha256", paymentHook).update(`${t}.${body}`).digest("hex")}`,
  });
  const paid = (event: string, order: number) => JSON.stringify({
    id: event,
    type: "checkout.session.completed",
    data: { object: { id: `cs_${event}`, client_reference_id: order, amount_total: 2400, currency: "eur" } },
  });
  const post = (body: string) => kit.appFetch(app.host)(`${url}/hooks/payment`, {
    method: "POST", headers: { "content-type": "application/json", ...stripe(body) }, body,
  });
  const answer = async (response: Response) => ({ status: response.status, body: await response.json() as any });
  const payment = await answer(await post(paid("evt_1", first)));
  assert.deepEqual(payment, { status: 200, body: { result: { changes: 1, lastInsertId: 0 } } });
  const again = await answer(await post(paid("evt_2", first)));
  assert.deepEqual(again, { status: 403, body: { error: "That hold is gone or already paid." } });
  const paidRow = (await mine()).find((row) => row.id === first);
  assert.deepEqual([paidRow.status, paidRow.payment_id], ["paid", "cs_evt_1"]);
  kit.pass("Payment provider webhook", `A checkout.session.completed signed Stripe's way marks order ${first} paid with payment_id cs_evt_1. A second payment answers ${again.status} ${JSON.stringify(again.body)}.`);

  // Unpaid holds lapse because every statement compares created_at with kernel.now.
  await Bun.sleep(1700);
  assert.deepEqual(await available(), [2, 100]);
  const lapsed = await answer(await post(paid("evt_3", second)));
  assert.equal(lapsed.status, 403);
  assert.deepEqual(lapsed.body, { error: "That hold is gone or already paid." });
  const againHold = await clients[9].call<any>("actions_invoke", { name: "orders.hold", input: { item: 1, qty: 2 } });
  assert.equal(againHold.changes, 1);
  assert.deepEqual(await available(), [0, 100]);
  // third stayed held until it lapsed unused; confirm it is gone too.
  const thirdGone = await answer(await post(paid("evt_4", third)));
  assert.equal(thirdGone.status, 403);
  const rows = await owner.call<any>("database_execute", { deploymentId: app.deploymentId, operation: "sql", input: { sql: "SELECT status, count(*) AS n FROM orders GROUP BY status ORDER BY status" } });
  kit.pass("Holds that lapse", `The paid hold stays sold. 1.7 s later the two unpaid holds no longer count, items.list shows 2 left, confirming a lapsed hold answers ${lapsed.status} "${lapsed.body.error}", and another buyer holds the 2. The lapsed rows stay in the table as 'held' (${JSON.stringify(rows.rows ?? rows)}) because no job runs to clear them.`);

  // What the page may load from a payment provider. Navigation to a hosted
  // checkout page is allowed; embedding the provider is not.
  const page = await kit.appFetch(app.host)(`${url}/`);
  const policy = page.headers.get("content-security-policy") ?? "";
  for (const directive of ["default-src 'self'", "connect-src 'self'", "form-action 'self'", "frame-ancestors 'none'"]) assert(policy.includes(directive), `${directive} missing from ${policy}`);
  assert.match(policy, /script-src 'self'( 'sha256-[^']+')*;/);
  assert(!/frame-src/.test(policy));
  assert(policy.includes("default-src 'self'"));
  const html = await page.text();
  assert(html.includes("https://pay.example/link/abc"));
  assert(html.includes(`session` ) || html.includes("Pay"));
  kit.pass("Payment provider in the page", `The page's policy is "${policy}". A provider's script, its API calls, an embedded checkout frame (frame-src falls back to default-src 'self') and a form that posts to it do not run; a plain link to a hosted payment page publishes and navigates. Checkout Session creation stays a call action with the secret in the Kernel.`);
});
