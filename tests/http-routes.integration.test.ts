import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";
import { signRoute } from "../src/lib/route-signature";

// The HTTP module against a real Kernel: routes on a private Deployment and
// the callers each one takes, signatures, API keys, the file read path, and
// keys revoked by an import.

setDefaultTimeout(60_000);

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let owner: McpClient;
let host: string;
let deploymentId: string;
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
const secret = "orders-webhook-secret-0123456789";
let address = 0;
const freshAddress = () => `198.18.1.${++address}`;

const actions = {
  version: 1,
  actions: {
    "orders.receive": {
      kind: "mutation", access: "anonymous",
      statement: "INSERT INTO orders (external_id, total) VALUES (?1, ?2)",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 64 }, { source: "input", key: "total", type: "number" }],
      http: { method: "POST", path: "/hooks/orders", signature: { header: "X-Signature", secret: "orders_webhook" } },
    },
    "orders.list": {
      kind: "query", access: "anonymous",
      statement: "SELECT external_id, total FROM orders WHERE (?1 IS NULL OR total >= ?1) ORDER BY id LIMIT 50",
      bindings: [{ source: "input", key: "min", type: "number", nullable: true }],
      http: { method: "GET", path: "/api/orders" },
    },
    "orders.void": {
      kind: "mutation", access: "anonymous",
      statement: "DELETE FROM orders WHERE external_id = ?1",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 64 }],
      http: { method: "POST", path: "/hooks/void", signature: { header: "X-Signature", secret: "orders_webhook" }, callers: ["signature"] },
    },
    "notes.add": {
      kind: "mutation", access: "anonymous",
      statement: "INSERT INTO notes (owner_id, body) VALUES (?1, ?2)",
      bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }],
      http: { method: "POST", path: "/api/notes" },
    },
    "notes.edit": {
      kind: "mutation", access: "anonymous", guard: "That note is not yours, or it does not exist.",
      statement: "UPDATE notes SET body = ?2 WHERE id = ?1 AND owner_id = ?3",
      bindings: [{ source: "input", key: "id", type: "number" }, { source: "input", key: "body", type: "string", maxLength: 200 }, { source: "actor", field: "id" }],
      http: { method: "POST", path: "/api/notes/edit" },
      signal: { name: "note.edited", properties: ["id", "actor"] },
    },
    "notes.rename": {
      kind: "mutation", access: "authenticated", guard: "That note is not yours, or it does not exist.",
      statement: "UPDATE notes SET body = ?2 WHERE id = ?1 AND owner_id = ?3",
      bindings: [{ source: "input", key: "id", type: "number" }, { source: "input", key: "body", type: "string", maxLength: 200 }, { source: "actor", field: "id" }],
      signal: { name: "note.renamed", properties: ["id", "actor"] },
    },
    "notes.touch": {
      kind: "mutation", access: "authenticated",
      statement: "UPDATE notes SET body = body WHERE id = ?1 AND owner_id = ?2",
      bindings: [{ source: "input", key: "id", type: "number" }, { source: "actor", field: "id" }],
      signal: { name: "note.touched", properties: ["actor"] },
    },
    "notes.mine": {
      kind: "query", access: "authenticated",
      statement: "SELECT body FROM notes WHERE owner_id = ?1 LIMIT 50",
      bindings: [{ source: "actor", field: "id" }],
    },
  },
};
const schema = "CREATE TABLE IF NOT EXISTS orders(id INTEGER PRIMARY KEY, external_id TEXT NOT NULL, total REAL NOT NULL);\nCREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, body TEXT NOT NULL);";

function release(id: string, files: Record<string, string>, initial: object = {}) {
  return (async () => {
    const project = { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "private" } };
    const transfer = await owner.upload(new TextEncoder().encode(JSON.stringify({
      project: { ...project, files }, visibility: "private", ...initial,
      artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Orders</main>" } },
    })), id);
    try {
      const plan = await owner.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
      return await owner.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId });
    } finally { await owner.cancel(transfer.transferId); }
  })();
}

function signed(body: string, at = Math.floor(Date.now() / 1000), key = secret, nonce?: string) {
  return signRoute(key, { method: "POST", path: "/hooks/orders", body }, at, nonce);
}

function route(pathname: string, init: { method?: string; body?: string; headers?: Record<string, string> } = {}, on = host) {
  return fetch(url + pathname, {
    method: init.method ?? "GET", body: init.body, redirect: "manual",
    headers: { host: on, "x-forwarded-for": freshAddress(), ...(init.body ? { "content-type": "application/json" } : {}), ...init.headers },
  });
}

async function json(response: Response) {
  return { status: response.status, body: await response.json() };
}

let port = 0;

async function startKernel() {
  kernel = Bun.spawn(["kernel-rs/target/debug/konae-kernel"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))),
      KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
      KONAE_DATA_DIR: root, KONAE_ADMIN_KEY: setupKey, KONAE_HOST_SUFFIX: "localhost",
      KONAE_TRUSTED_PROXIES: "127.0.0.1",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (kernel.exitCode !== null) throw Error("HTTP route test Kernel stopped before startup. Set KONAE_HTTP_TEST_PORT to a free port.");
    try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  expect(ready).toBe(true);
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-http-routes-"));
  port = Number(process.env.KONAE_HTTP_TEST_PORT ?? 19700 + Math.floor(Math.random() * 40));
  url = `http://127.0.0.1:${port}`;
  await startKernel();
  const response = await fetch(`${url}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@routes.test", password, setupKey }),
  });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  await owner.call("project_create", { projectId: "http_routes", name: "HTTP routes" });
}, 120_000);

afterAll(async () => {
  kernel?.kill();
  if (kernel) await kernel.exited;
  if (root) await rm(root, { recursive: true, force: true });
});

describe("HTTP routes", () => {
  test("a release naming an unset secret is refused, then publishes once the owner sets it", async () => {
    const files = { "schema.sql": schema, "actions.json": JSON.stringify(actions) };
    await expect(release("http_routes", files)).rejects.toThrow(
      "Actions orders.receive and orders.void sign with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.");
    await owner.call("secret_set", { projectId: "http_routes", name: "orders_webhook", value: secret });
    const listed = await owner.call<{ secrets: { name: string }[] }>("secrets_list", { projectId: "http_routes" });
    expect(listed.secrets.map(item => item.name)).toEqual(["orders_webhook"]);
    expect(JSON.stringify(listed)).not.toContain(secret);
    const receipt = await release("http_routes", files);
    host = receipt.hostname;
    deploymentId = receipt.id;
    await expect(owner.call("secret_delete", { projectId: "http_routes", name: "orders_webhook" })).rejects.toThrow(
      "The published action orders.receive, orders.void signs with orders_webhook. Publish without that signature before deleting the secret.");
  });

  test("the Kernel refuses at publish what the Gate refuses, with the same message", async () => {
    const authenticated = structuredClone(actions);
    (authenticated.actions["notes.mine"] as Record<string, unknown>).http = { method: "GET", path: "/api/mine" };
    await expect(release("http_routes", { "schema.sql": schema, "actions.json": JSON.stringify(authenticated) })).rejects.toThrow(
      "Action notes.mine is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route.");
    const collision = structuredClone(actions);
    (collision.actions["notes.add"] as Record<string, unknown>).http = { method: "POST", path: "/hooks/orders" };
    await expect(release("http_routes", { "schema.sql": schema, "actions.json": JSON.stringify(collision) })).rejects.toThrow(
      "Actions notes.add and orders.receive both bind POST /hooks/orders.");
  });

  test("an unsigned route on a private Deployment needs a key or a session, as its shell needs sign-in", async () => {
    expect(await (await route("/")).text()).toContain("Sign in to this app");
    expect(await (await route("/elsewhere")).text()).toContain("Sign in to this app");
    const listed = await json(await route("/api/orders"));
    expect(listed).toEqual({ status: 401, body: { error: "authentication-required" } });
    const unknown = await route("/hooks/unknown", { method: "POST", body: "{}" });
    expect(unknown.status).toBe(405);
    expect(unknown.headers.get("allow")).toBe("GET, HEAD");
    const postOnGetRoute = await route("/api/orders", { method: "POST", body: "{}" });
    expect(postOnGetRoute.status).toBe(405);
    expect(postOnGetRoute.headers.get("allow")).toBe("GET");
  });

  test("HEAD on a GET route is refused before the query runs, and HEAD on the shell still answers", async () => {
    const head = await route("/api/orders", { method: "HEAD", headers: { authorization: "Bearer lbk_unknown" } });
    expect(head.status).toBe(405);
    expect(head.headers.get("allow")).toBe("GET");
    expect(await head.text()).toBe("");
    expect((await route("/", { method: "HEAD" })).status).toBe(200);
  });

  test("a signed POST is accepted once, and unsigned, tampered, stale, foreign and replayed ones are refused", async () => {
    const body = JSON.stringify({ id: "A-1", total: 12.5 });
    const refusedWith = async (headers: Record<string, string>, sent = body) => json(await route("/hooks/orders", { method: "POST", body: sent, headers }));
    expect(await refusedWith({})).toEqual({ status: 401, body: { error: "invalid-signature" } });
    expect(await refusedWith({ "x-signature": await signed(body) }, JSON.stringify({ id: "A-1", total: 99 }))).toEqual({ status: 401, body: { error: "invalid-signature" } });
    // A stale t has its own answer, whatever the secret, so a sender with a skewed clock can tell.
    expect(await refusedWith({ "x-signature": await signed(body, Math.floor(Date.now() / 1000) - 301) })).toEqual({ status: 401, body: { error: "stale-timestamp" } });
    expect(await refusedWith({ "x-signature": await signed(body, Math.floor(Date.now() / 1000) + 301, "another-secret-0123456789") })).toEqual({ status: 401, body: { error: "stale-timestamp" } });
    expect(await refusedWith({ "x-signature": await signed(body, undefined, "another-secret-0123456789") })).toEqual({ status: 401, body: { error: "invalid-signature" } });
    const elsewhere = await signRoute(secret, { method: "POST", path: "/hooks/void", body });
    expect(await refusedWith({ "x-signature": elsewhere })).toEqual({ status: 401, body: { error: "invalid-signature" } });
    const now = Math.floor(Date.now() / 1000);
    const header = await signed(body, now, secret, "first");
    expect(await refusedWith({ "x-signature": header })).toEqual({ status: 200, body: { result: { changes: 1, lastInsertId: 1 } } });
    expect(await refusedWith({ "x-signature": header })).toEqual({ status: 409, body: { error: "replayed-signature" } });
    // A retry in the same second with a new nonce is a new request.
    const retry = await json(await route("/hooks/orders", { method: "POST", body, headers: { "x-signature": await signed(body, now, secret, "retry") } }));
    expect(retry).toEqual({ status: 200, body: { result: { changes: 1, lastInsertId: 2 } } });
    await owner.call("database_execute", { deploymentId, operation: "sql", input: { sql: "DELETE FROM orders WHERE id = 2" } });
    const second = JSON.stringify({ id: "B-2", total: 3 });
    expect((await refusedWith({ "x-signature": await signed(second) }, second)).status).toBe(200);
    const extra = JSON.stringify({ id: "C-3", total: 1, note: "x" });
    expect(await refusedWith({ "x-signature": await signed(extra) }, extra)).toEqual({ status: 400, body: { error: "Unexpected action input: note" } });
    expect((await route("/hooks/orders", { method: "POST", body: "x".repeat(64 * 1024 + 1) })).status).toBe(413);
  });

  test("a GET route maps its query string onto typed bindings", async () => {
    const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Query reader", actions: ["orders.list"] });
    const get = (pathname: string) => route(pathname, { headers: { authorization: `Bearer ${reader.key}` } });
    expect(await json(await get("/api/orders"))).toEqual({ status: 200, body: { result: [{ external_id: "A-1", total: 12.5 }, { external_id: "B-2", total: 3 }] } });
    expect(await json(await get("/api/orders?min=10"))).toEqual({ status: 200, body: { result: [{ external_id: "A-1", total: 12.5 }] } });
    expect(await json(await get("/api/orders?min=ten"))).toEqual({ status: 400, body: { error: "min must be a number." } });
    expect(await json(await get("/api/orders?other=1"))).toEqual({ status: 400, body: { error: "Unexpected action input: other" } });
    expect(await json(await get("/api/orders?min=1&min=2"))).toEqual({ status: 400, body: { error: "Repeated query parameter: min" } });
    const response = await get("/api/orders");
    expect(response.headers.get("content-type")).toBe("application/json");
    expect(response.headers.get("cache-control")).toBe("no-store");
  });

  test("an API key is a service actor bound to the actions it names", async () => {
    const issued = await owner.call<{ keyId: string; key: string }>("api_keys_create", {
      deploymentId, name: "Order sync", actions: ["notes.add", "orders.receive"], files: ["private"],
    });
    expect(issued.key).toStartWith("lbk_");
    const bearer = { authorization: `Bearer ${issued.key}` };
    const note = await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "from a program" }), headers: bearer }));
    expect(note.status).toBe(200);
    const rows = await owner.call<{ rows: { owner_id: string }[] }>("database_execute", { deploymentId, operation: "sql", input: { sql: "SELECT owner_id FROM notes" } });
    expect(JSON.stringify(rows)).toContain(issued.keyId);
    // A key that names a signed action stands in for the signature.
    const unsigned = JSON.stringify({ id: "K-1", total: 1 });
    expect((await route("/hooks/orders", { method: "POST", body: unsigned, headers: bearer })).status).toBe(200);
    expect(await json(await route("/api/orders", { headers: bearer }))).toEqual({ status: 403, body: { error: "This API key may not call orders.list." } });
    expect(await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "x" }), headers: { authorization: "Bearer lbk_unknown" } })))
      .toEqual({ status: 401, body: { error: "invalid-api-key" } });
    // On a private Deployment an unsigned route takes a key or a session, and nothing else.
    expect(await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "x" }) })))
      .toEqual({ status: 401, body: { error: "authentication-required" } });
    // A route whose callers leave out "key" refuses every key, before the action is checked.
    expect(await json(await route("/hooks/void", { method: "POST", body: JSON.stringify({ id: "K-1" }), headers: bearer })))
      .toEqual({ status: 403, body: { error: "This route does not accept API keys." } });
    const listed = await owner.call<{ keys: Record<string, unknown>[] }>("api_keys_list", { deploymentId });
    expect(listed.keys.map(key => key.keyId)).toContain(issued.keyId);
    expect(JSON.stringify(listed)).not.toContain(issued.key);
    await owner.call("api_keys_revoke", { deploymentId, keyId: issued.keyId });
    expect((await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "x" }), headers: bearer })).status).toBe(401);
    await expect(owner.call("api_keys_create", { deploymentId, name: "Wrong", actions: ["notes.mine"] })).rejects.toThrow("Action notes.mine has no HTTP route, so a key cannot call it.");
  });

  test("unknown API keys use up the client's bucket, and then no key is looked up from that address", async () => {
    const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Order reader", actions: ["orders.list"] });
    const from = (address: string, key: string, pathname = "/api/orders") =>
      route(pathname, { headers: { authorization: `Bearer ${key}`, "x-forwarded-for": address } });
    const statuses: number[] = [];
    for (let i = 0; i < 9; i++) statuses.push((await from("198.18.3.1", `lbk_wrong${i}`)).status);
    // The file read path shares the bucket.
    statuses.push((await from("198.18.3.1", "lbk_wrong-file", `/_konae/files/${crypto.randomUUID()}`)).status);
    expect(statuses).toEqual(Array(10).fill(401));
    const limited = await from("198.18.3.1", "lbk_wrong-again");
    expect(await json(limited)).toEqual({ status: 429, body: { error: "rate-limited" } });
    expect(Number(limited.headers.get("retry-after"))).toBeGreaterThan(0);
    expect((await from("198.18.3.1", reader.key)).status).toBe(429);
    expect((await from("198.18.3.2", reader.key)).status).toBe(200);
    for (let i = 0; i < 15; i++) expect((await from("198.18.3.3", reader.key)).status).toBe(200);
  });

  test("a session cookie counts only from the app's own origin", async () => {
    await owner.call("signup_policy_set", { deploymentId, mode: "open" });
    const signup = await route("/_konae/auth/signup", { method: "POST", body: JSON.stringify({ email: "ana@routes.test", password }) });
    expect(signup.status).toBe(200);
    const cookie = signup.headers.get("set-cookie")!.split(";")[0];
    const own = await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "mine" }), headers: { cookie, "sec-fetch-site": "same-origin" } }));
    expect(own.status).toBe(200);
    for (const site of ["same-site", "cross-site"]) {
      expect(await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "forged" }), headers: { cookie, "sec-fetch-site": site } })))
        .toEqual({ status: 401, body: { error: "authentication-required" } });
    }
    expect((await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "forged" }), headers: { cookie, origin: "http://other-app.localhost" } })).status).toBe(401);
    // A signed route keeps asking the person for its signature.
    expect(await json(await route("/hooks/orders", { method: "POST", body: JSON.stringify({ id: "S-1", total: 1 }), headers: { cookie, "sec-fetch-site": "same-origin" } })))
      .toEqual({ status: 401, body: { error: "invalid-signature" } });
  });

  test("an action whose route does not answer anyone answers only on its route", async () => {
    const signup = await route("/_konae/auth/login", { method: "POST", body: JSON.stringify({ email: "ana@routes.test", password }) });
    const cookie = signup.headers.get("set-cookie")!.split(";")[0];
    const member = new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("host", host); headers.set("cookie", cookie);
      return fetch(input, { ...init, headers });
    } });
    await expect(member.call("actions_invoke", { name: "orders.receive", input: { id: "M-1", total: 1 } }))
      .rejects.toThrow("Action orders.receive answers only on its HTTP route, POST /hooks/orders.");
    await expect(member.call("actions_invoke", { name: "orders.list", input: {} }))
      .rejects.toThrow("Action orders.list answers only on its HTTP route, GET /api/orders.");
    expect(await member.call("actions_invoke", { name: "notes.mine", input: {} })).toEqual([{ body: "mine" }]);
    // The same person reads the unsigned route with the session cookie, from the app's origin.
    expect((await route("/api/orders", { headers: { cookie, "sec-fetch-site": "same-origin" } })).status).toBe(200);
  });

  test("the runtime endpoint refuses an API key and says where it works", async () => {
    const issued = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Misplaced", actions: ["orders.list"] });
    const response = await route("/_konae/mcp", {
      method: "POST", headers: { authorization: `Bearer ${issued.key}`, accept: "application/json, text/event-stream" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "actions_invoke", arguments: { name: "orders.list", input: {} } } }),
    });
    expect(await json(response)).toEqual({ status: 403, body: { error: "An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session." } });
  });

  test("the file read path applies the file's visibility to each caller", async () => {
    const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a8ioAAAAASUVORK5CYII="), c => c.charCodeAt(0));
    const signup = async (email: string) => {
      const response = await route("/_konae/auth/signup", { method: "POST", body: JSON.stringify({ email, password }) });
      return response.headers.get("set-cookie")!.split(";")[0];
    };
    const ben = await signup("ben@routes.test");
    const cara = await signup("cara@routes.test");
    const runtime = new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("host", host); headers.set("cookie", ben);
      return fetch(input, { ...init, headers });
    } });
    const transfer = await runtime.upload(png);
    const file = await runtime.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "badge.png", type: "image/png" });
    const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Badge reader", files: ["private"] });
    const publicOnly = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Public reader", files: ["public"] });
    const read = (headers: Record<string, string>) => route(`/_konae/files/${file.id}`, { headers });

    const byKey = await read({ authorization: `Bearer ${reader.key}` });
    expect(byKey.status).toBe(200);
    expect(byKey.headers.get("content-type")).toBe("image/png");
    expect(byKey.headers.get("content-disposition")).toBe("inline");
    expect(byKey.headers.get("x-content-type-options")).toBe("nosniff");
    expect(byKey.headers.get("cross-origin-resource-policy")).toBe("same-origin");
    expect(new Uint8Array(await byKey.arrayBuffer())).toEqual(png);
    expect((await read({ cookie: ben, "sec-fetch-site": "same-origin" })).status).toBe(200);
    expect((await read({ cookie: cara })).status).toBe(404);
    expect((await read({ authorization: `Bearer ${publicOnly.key}` })).status).toBe(404);
    expect((await read({ cookie: ben, "sec-fetch-site": "same-site" })).status).toBe(401);
    expect((await read({})).status).toBe(401);
    expect((await route(`/_konae/files/${crypto.randomUUID()}`, { headers: { authorization: `Bearer ${reader.key}` } })).status).toBe(404);
    expect((await route("/_konae/files/", { headers: { authorization: `Bearer ${reader.key}` } })).status).toBe(410);
  });

  test("one owner's two apps cannot share a route secret", async () => {
    await owner.call("project_create", { projectId: "http_routes_copy", name: "HTTP routes copy" });
    // A signature names no app, so one owner's two projects cannot share a value.
    await expect(owner.call("secret_set", { projectId: "http_routes_copy", name: "orders_webhook", value: secret }))
      .rejects.toThrow("Another of your projects already uses this secret value.");
    await owner.call("secret_set", { projectId: "http_routes_copy", name: "orders_webhook", value: `${secret}_copy` });
  });

  test("a guarded mutation that changes no row is refused, counted as refused and records no Signal", async () => {
    const today = new Date().toISOString().slice(0, 10);
    const login = await route("/_konae/auth/login", { method: "POST", body: JSON.stringify({ email: "ana@routes.test", password }) });
    const cookie = login.headers.get("set-cookie")!.split(";")[0];
    const ana = (await owner.call<{ users: { userId: string; email: string }[] }>("app_users_list", { deploymentId })).users.find((user) => user.email === "ana@routes.test")!.userId;
    const member = new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("host", host); headers.set("cookie", cookie);
      return fetch(input, { ...init, headers });
    } });
    const [mine] = (await owner.call<{ rows: { id: number }[] }>("database_execute", { deploymentId, operation: "sql", input: { sql: `SELECT id FROM notes WHERE owner_id = '${ana}' LIMIT 1` } })).rows;
    // The runtime endpoint: a tool error with the guard, then a change that records the Signal with the actor.
    await expect(member.call("actions_invoke", { name: "notes.rename", input: { id: 999_999, body: "not mine" } })).rejects.toThrow("That note is not yours, or it does not exist.");
    expect(await member.call("actions_invoke", { name: "notes.rename", input: { id: mine.id, body: "renamed" } })).toMatchObject({ changes: 1 });
    // Without a guard a mutation that changes nothing still answers { changes: 0 }, and records no Signal.
    expect(await member.call("actions_invoke", { name: "notes.touch", input: { id: 999_999 } })).toEqual({ changes: 0, lastInsertId: 0 });
    // The route: 403 with the guard, then a change by the person's session.
    const edit = (id: number, body: string) => route("/api/notes/edit", { method: "POST", body: JSON.stringify({ id, body }), headers: { cookie, "sec-fetch-site": "same-origin" } });
    expect(await json(await edit(999_999, "not mine"))).toEqual({ status: 403, body: { error: "That note is not yours, or it does not exist." } });
    expect((await json(await edit(mine.id, "edited"))).status).toBe(200);

    const usage = await owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId, from: today, to: today });
    const total = (kind: string, name: string) => usage.totals.find((row) => row.kind === kind && row.name === name)?.count ?? 0;
    expect(total("refused", "notes.rename")).toBe(1);
    expect(total("refused", "notes.edit")).toBe(1);
    expect(total("action", "notes.rename")).toBe(1);
    expect(total("action", "notes.touch")).toBe(1);
    expect(total("route", "POST /api/notes/edit")).toBe(1);
    const signals = async (name: string) => (await owner.call<{ signals: { properties: Record<string, unknown> }[] }>("signals_read", { deploymentId, name })).signals;
    expect((await signals("note.renamed")).map((signal) => signal.properties)).toEqual([{ id: mine.id, actor: ana }]);
    expect((await signals("note.edited")).map((signal) => signal.properties)).toEqual([{ id: mine.id, actor: ana }]);
    expect(await signals("note.touched")).toEqual([]);
  });

  test("anonymous callers share the runtime's rate limit per client address", async () => {
    // A caller without a key or a session is refused on this private route,
    // and still counts against its address before the refusal.
    const statuses: number[] = [];
    for (let i = 0; i < 31; i++) {
      statuses.push((await route("/api/orders", { headers: { "x-forwarded-for": "198.18.2.1" } })).status);
    }
    expect(statuses.slice(0, 30).every(status => status === 401)).toBe(true);
    const limited = await route("/api/orders", { headers: { "x-forwarded-for": "198.18.2.1" } });
    expect(limited.status).toBe(429);
    expect(Number(limited.headers.get("retry-after"))).toBeGreaterThan(0);
  });

  test("a signature accepted before a Kernel restart is refused after it", async () => {
    const body = JSON.stringify({ id: "R-1", total: 7 });
    const header = await signed(body);
    const send = async () => json(await route("/hooks/orders", { method: "POST", body, headers: { "x-signature": header } }));
    expect((await send()).status).toBe(200);
    kernel.kill();
    await kernel.exited;
    await startKernel();
    expect(await send()).toEqual({ status: 409, body: { error: "replayed-signature" } });
    const rows = await owner.call<{ rows: unknown[] }>("database_execute", { deploymentId, operation: "sql", input: { sql: "SELECT id FROM orders WHERE external_id = 'R-1'" } });
    expect(rows.rows).toHaveLength(1);
  });
});
