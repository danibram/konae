import { afterAll, beforeAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";

// ADR-0010 against a real Kernel, the same binary `bun run littlebox -- dev`
// starts: Usage counts what the Kernel allowed and nothing it refused, a
// Signal is recorded from the action's inputs, and only the owner reads them.

setDefaultTimeout(60_000);

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let owner: McpClient;
let host: string;
let deploymentId: string;
const setupKey = crypto.randomUUID();
const password = crypto.randomUUID();
let address = 0;
const freshAddress = () => `198.18.3.${++address}`;
const today = () => new Date().toISOString().slice(0, 10);
const yesterday = () => new Date(Date.now() - 86_400_000).toISOString().slice(0, 10);

const actions = {
  version: 1,
  actions: {
    "notes.add": {
      kind: "mutation", access: "anonymous",
      statement: "INSERT INTO notes (topic, body) VALUES (?1, ?2)",
      bindings: [{ source: "input", key: "topic", type: "string", maxLength: 40 }, { source: "input", key: "body", type: "string", maxLength: 400 }],
      signal: { name: "note.added", properties: ["topic"] },
    },
    "notes.list": {
      kind: "query", access: "anonymous",
      statement: "SELECT topic, body FROM notes ORDER BY id LIMIT 50", bindings: [],
    },
    "notes.mine": {
      kind: "query", access: "authenticated",
      statement: "SELECT body FROM notes WHERE topic = ?1 LIMIT 50", bindings: [{ source: "actor", field: "id" }],
    },
    "page.like": {
      kind: "signal", access: "anonymous",
      bindings: [{ source: "input", key: "page", type: "string", maxLength: 80 }, { source: "input", key: "stars", type: "number", nullable: true }],
      signal: { name: "page.liked", properties: ["page", "stars"] },
    },
    "orders.receive": {
      kind: "mutation", access: "anonymous",
      statement: "INSERT INTO notes (topic, body) VALUES ('order', ?1)",
      bindings: [{ source: "input", key: "id", type: "string", maxLength: 64 }],
      http: { method: "POST", path: "/hooks/orders" },
      signal: { name: "order.received", properties: ["id"] },
    },
  },
};
// The CHECK lets a call pass its bindings and still fail in SQL.
const schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, topic TEXT NOT NULL, body TEXT NOT NULL CHECK (length(body) < 100));";

async function release(id: string, files: Record<string, string>) {
  const project = { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" } };
  const transfer = await owner.upload(new TextEncoder().encode(JSON.stringify({
    project: { ...project, files }, visibility: "public",
    artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Usage</main>" } },
  })), id);
  try {
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    return await owner.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId });
  } finally { await owner.cancel(transfer.transferId); }
}

function runtime(cookie?: string) {
  return new McpClient(`${url}/_littlebox/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", host); headers.set("x-forwarded-for", freshAddress());
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

function post(pathname: string, body: unknown, headers: Record<string, string> = {}) {
  return fetch(url + pathname, {
    method: "POST", body: JSON.stringify(body),
    headers: { host, "content-type": "application/json", "x-forwarded-for": freshAddress(), ...headers },
  });
}

type Totals = { totals: { kind: string; name: string; count: number }[] };
async function totals() {
  const usage = await owner.call<Totals>("usage_read", { deploymentId, from: yesterday(), to: today() });
  return Object.fromEntries(usage.totals.map((row) => [row.name ? `${row.kind} ${row.name}` : row.kind, row.count]));
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-usage-"));
  const port = Number(process.env.LITTLEBOX_USAGE_TEST_PORT ?? 19950 + Math.floor(Math.random() * 40));
  url = `http://127.0.0.1:${port}`;
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
    if (kernel.exitCode !== null) throw Error("Usage test Kernel stopped before startup. Set LITTLEBOX_USAGE_TEST_PORT to a free port.");
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  expect(ready).toBe(true);
  const response = await fetch(`${url}/_littlebox/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@usage.test", password, setupKey }),
  });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  await owner.call("project_create", { projectId: "usage_app", name: "Usage" });
  const receipt = await release("usage_app", { "schema.sql": schema, "actions.json": JSON.stringify(actions) });
  host = receipt.hostname;
  deploymentId = receipt.id;
}, 120_000);

afterAll(async () => {
  kernel?.kill();
  if (kernel) await kernel.exited;
  if (root) await rm(root, { recursive: true, force: true });
});

describe("Usage and Signals", () => {
  test("a new Deployment has no store and reads as empty", async () => {
    expect(await owner.call("usage_read", { deploymentId, from: today(), to: today() })).toEqual({ from: today(), to: today(), days: [], totals: [] });
    expect(await owner.call("signals_read", { deploymentId })).toEqual({ signals: [], nextCursor: null });
  });

  test("counts what the Kernel allowed and records each declared Signal from its inputs", async () => {
    const visitor = runtime();
    await visitor.call("auth_session");
    await visitor.call("actions_invoke", { name: "notes.add", input: { topic: "garden", body: "Tomatoes" } });
    expect(await visitor.call("actions_invoke", { name: "page.like", input: { page: "/home", stars: 5 } })).toEqual({ recorded: true });
    expect(await visitor.call("actions_invoke", { name: "page.like", input: { page: "/about" } })).toEqual({ recorded: true });
    await visitor.call("actions_invoke", { name: "notes.list" });
    const routed = await post("/hooks/orders", { id: "A-1" });
    expect(routed.status).toBe(200);
    const signup = await post("/_littlebox/auth/signup", { email: "ana@usage.test", password });
    expect(signup.status).toBe(200);
    const login = await post("/_littlebox/auth/login", { email: "ana@usage.test", password });
    expect(login.status).toBe(200);
    const ana = runtime(login.headers.get("set-cookie")!.split(";")[0]);
    const upload = await ana.upload(new TextEncoder().encode("attachment"));
    const file = await ana.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain" });
    await ana.cancel(upload.transferId);
    await ana.download(await ana.call("files_read", { fileId: file.id }));
    const read = await fetch(`${url}/_littlebox/files/${file.id}`, { headers: { host, cookie: login.headers.get("set-cookie")!.split(";")[0], "sec-fetch-site": "same-origin" } });
    expect(read.status).toBe(200);

    expect(await totals()).toEqual({
      session: 1, "action notes.add": 1, "action page.like": 2, "action notes.list": 1, "route POST /hooks/orders": 1,
      signup: 1, login: 1, upload: 1, file_read: 2,
    });
    const signals = await owner.call<{ signals: { name: string; at: number; properties: Record<string, unknown> }[] }>("signals_read", { deploymentId });
    expect(signals.signals.map(({ name, properties }) => ({ name, properties }))).toEqual([
      { name: "order.received", properties: { id: "A-1" } },
      { name: "page.liked", properties: { page: "/about", stars: null } },
      { name: "page.liked", properties: { page: "/home", stars: 5 } },
      { name: "note.added", properties: { topic: "garden" } },
    ]);
    const liked = await owner.call<{ signals: unknown[] }>("signals_read", { deploymentId, name: "page.liked" });
    expect(liked.signals).toHaveLength(2);
    expect((await stat(path.join(root, "deployments", deploymentId, "usage.sqlite"))).isFile()).toBe(true);
  });

  test("a refused call counts nothing and records no Signal", async () => {
    const before = await totals();
    const signalsBefore = (await owner.call<{ signals: unknown[] }>("signals_read", { deploymentId })).signals.length;
    const visitor = runtime();
    // Needs a session.
    await expect(visitor.call("actions_invoke", { name: "notes.mine" })).rejects.toThrow("authentication-required");
    // An input the action does not declare.
    await expect(visitor.call("actions_invoke", { name: "notes.add", input: { topic: "x", body: "y", extra: 1 } })).rejects.toThrow("Unexpected action input: extra");
    await expect(visitor.call("actions_invoke", { name: "page.like", input: {} })).rejects.toThrow("Missing action input: page");
    // Bindings pass, SQL refuses: the CHECK fails.
    await expect(visitor.call("actions_invoke", { name: "notes.add", input: { topic: "x", body: "y".repeat(150) } })).rejects.toThrow("CHECK");
    // A route with a missing input.
    expect((await post("/hooks/orders", {})).status).toBe(400);
    // A wrong password and a repeated signup.
    expect((await post("/_littlebox/auth/login", { email: "ana@usage.test", password: "wrong-password-123" })).status).toBe(401);
    expect((await post("/_littlebox/auth/signup", { email: "ana@usage.test", password })).status).not.toBe(200);
    // A file another caller may not read.
    expect((await fetch(`${url}/_littlebox/files/${crypto.randomUUID()}`, { headers: { host } })).status).toBe(404);
    expect(await totals()).toEqual(before);
    expect((await owner.call<{ signals: unknown[] }>("signals_read", { deploymentId })).signals).toHaveLength(signalsBefore);
  });

  test("only the owner reads Usage and Signals", async () => {
    for (const name of ["usage_read", "signals_read"]) {
      await expect(runtime().call(name, { deploymentId, from: today(), to: today() })).rejects.toThrow("Unknown tool or insufficient permission.");
    }
    await owner.call("project_create", { projectId: "usage_other", name: "Other" });
    const { token } = await owner.call<{ token: string }>("token_create", { scopes: ["read"], projectIds: ["usage_other"], ttlSeconds: 600 });
    const elsewhere = new McpClient(`${url}/mcp`, { token });
    await expect(elsewhere.call("usage_read", { deploymentId, from: today(), to: today() })).rejects.toThrow("outside this token's grants");
    await expect(elsewhere.call("signals_read", { deploymentId })).rejects.toThrow("outside this token's grants");
    const { token: reader } = await owner.call<{ token: string }>("token_create", { scopes: ["read"], projectIds: ["usage_app"], ttlSeconds: 600 });
    expect((await new McpClient(`${url}/mcp`, { token: reader }).call<Totals>("usage_read", { deploymentId, from: today(), to: today() })).totals.length).toBeGreaterThan(0);
  });

  test("usage_read refuses a malformed or oversized range and signals_read a forged cursor", async () => {
    await expect(owner.call("usage_read", { deploymentId, from: "2026-02-30", to: today() })).rejects.toThrow("from and to must be dates written YYYY-MM-DD.");
    await expect(owner.call("usage_read", { deploymentId, from: today(), to: yesterday() })).rejects.toThrow("from must not be after to.");
    await expect(owner.call("usage_read", { deploymentId, from: "2020-01-01", to: today() })).rejects.toThrow("Usage covers at most 400 days at a time.");
    await expect(owner.call("signals_read", { deploymentId, cursor: "next" })).rejects.toThrow("Invalid cursor.");
  });

  test("the Kernel refuses a signal the editor refuses, with the same message", async () => {
    const refused = { ...actions, actions: { ...actions.actions, "page.like": { ...actions.actions["page.like"], signal: { name: "page.liked", properties: ["visitor"] } } } };
    await expect(release("usage_app", { "schema.sql": schema, "actions.json": JSON.stringify(refused) }))
      .rejects.toThrow("Action page.like names signal property visitor, which is not one of its input bindings.");
  });
});
