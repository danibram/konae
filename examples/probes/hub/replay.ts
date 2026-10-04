// Probe: an integration hub that receives webhooks from services the owner
// does not control and exposes an API for a program. Decides the matrix cells
// for signed webhooks in, payments (a payment provider's webhook is one of
// these senders) and the hub's program API.
//
//   LITTLEBOX_PROBE_PORT=23010 bun run examples/probes/hub/replay.ts
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { signRoute } from "../../../src/lib/route-signature";
import { probe } from "../_kit";

const here = import.meta.dir;
const secret = "push-hook-secret-0123456789";

await probe("hub", 23010, async (kit) => {
  const { url, owner } = kit;
  const app = await kit.publish(here, { projectId: "probe_hub", visibility: "private", signup: { mode: "closed" }, secrets: { push_hook: secret } });
  const post = (route: string, body: string, headers: Record<string, string> = {}) =>
    kit.appFetch(app.host)(url + route, { method: "POST", headers: { "content-type": "application/json", ...headers }, body });
  const answer = async (response: Response) => ({ status: response.status, body: await response.json() as any });

  // A push event as a code host sends it: nested objects and fields the action does not declare,
  // signed as the sender signs, an HMAC of the body alone in "sha256=<hex>".
  const providerBody = JSON.stringify({ ref: "refs/heads/main", after: "9f2c1e", repository: { full_name: "acme/site" }, pusher: { name: "ana" } });
  const providerSignature = `sha256=${createHmac("sha256", secret).update(providerBody).digest("hex")}`;
  const provider = await answer(await post("/hooks/push", providerBody, { "X-Hub-Signature-256": providerSignature }));
  assert.equal(provider.status, 401);
  assert.deepEqual(provider.body, { error: "invalid-signature" });
  kit.pass("Provider signature", `A push signed the provider's way (an HMAC of the body, "sha256=<hex>", with the same secret) answers ${provider.status} ${JSON.stringify(provider.body)}: the route checks only Littlebox's scheme.`);

  // A payment provider's scheme, "t=<unix>,v1=<hex>" over "<t>.<body>", is refused the same way.
  const t = Math.floor(Date.now() / 1000);
  const paymentSignature = `t=${t},v1=${createHmac("sha256", secret).update(`${t}.${providerBody}`).digest("hex")}`;
  const payment = await answer(await post("/hooks/push", providerBody, { "X-Hub-Signature-256": paymentSignature }));
  assert.equal(payment.status, 401);
  assert.deepEqual(payment.body, { error: "invalid-signature" });
  kit.pass("Payment provider signature", `A "t=...,v1=..." header over "<t>.<body>" answers ${payment.status} ${JSON.stringify(payment.body)}.`);

  // The same body signed Littlebox's way passes the signature and is refused for its shape.
  const littleboxSigned = (body: string) => signRoute(secret, { method: "POST", path: "/hooks/push", body });
  const extra = await answer(await post("/hooks/push", providerBody, { "X-Hub-Signature-256": await littleboxSigned(providerBody) }));
  assert.equal(extra.status, 400);
  kit.pass("Undeclared fields", `The provider's body, correctly signed, answers ${extra.status} ${JSON.stringify(extra.body)}: an action takes only the inputs it declares.`);
  const nestedBody = JSON.stringify({ ref: { name: "main" }, after: "9f2c1e" });
  const nested = await answer(await post("/hooks/push", nestedBody, { "X-Hub-Signature-256": await littleboxSigned(nestedBody) }));
  assert.equal(nested.status, 400);
  kit.pass("Nested value", `An object where a string input is declared answers ${nested.status} ${JSON.stringify(nested.body)}: inputs are strings, numbers and booleans.`);
  const flatBody = JSON.stringify({ ref: "refs/heads/main", after: "9f2c1e" });
  const flat = await answer(await post("/hooks/push", flatBody, { "X-Hub-Signature-256": await littleboxSigned(flatBody) }));
  assert.equal(flat.status, 200);
  assert.equal(flat.body.result.changes, 1);
  kit.pass("Littlebox-shaped webhook", "A body of exactly the declared inputs, signed Littlebox's way, answers 200 { changes: 1 }.");

  // A sender that cannot set Authorization: the private app's unsigned route needs a key or a session.
  const deployKey = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "deploy bot", actions: ["deploys.receive"], files: [] });
  // A form-encoded body, as chat and telephony services post.
  const form = await kit.appFetch(app.host)(`${url}/hooks/deploy`, {
    method: "POST", headers: { "content-type": "application/x-www-form-urlencoded", authorization: `Bearer ${deployKey.key}` }, body: "ref=main&detail=ok",
  });
  const formAnswer = await answer(form);
  assert.equal(formAnswer.status, 400);
  kit.pass("Form-encoded body", `POST /hooks/deploy with application/x-www-form-urlencoded and a valid key answers ${formAnswer.status} ${JSON.stringify(formAnswer.body)}.`);

  const keyless = await answer(await post("/hooks/deploy", JSON.stringify({ ref: "main", detail: null })));
  assert.equal(keyless.status, 401);
  assert.deepEqual(keyless.body, { error: "authentication-required" });
  const keyInQuery = await answer(await post(`/hooks/deploy?key=${deployKey.key}`, JSON.stringify({ ref: "main", detail: null })));
  assert.notEqual(keyInQuery.status, 200);
  const keyed = await answer(await post("/hooks/deploy", JSON.stringify({ ref: "main", detail: JSON.stringify({ build: 41, ok: true }) }), { authorization: `Bearer ${deployKey.key}` }));
  assert.equal(keyed.status, 200);
  kit.pass("Key only in a header", `Without a key the route answers ${keyless.status} ${JSON.stringify(keyless.body)}; the key in the query string answers ${keyInQuery.status} ${JSON.stringify(keyInQuery.body)}; in Authorization: Bearer it answers 200. A sender that posts nested JSON can pass it only as a string input it encodes itself.`);

  // A program reads what arrived, with a key that names the feed.
  const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId: app.deploymentId, name: "dashboard", actions: ["events.feed"], files: [] });
  const feed = await kit.appFetch(app.host)(`${url}/api/events`, { headers: { authorization: `Bearer ${reader.key}` } });
  const rows = (await feed.json() as any).result;
  assert.deepEqual(rows.map((row: any) => row.source), ["push", "deploy"]);
  kit.pass("Program API", "GET /api/events with a key that names events.feed returns the push and the deploy that were stored.");

  // The public copy holds its own secret: one owner's two projects cannot share a value.
  // On a public app the unsigned route takes anyone, so the hub cannot tell the provider from a stranger.
  const open = await kit.publish(here, { projectId: "probe_hub_public", visibility: "public", signup: { mode: "closed" }, secrets: { push_hook: `public-${secret}` } });
  const spoofed = await kit.appFetch(open.host)(`${url}/hooks/deploy`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ ref: "main", detail: "from anyone" }) });
  assert.equal(spoofed.status, 200);
  kit.pass("Unsigned route on a public app", "POST /hooks/deploy with no signature, key or session answers 200 and stores the row: a sender that cannot sign Littlebox's way or send a key lands only where anyone can post.");
});
