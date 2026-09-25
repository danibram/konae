import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { createHmac } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";

// The HTTP module against a real Kernel: routes on a private Deployment,
// signatures, API keys, the file read path, and keys revoked by an import.

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
    "notes.add": {
      kind: "mutation", access: "anonymous",
      statement: "INSERT INTO notes (owner_id, body) VALUES (?1, ?2)",
      bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }],
      http: { method: "POST", path: "/api/notes" },
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

function signed(body: string, at = Math.floor(Date.now() / 1000), key = secret) {
  return `t=${at},v1=${createHmac("sha256", key).update(`${at}.${body}`).digest("hex")}`;
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
  kernel = Bun.spawn(["kernel-rs/target/debug/littlebox-kernel"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_PUBLIC_ORIGIN: url,
      LITTLEBOX_DATA_DIR: root, LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_HOST_SUFFIX: "localhost",
      LITTLEBOX_TRUSTED_PROXIES: "127.0.0.1",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (kernel.exitCode !== null) throw Error("HTTP route test Kernel stopped before startup. Set LITTLEBOX_HTTP_TEST_PORT to a free port.");
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  expect(ready).toBe(true);
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-http-routes-"));
  port = Number(process.env.LITTLEBOX_HTTP_TEST_PORT ?? 19700 + Math.floor(Math.random() * 40));
  url = `http://127.0.0.1:${port}`;
  await startKernel();
  const response = await fetch(`${url}/_littlebox/owner/signup`, {
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
      "Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.");
    await owner.call("secret_set", { projectId: "http_routes", name: "orders_webhook", value: secret });
    const listed = await owner.call<{ secrets: { name: string }[] }>("secrets_list", { projectId: "http_routes" });
    expect(listed.secrets.map(item => item.name)).toEqual(["orders_webhook"]);
    expect(JSON.stringify(listed)).not.toContain(secret);
    const receipt = await release("http_routes", files);
    host = receipt.hostname;
    deploymentId = receipt.id;
    await expect(owner.call("secret_delete", { projectId: "http_routes", name: "orders_webhook" })).rejects.toThrow(
      "The published action orders.receive signs with orders_webhook. Publish without that signature before deleting the secret.");
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

  test("an anonymous route answers on a private Deployment while its shell stays behind sign-in", async () => {
    expect(await (await route("/")).text()).toContain("Sign in to this app");
    expect(await (await route("/elsewhere")).text()).toContain("Sign in to this app");
    const listed = await json(await route("/api/orders"));
    expect(listed).toEqual({ status: 200, body: { result: [] } });
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
    expect(await refusedWith({ "x-signature": signed(body) }, JSON.stringify({ id: "A-1", total: 99 }))).toEqual({ status: 401, body: { error: "invalid-signature" } });
    expect(await refusedWith({ "x-signature": signed(body, Math.floor(Date.now() / 1000) - 301) })).toEqual({ status: 401, body: { error: "invalid-signature" } });
    expect(await refusedWith({ "x-signature": signed(body, undefined, "another-secret-0123456789") })).toEqual({ status: 401, body: { error: "invalid-signature" } });
    const header = signed(body);
    expect(await refusedWith({ "x-signature": header })).toEqual({ status: 200, body: { result: { changes: 1, lastInsertId: 1 } } });
    expect(await refusedWith({ "x-signature": header })).toEqual({ status: 409, body: { error: "replayed-signature" } });
    const second = JSON.stringify({ id: "B-2", total: 3 });
    expect((await refusedWith({ "x-signature": signed(second) }, second)).status).toBe(200);
    const extra = JSON.stringify({ id: "C-3", total: 1, note: "x" });
    expect(await refusedWith({ "x-signature": signed(extra) }, extra)).toEqual({ status: 400, body: { error: "Unexpected action input: note" } });
    expect((await route("/hooks/orders", { method: "POST", body: "x".repeat(64 * 1024 + 1) })).status).toBe(413);
  });

  test("a GET route maps its query string onto typed bindings", async () => {
    expect(await json(await route("/api/orders"))).toEqual({ status: 200, body: { result: [{ external_id: "A-1", total: 12.5 }, { external_id: "B-2", total: 3 }] } });
    expect(await json(await route("/api/orders?min=10"))).toEqual({ status: 200, body: { result: [{ external_id: "A-1", total: 12.5 }] } });
    expect(await json(await route("/api/orders?min=ten"))).toEqual({ status: 400, body: { error: "min must be a number." } });
    expect(await json(await route("/api/orders?other=1"))).toEqual({ status: 400, body: { error: "Unexpected action input: other" } });
    expect(await json(await route("/api/orders?min=1&min=2"))).toEqual({ status: 400, body: { error: "Repeated query parameter: min" } });
    const response = await route("/api/orders");
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
    // Without a key or a session, an action that binds the actor has nobody to bind.
    expect(await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "x" }) })))
      .toEqual({ status: 400, body: { error: "This action requires an authenticated actor." } });
    const listed = await owner.call<{ keys: Record<string, unknown>[] }>("api_keys_list", { deploymentId });
    expect(listed.keys.map(key => key.keyId)).toEqual([issued.keyId]);
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
    statuses.push((await from("198.18.3.1", "lbk_wrong-file", `/_littlebox/files/${crypto.randomUUID()}`)).status);
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
    const signup = await route("/_littlebox/auth/signup", { method: "POST", body: JSON.stringify({ email: "ana@routes.test", password }) });
    expect(signup.status).toBe(200);
    const cookie = signup.headers.get("set-cookie")!.split(";")[0];
    const own = await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "mine" }), headers: { cookie, "sec-fetch-site": "same-origin" } }));
    expect(own.status).toBe(200);
    for (const site of ["same-site", "cross-site"]) {
      expect(await json(await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "forged" }), headers: { cookie, "sec-fetch-site": site } })))
        .toEqual({ status: 400, body: { error: "This action requires an authenticated actor." } });
    }
    expect((await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "forged" }), headers: { cookie, origin: "http://other-app.localhost" } })).status).toBe(400);
  });

  test("the file read path applies the file's visibility to each caller", async () => {
    const png = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a8ioAAAAASUVORK5CYII="), c => c.charCodeAt(0));
    const signup = async (email: string) => {
      const response = await route("/_littlebox/auth/signup", { method: "POST", body: JSON.stringify({ email, password }) });
      return response.headers.get("set-cookie")!.split(";")[0];
    };
    const ben = await signup("ben@routes.test");
    const cara = await signup("cara@routes.test");
    const runtime = new McpClient(`${url}/_littlebox/mcp`, { fetch: (input, init) => {
      const headers = new Headers(init?.headers); headers.set("host", host); headers.set("cookie", ben);
      return fetch(input, { ...init, headers });
    } });
    const transfer = await runtime.upload(png);
    const file = await runtime.call<{ id: string }>("files_upload", { transferId: transfer.transferId, name: "badge.png", type: "image/png" });
    const reader = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Badge reader", files: ["private"] });
    const publicOnly = await owner.call<{ key: string }>("api_keys_create", { deploymentId, name: "Public reader", files: ["public"] });
    const read = (headers: Record<string, string>) => route(`/_littlebox/files/${file.id}`, { headers });

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
    expect((await route(`/_littlebox/files/${crypto.randomUUID()}`, { headers: { authorization: `Bearer ${reader.key}` } })).status).toBe(404);
    expect((await route("/_littlebox/files/", { headers: { authorization: `Bearer ${reader.key}` } })).status).toBe(410);
  });

  test("a portable export carries keys and the import revokes them", async () => {
    const live = await owner.call<{ key: string; keyId: string }>("api_keys_create", { deploymentId, name: "Travels", actions: ["notes.add"] });
    const exported = await owner.call<{ transferId: string; size: number; sha256: string }>("project_export", { deploymentId });
    const state = (JSON.parse(new TextDecoder().decode(await owner.download(exported as never))) as { result: Record<string, unknown> }).result;
    await owner.call("project_create", { projectId: "http_routes_copy", name: "HTTP routes copy" });
    await owner.call("secret_set", { projectId: "http_routes_copy", name: "orders_webhook", value: secret });
    const copy = await release("http_routes_copy", { "schema.sql": schema, "actions.json": JSON.stringify(actions) }, {
      initialDatabase: state.initialDatabase, initialIdentity: state.initialIdentity, initialFiles: state.initialFiles,
    });
    const keys = await owner.call<{ keys: { keyId: string; revokedAt: number | null; revokedReason: string | null }[] }>("api_keys_list", { deploymentId: copy.id });
    expect(keys.keys.find(key => key.keyId === live.keyId)).toMatchObject({ revokedAt: expect.any(Number), revokedReason: "import" });
    expect(keys.keys.every(key => key.revokedAt !== null)).toBe(true);
    expect((await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "x" }), headers: { authorization: `Bearer ${live.key}` } }, copy.hostname)).status).toBe(401);
    expect((await route("/api/notes", { method: "POST", body: JSON.stringify({ body: "x" }), headers: { authorization: `Bearer ${live.key}` } })).status).toBe(200);
  });

  test("anonymous callers share the runtime's rate limit per client address", async () => {
    const statuses: number[] = [];
    for (let i = 0; i < 31; i++) {
      statuses.push((await route("/api/orders", { headers: { "x-forwarded-for": "198.18.2.1" } })).status);
    }
    expect(statuses.slice(0, 30).every(status => status === 200)).toBe(true);
    const limited = await route("/api/orders", { headers: { "x-forwarded-for": "198.18.2.1" } });
    expect(limited.status).toBe(429);
    expect(Number(limited.headers.get("retry-after"))).toBeGreaterThan(0);
  });

  test("a signature accepted before a Kernel restart is refused after it", async () => {
    const body = JSON.stringify({ id: "R-1", total: 7 });
    const header = signed(body);
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
