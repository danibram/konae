// A hostile program on the internet and a hostile app user against the
// showcase apps' HTTP routes, API keys and the file read path. Each test is
// one attempt from the security review.
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { cp } from "node:fs/promises";
import path from "node:path";
import { buildLocal } from "../scripts/littlebox";
import { signRoute } from "../src/lib/route-signature";
import { release } from "./migration-kernel";
import { answerText, appUser, freshClient, hostFetch, rawCall, runtimeClient, startLab, testPort, type Lab } from "./security-review-kernel";

let lab: Lab;
beforeAll(async () => { lab = await startLab(testPort("LITTLEBOX_REVIEW_ROUTES_PORT", 23800)); }, 180_000);
afterAll(async () => { await lab?.stop(); });

const day = () => new Date().toISOString().slice(0, 10);
const checkin = (body: string, headers: Record<string, string> = {}, route = "/api/checkin") =>
  hostFetch(lab, lab.apps.booking.hostname, route, { method: "POST", headers: { "content-type": "application/json", ...headers }, body });
const panelSign = (body: string, extra: { path?: string; query?: string; at?: number; nonce?: string } = {}) =>
  signRoute(lab.secrets.panel_hook, { method: "POST", path: extra.path ?? "/api/checkin", query: extra.query, body }, extra.at, extra.nonce);

describe("route signatures", () => {
  test("path, case, trailing slash, encoding and query variants do not reach the signed route with another request's signature", async () => {
    const body = JSON.stringify({ room: "none", day: day(), hour: 9 });
    const signature = await panelSign(body);
    for (const route of ["/api/checkin/", "/API/checkin", "/api/%63heckin", "/api/checkin;x"]) {
      expect((await checkin(body, { "x-panel-signature": signature }, route)).status).toBe(405);
    }
    // The query string is signed, so adding one breaks the signature.
    expect((await checkin(body, { "x-panel-signature": signature }, "/api/checkin?room=other")).status).toBe(401);
    // The signed path is the route's own, so a signature for another path fails on this one.
    const forOther = await panelSign(body, { path: "/api/other" });
    expect(await (await checkin(body, { "x-panel-signature": forOther })).json()).toEqual({ error: "invalid-signature" });
    expect((await checkin(body, { "x-panel-signature": signature })).status).toBe(200);
  });

  test("the MAC covers the raw bytes: re-encoded JSON, duplicate keys and a method override header are refused", async () => {
    const body = JSON.stringify({ room: "none", day: day(), hour: 9 });
    const signature = await panelSign(body);
    // The same JSON value written with other whitespace or key order is other bytes.
    for (const other of [JSON.stringify({ day: day(), room: "none", hour: 9 }), JSON.stringify(JSON.parse(body), null, 1), `${body} `]) {
      expect(await (await checkin(other, { "x-panel-signature": signature })).json()).toEqual({ error: "invalid-signature" });
    }
    // A duplicate key is part of the signed bytes; changing it breaks the MAC.
    const duplicate = `{"room":"none","day":"${day()}","hour":9,"hour":10}`;
    const signedDuplicate = await panelSign(duplicate);
    expect((await checkin(duplicate.replace('"hour":10', '"hour":11'), { "x-panel-signature": signedDuplicate })).status).toBe(401);
    // No method override is read: a GET with X-HTTP-Method-Override is a GET, which the POST route does not answer.
    const overridden = await hostFetch(lab, lab.apps.booking.hostname, "/api/checkin", { headers: { "x-http-method-override": "POST", "x-panel-signature": signature } });
    expect(overridden.status).not.toBe(200);
    expect((await checkin(body, { "x-panel-signature": signature })).status).toBe(200);
  });

  test("a chunked body is verified over the bytes the Kernel received", async () => {
    const body = JSON.stringify({ room: "none", day: day(), hour: 10 });
    const signature = await panelSign(body);
    const chunked = (text: string) => new ReadableStream({ start(controller) {
      for (let at = 0; at < text.length; at += 7) controller.enqueue(new TextEncoder().encode(text.slice(at, at + 7)));
      controller.close();
    } });
    const send = (text: string) => fetch(`${lab.kernel.url}/api/checkin`, {
      method: "POST", body: chunked(text), headers: { host: lab.apps.booking.hostname, "x-forwarded-for": freshClient(), "content-type": "application/json", "x-panel-signature": signature },
    });
    expect((await send(body.replace("none", "nonf"))).status).toBe(401);
    expect((await send(body)).status).toBe(200);
  });

  test("a header with thousands of v1 values costs no MAC each, and three 32-byte values are refused", async () => {
    const body = JSON.stringify({ room: "none", day: day(), hour: 11 });
    const signature = await panelSign(body);
    const crowded = signature.replace("v1=", `${"v1=,v1=00,".repeat(3000)}v1=`);
    const started = performance.now();
    expect((await checkin(body, { "x-panel-signature": crowded })).status).toBe(200);
    expect(performance.now() - started).toBeLessThan(2000);
    const fresh = await panelSign(body);
    const three = fresh.replace("v1=", `v1=${"ab".repeat(32)},v1=${"cd".repeat(32)},v1=`);
    expect(await (await checkin(body, { "x-panel-signature": three })).json()).toEqual({ error: "invalid-signature" });
    // Two values, as during a rotation, still verify.
    const two = fresh.replace("v1=", `v1=${"ab".repeat(32)},v1=`);
    expect((await checkin(body, { "x-panel-signature": two })).status).toBe(200);
  });

  test("a signature captured on one Deployment cannot be replayed on another of the same owner", async () => {
    // The replay worked while two projects could hold one value: the owner can no longer set it.
    const copy = path.join(lab.root, "booking-copy");
    await cp(path.join(lab.root, "booking"), copy, { recursive: true });
    const compiled = await buildLocal(copy);
    const project = { id: "review_booking_copy", name: "Booking copy", slug: "booking-copy", manifest: { version: 1, visibility: "public" } };
    await lab.kernel.owner.call("project_create", { projectId: project.id, name: project.name, slug: project.slug, manifest: project.manifest });
    await expect(lab.kernel.owner.call("secret_set", { projectId: project.id, name: "panel_hook", value: lab.secrets.panel_hook }))
      .rejects.toThrow("Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.");
    const own = `copy_${lab.secrets.panel_hook}`;
    await lab.kernel.owner.call("secret_set", { projectId: project.id, name: "panel_hook", value: own });
    const { receipt } = await release(lab.kernel.owner, { project: { ...project, files: compiled.files }, visibility: "public", artifact: compiled.artifact }, project.id);
    const body = JSON.stringify({ room: "none", day: day(), hour: 12 });
    const captured = await panelSign(body);
    expect((await checkin(body, { "x-panel-signature": captured })).status).toBe(200);
    const replayed = await hostFetch(lab, receipt.hostname, "/api/checkin", { method: "POST", headers: { "content-type": "application/json", "x-panel-signature": captured }, body });
    expect(await replayed.json()).toEqual({ error: "invalid-signature" });
  });
});

describe("route-only actions and API keys", () => {
  test("a signed or key-only action is unreachable through actions_invoke, with or without a session", async () => {
    const { booking, inbox } = lab.apps;
    const visitor = await rawCall(lab, "/_littlebox/mcp", booking.hostname, "actions_invoke", { name: "panel.checkin", input: { room: "x", day: day(), hour: 9 } });
    expect(answerText(visitor.body)).toBe("Action panel.checkin answers only on its HTTP route, POST /api/checkin.");
    const member = await appUser(lab, inbox);
    for (const name of ["events.receive", "events.feed"]) {
      const answer = await rawCall(lab, "/_littlebox/mcp", inbox.hostname, "actions_invoke", { name, input: {} }, { cookie: member.cookie });
      expect(answerText(answer.body)).toContain(`Action ${name} answers only on its HTTP route`);
    }
    // A session does not replace the key the feed asks for.
    expect((await hostFetch(lab, inbox.hostname, "/api/events", { headers: { cookie: member.cookie } })).status).toBe(401);
  });

  test("a key reaches only the actions and files it names, only on its own Deployment, and never the runtime endpoint", async () => {
    const { inbox, habits, gallery } = lab.apps;
    const feed = await lab.kernel.owner.call<{ key: string }>("api_keys_create", { deploymentId: inbox.id, name: "Feed", actions: ["events.feed"] });
    const bearer = { authorization: `Bearer ${feed.key}` };
    expect((await hostFetch(lab, inbox.hostname, "/api/events", { headers: bearer })).status).toBe(200);
    expect(await (await hostFetch(lab, inbox.hostname, "/hooks/events", { method: "POST", headers: bearer, body: "{}" })).json())
      .toEqual({ error: "This API key may not call events.receive." });
    expect((await hostFetch(lab, inbox.hostname, `/_littlebox/files/${crypto.randomUUID()}`, { headers: bearer })).status).toBe(404);
    for (const app of [habits, gallery]) {
      expect(await (await hostFetch(lab, app.hostname, app === habits ? "/api/habits" : "/feed.json", { headers: bearer })).json()).toEqual({ error: "invalid-api-key" });
    }
    const runtime = await rawCall(lab, "/_littlebox/mcp", inbox.hostname, "actions_invoke", { name: "events.list", input: {} }, bearer);
    expect(runtime.status).toBe(403);
    expect(await lab.kernel.owner.call("api_keys_list", { deploymentId: inbox.id }).then(JSON.stringify)).not.toContain(feed.key);
    // An owner credential is not a key either.
    expect((await hostFetch(lab, inbox.hostname, "/api/events", { headers: { authorization: `Bearer ${lab.kernel.ownerCookie.split("=")[1]}` } })).status).toBe(401);
  });

  test("guessing keys from one address stops at the bucket, even with a valid key afterwards", async () => {
    const { habits } = lab.apps;
    const real = await lab.kernel.owner.call<{ key: string }>("api_keys_create", { deploymentId: habits.id, name: "Import", actions: ["habits.mine"] });
    const client = freshClient();
    const statuses: number[] = [];
    for (let attempt = 0; attempt < 12; attempt++) {
      statuses.push((await hostFetch(lab, habits.hostname, "/api/habits", { client, headers: { authorization: `Bearer lbk_${crypto.randomUUID().replaceAll("-", "")}` } })).status);
    }
    expect(statuses.slice(0, 10).every((status) => status === 401)).toBe(true);
    expect(statuses.slice(10)).toEqual([429, 429]);
    // The bucket answers before any lookup, so a real key from that address waits too.
    expect((await hostFetch(lab, habits.hostname, "/api/habits", { client, headers: { authorization: `Bearer ${real.key}` } })).status).toBe(429);
    expect((await hostFetch(lab, habits.hostname, "/api/habits", { headers: { authorization: `Bearer ${real.key}` } })).status).toBe(200);
  });
});

describe("the file read path", () => {
  test("traversal and foreign IDs never leave the Deployment's file index", async () => {
    const { club, gallery } = lab.apps;
    const member = await appUser(lab, club);
    const other = await appUser(lab, gallery);
    const client = runtimeClient(lab, gallery.hostname, other.cookie);
    const transfer = await client.upload(new TextEncoder().encode("gallery file"));
    const galleryFile = await client.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "g.txt", type: "text/plain", visibility: "public" });
    for (const id of [
      "..%2F..%2Fdatabase.sqlite", "..%2Fidentity.sqlite", "%2e%2e%2f%2e%2e%2fmcp.sqlite", "index.json", "%2Fetc%2Fpasswd",
      "..%5C..%5Cdatabase.sqlite", `${galleryFile.id}.blob`, `${galleryFile.id}%00`, galleryFile.id,
    ]) {
      const response = await hostFetch(lab, club.hostname, `/_littlebox/files/${id}`, { headers: { cookie: member.cookie } });
      expect(response.status, id).toBe(404);
      await response.arrayBuffer();
    }
    for (const fileId of ["../../database.sqlite", galleryFile.id, `${galleryFile.id}/../x`]) {
      await expect(runtimeClient(lab, club.hostname, member.cookie).call("files_read", { fileId })).rejects.toThrow("File not found.");
    }
  });

  test("an upload typed as JavaScript, HTML or SVG is served as bytes, so a page cannot load it as a script", async () => {
    const { gallery } = lab.apps;
    const member = await appUser(lab, gallery);
    const client = runtimeClient(lab, gallery.hostname, member.cookie);
    for (const type of ["text/javascript", "application/javascript", "text/html", "image/svg+xml", "application/xhtml+xml"]) {
      const transfer = await client.upload(new TextEncoder().encode("alert(document.domain)"));
      const file = await client.call<{ id: string; type: string }>("files_upload", { transferId: transfer.transferId, name: "x", type, visibility: "public" });
      await client.cancel(transfer.transferId);
      expect(file.type).toBe(type);
      const response = await hostFetch(lab, gallery.hostname, `/_littlebox/files/${file.id}`);
      expect(response.headers.get("content-type"), type).toBe("application/octet-stream");
      expect(response.headers.get("x-content-type-options")).toBe("nosniff");
      expect(response.headers.get("content-disposition")).toBe("attachment");
      await response.arrayBuffer();
    }
    const transfer = await client.upload(new Uint8Array([137, 80, 78, 71]));
    const image = await client.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "p.png", type: "image/png", visibility: "public" });
    expect((await hostFetch(lab, gallery.hostname, `/_littlebox/files/${image.id}`)).headers.get("content-type")).toBe("image/png");
  });

  test("a transfer of another user, or of another app, cannot become my upload, and control characters leave the name", async () => {
    const { gallery, club } = lab.apps;
    const ana = runtimeClient(lab, gallery.hostname, (await appUser(lab, gallery)).cookie);
    const ben = runtimeClient(lab, gallery.hostname, (await appUser(lab, gallery)).cookie);
    const clubUser = runtimeClient(lab, club.hostname, (await appUser(lab, club)).cookie);
    const transfer = await ana.upload(new TextEncoder().encode("ana's bytes"));
    await expect(ben.call("files_upload", { transferId: transfer.transferId, name: "stolen" })).rejects.toThrow("Transfer not found or expired.");
    await expect(clubUser.call("files_upload", { transferId: transfer.transferId, name: "stolen" })).rejects.toThrow("Transfer not found or expired.");
    await expect(ben.call("files_upload", { transferId: "../../mcp.sqlite", name: "x" })).rejects.toThrow("Invalid transfer ID.");
    const named = await ana.call<{ name: string }>("files_upload", { transferId: transfer.transferId, name: `\u0001\u0007ok\r\n${"\u0002".repeat(300)}` });
    expect(named.name).toBe("ok");
  });
});

describe("quotas a visitor can reach", () => {
  test("visitors reading public files cannot hold the transfers a member uploads with", async () => {
    const { gallery } = lab.apps;
    const member = runtimeClient(lab, gallery.hostname, (await appUser(lab, gallery)).cookie);
    const transfer = await member.upload(new TextEncoder().encode("public"));
    const file = await member.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "p.txt", type: "text/plain", visibility: "public" });
    await member.cancel(transfer.transferId);
    // Eight addresses read twice each and never cancel: the visitors' pool of 16 is full.
    for (let address = 0; address < 8; address++) {
      const visitor = runtimeClient(lab, gallery.hostname, undefined, freshClient());
      for (let read = 0; read < 2; read++) await visitor.call("files_read", { fileId: file.id });
    }
    await expect(runtimeClient(lab, gallery.hostname, undefined, freshClient()).call("files_read", { fileId: file.id })).rejects.toThrow("Too many active transfers");
    const next = await member.upload(new TextEncoder().encode("still uploads"));
    expect((await member.call<{ id: string }>("files_upload", { transferId: next.transferId, name: "n.txt" })).id).toBeTruthy();
  });

  test("a visitor's actions are rate limited per address", async () => {
    const { booking } = lab.apps;
    const client = freshClient();
    const answers: string[] = [];
    for (let call = 0; call < 32; call++) answers.push(answerText((await rawCall(lab, "/_littlebox/mcp", booking.hostname, "actions_invoke", { name: "rooms.list", input: {} }, { "x-forwarded-for": client })).body));
    expect(answers.slice(0, 30).every((text) => text !== "rate-limited")).toBe(true);
    expect(answers.slice(30)).toEqual(["rate-limited", "rate-limited"]);
  });
});
