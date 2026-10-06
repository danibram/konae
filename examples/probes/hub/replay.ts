// Probe: an integration hub that receives webhooks from services the owner
// does not control and exposes an API for a program. Decides the matrix cells
// for signed webhooks in, payments (a payment provider's webhook is one of
// these senders) and the hub's program API.
//
// Each sender signs here with node:crypto, the way its own documentation
// says, not with Littlebox's signer, so the probe checks the Kernel against
// the provider and not against itself.
//
//   LITTLEBOX_PROBE_PORT=23010 bun run examples/probes/hub/replay.ts
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { createHmac } from "node:crypto";
import { probe } from "../_kit";

const here = import.meta.dir;
const secrets = {
  push_hook: "push-hook-secret-0123456789",
  payment_hook: "whsec_payment_hook_0123456789",
  email_hook: `whsec_${Buffer.from("email-hook-key-0123456789").toString("base64")}`,
};

await probe("hub", 23010, async (kit) => {
  const { url, owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_hub", visibility: "private", signup: { mode: "closed" }, secrets });
  const post = (route: string, body: string, headers: Record<string, string> = {}) =>
    kit.appFetch(app.host)(url + route, { method: "POST", headers: { "content-type": "application/json", ...headers }, body });
  const answer = async (response: Response) => ({ status: response.status, body: await response.json() as any });
  const seconds = () => Math.floor(Date.now() / 1000);

  // A push as a code host sends it: nested objects, keys no input names, and an HMAC of the body alone in "sha256=<hex>".
  const github = (body: string, secret = secrets.push_hook) => ({ "X-Hub-Signature-256": `sha256=${createHmac("sha256", secret).update(body).digest("hex")}`, "X-GitHub-Delivery": crypto.randomUUID() });
  const pushBody = JSON.stringify({ ref: "refs/heads/main", after: "9f2c1e", repository: { full_name: "acme/site", private: true }, pusher: { name: "ana", email: "ana@example.com" }, commits: [{ id: "9f2c1e" }] });
  const push = await answer(await post("/hooks/push", pushBody, github(pushBody)));
  assert.deepEqual(push, { status: 200, body: { result: { changes: 1, lastInsertId: 1 } } });
  const replayed = await answer(await post("/hooks/push", pushBody, github(pushBody)));
  assert.deepEqual(replayed, { status: 409, body: { error: "replayed-signature" } });
  const wrongSecret = await answer(await post("/hooks/push", pushBody.replace("9f2c1e", "9f2c1f"), github(pushBody.replace("9f2c1e", "9f2c1f"), "another-secret-0123456789")));
  assert.deepEqual(wrongSecret, { status: 401, body: { error: "invalid-signature" } });
  const tampered = await answer(await post("/hooks/push", pushBody.replace("acme/site", "acme/evil"), github(pushBody)));
  assert.deepEqual(tampered, { status: 401, body: { error: "invalid-signature" } });
  kit.pass("Provider signature", `A push signed the code host's way, "sha256=<hex>" over the body, on a route that declares "scheme": "hmac-body", answers 200 { changes: 1 }. The same delivery again answers ${replayed.status} ${JSON.stringify(replayed.body)}, even with a new X-GitHub-Delivery, because the MAC is what is remembered; a wrong secret and a changed byte answer 401 "invalid-signature".`);

  // A payment provider's scheme: "t=<unix>,v1=<hex>" over "<t>.<body>". A shop's program creates the order first, with a key.
  const shopKey = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "shop", actions: ["orders.create", "orders.list"], files: [] });
  const keyed = { authorization: `Bearer ${shopKey.key}` };
  assert.equal((await post("/api/orders", JSON.stringify({ id: "ord_1", item: "Print, A3" }), keyed)).status, 200);
  const stripe = (body: string, t = seconds()) => ({ "Stripe-Signature": `t=${t},v1=${createHmac("sha256", secrets.payment_hook).update(`${t}.${body}`).digest("hex")}` });
  const paid = (event: string) => JSON.stringify({ id: event, type: "checkout.session.completed", livemode: false, data: { object: { id: `cs_${event}`, client_reference_id: "ord_1", amount_total: 2400, currency: "eur" } } });
  const stale = await answer(await post("/hooks/payment", paid("evt_1"), stripe(paid("evt_1"), seconds() - 400)));
  assert.deepEqual(stale, { status: 401, body: { error: "stale-timestamp" } });
  const signed = stripe(paid("evt_1"));
  const payment = await answer(await post("/hooks/payment", paid("evt_1"), signed));
  assert.deepEqual(payment, { status: 200, body: { result: { changes: 1, lastInsertId: 0 } } });
  const again = await answer(await post("/hooks/payment", paid("evt_1"), signed));
  assert.deepEqual(again, { status: 409, body: { error: "replayed-signature" } });
  const second = await answer(await post("/hooks/payment", paid("evt_2"), stripe(paid("evt_2"))));
  assert.deepEqual(second, { status: 403, body: { error: "This order is unknown or already paid." } });
  const orders = (await answer(await kit.appFetch(app.host)(`${url}/api/orders`, { headers: keyed }))).body.result;
  assert.deepEqual(orders.map((order: any) => [order.id, order.payment_id, order.paid]), [["ord_1", "cs_evt_1", 1]]);
  kit.pass("Payment provider signature", `A checkout event signed "t=...,v1=..." over "<t>.<body>", on a route that declares "scheme": "timestamped" and reads the order from $.data.object.client_reference_id, marks the order paid through a guarded mutation. A t 400 seconds old answers ${stale.status} ${JSON.stringify(stale.body)}, the same header again ${again.status} ${JSON.stringify(again.body)}, and a second payment for the order ${second.status} ${JSON.stringify(second.body)}.`);

  // A mail provider that sends through Svix: three headers, the id inside the MAC.
  const svixKey = Buffer.from(secrets.email_hook.slice("whsec_".length), "base64");
  const svix = (id: string, body: string, t = seconds()) => ({
    "svix-id": id, "svix-timestamp": String(t), "svix-signature": `v1,${createHmac("sha256", svixKey).update(`${id}.${t}.${body}`).digest("base64")}`,
  });
  const bounce = JSON.stringify({ type: "email.bounced", created_at: "2026-11-07T10:00:00Z", data: { email_id: "4ef9a417", to: ["ana@example.com"] } });
  const email = await answer(await post("/hooks/email", bounce, svix("msg_1", bounce)));
  assert.equal(email.status, 200);
  const retried = await answer(await post("/hooks/email", bounce, svix("msg_1", bounce, seconds() + 1)));
  assert.deepEqual(retried, { status: 409, body: { error: "replayed-signature" } });
  kit.pass("Svix sender", `A Resend-style event with svix-id, svix-timestamp and svix-signature on a route that declares "scheme": "svix" answers 200. The same svix-id signed again a second later answers ${retried.status} ${JSON.stringify(retried.body)}.`);

  // Keys no input names: the provider routes leave them out, a route without fields still refuses them.
  const deployKey = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "deploy bot", actions: ["deploys.receive"], files: [] });
  const strict = await answer(await post("/hooks/deploy", JSON.stringify({ ref: "main", detail: null, pusher: "ana" }), { authorization: `Bearer ${deployKey.key}` }));
  assert.deepEqual(strict, { status: 400, body: { error: "Unexpected action input: pusher" } });
  kit.pass("Undeclared fields", `The push carried after, private, email and commits, which no input names, and the route that declares fields stored it. A route without fields still answers ${strict.status} ${JSON.stringify(strict.body)}.`);

  // Both doors refuse a scheme they do not know, with one sentence.
  const actions = JSON.parse(await readFile(path.join(here, "actions.json"), "utf8"));
  actions.actions["pushes.receive"].http.signature.scheme = "github";
  const sources = { "actions.json": JSON.stringify(actions), "schema.sql": await readFile(path.join(here, "schema.sql"), "utf8") };
  const sentence = 'Action pushes.receive names an unknown signature scheme. Use "littlebox", "hmac-body", "timestamped" or "svix".';
  assert.equal(await kit.planRefusal(here, "probe_hub_scheme", sources), sentence);
  assert.equal((await kit.gateRefusal(sources)).detail, sentence);
  kit.pass("Unknown scheme at both doors", `release_plan and the Validation Gate refuse "scheme": "github" with ${JSON.stringify(sentence)}.`);

  // A form-encoded body, as chat and telephony services post.
  const form = await kit.appFetch(app.host)(`${url}/hooks/deploy`, {
    method: "POST", headers: { "content-type": "application/x-www-form-urlencoded", authorization: `Bearer ${deployKey.key}` }, body: "ref=main&detail=ok",
  });
  const formAnswer = await answer(form);
  assert.equal(formAnswer.status, 400);
  kit.pass("Form-encoded body", `POST /hooks/deploy with application/x-www-form-urlencoded and a valid key answers ${formAnswer.status} ${JSON.stringify(formAnswer.body)}.`);

  // A sender that cannot set Authorization: the private app's unsigned route needs a key or a session.
  const keyless = await answer(await post("/hooks/deploy", JSON.stringify({ ref: "main", detail: null })));
  assert.equal(keyless.status, 401);
  assert.deepEqual(keyless.body, { error: "authentication-required" });
  const keyInQuery = await answer(await post(`/hooks/deploy?key=${deployKey.key}`, JSON.stringify({ ref: "main", detail: null })));
  assert.notEqual(keyInQuery.status, 200);
  const deployed = await answer(await post("/hooks/deploy", JSON.stringify({ ref: "main", detail: JSON.stringify({ build: 41, ok: true }) }), { authorization: `Bearer ${deployKey.key}` }));
  assert.equal(deployed.status, 200);
  kit.pass("Key only in a header", `Without a key the route answers ${keyless.status} ${JSON.stringify(keyless.body)}; the key in the query string answers ${keyInQuery.status} ${JSON.stringify(keyInQuery.body)}; in Authorization: Bearer it answers 200.`);

  // A program reads what arrived, with a key that names the feed. The push's detail came from two nested paths.
  const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "dashboard", actions: ["events.feed"], files: [] });
  const feed = await kit.appFetch(app.host)(`${url}/api/events`, { headers: { authorization: `Bearer ${reader.key}` } });
  const rows = (await feed.json() as any).result;
  assert.deepEqual(rows.map((row: any) => [row.source, row.ref, row.detail]), [
    ["push", "refs/heads/main", "acme/site by ana"], ["email", "email.bounced", "4ef9a417"], ["deploy", "main", JSON.stringify({ build: 41, ok: true })],
  ]);
  kit.pass("Program API", `GET /api/events with a key that names events.feed returns the push, with "acme/site by ana" read from $.repository.full_name and $.pusher.name, the bounce and the deploy.`);

  // On a public app the unsigned route takes anyone, so the hub cannot tell the provider from a stranger.
  // One owner's two projects cannot share a secret value, so the public copy has its own.
  const publicSecrets = { push_hook: `public-${secrets.push_hook}`, payment_hook: `${secrets.payment_hook}_public`, email_hook: `whsec_${Buffer.from("public-email-key-0123456789").toString("base64")}` };
  const open = await kit.publish(here, { projectId: "probe_hub_public", visibility: "public", signup: { mode: "closed" }, secrets: publicSecrets });
  const spoofed = await kit.appFetch(open.host)(`${url}/hooks/deploy`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ ref: "main", detail: "from anyone" }) });
  assert.equal(spoofed.status, 200);
  kit.pass("Unsigned route on a public app", "POST /hooks/deploy with no signature, key or session answers 200 and stores the row: a sender that cannot sign in one of the four schemes or send a key lands only where anyone can post.");
});
