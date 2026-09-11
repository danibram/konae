import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let cookie: string;
let appHost: string;
const setupKey = crypto.randomUUID();

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "littlebox-http-security-"));
  const port = 19660 + Math.floor(Math.random() * 20);
  url = `http://127.0.0.1:${port}`;
  kernel = Bun.spawn(["kernel-rs/target/debug/littlebox-kernel"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("LITTLEBOX_") && !key.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_PUBLIC_ORIGIN: url,
      LITTLEBOX_DATA_DIR: root, LITTLEBOX_ADMIN_KEY: setupKey, LITTLEBOX_HOST_SUFFIX: "localhost",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (kernel.exitCode !== null) throw Error("Security test Kernel stopped before startup.");
    try { ready = (await fetch(`${url}/_littlebox/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  expect(ready).toBe(true);
  const response = await fetch(`${url}/_littlebox/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@security.test", password: crypto.randomUUID(), setupKey }),
  });
  expect(response.status).toBe(200);
  cookie = response.headers.get("set-cookie")!.split(";")[0];
  const client = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
  const project = { id: "http_security", name: "HTTP security", slug: "http-security", manifest: { version: 1, visibility: "public" } };
  await client.call("project_create", { projectId: project.id, name: project.name });
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify({
    project: { ...project, files: { "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);", "actions.json": '{"version":1,"actions":{}}' } },
    visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Public shell</main>" } },
  })), project.id);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    appHost = (await client.call<{ hostname: string }>("release_apply", { planId: plan.planId })).hostname;
  } finally { await client.cancel(transfer.transferId); }
}, 120_000);

afterAll(async () => {
  kernel?.kill();
  if (kernel) await kernel.exited;
  if (root) await rm(root, { recursive: true, force: true });
});

describe("HTTP security contract", () => {
  test("legacy data, deployment, file and session routes remain closed for every method and host", async () => {
    const paths = [
      "/_littlebox/admin/deploy", "/_littlebox/admin/storage/test",
      "/_littlebox/admin/deployments/unknown/database/export",
      "/_littlebox/admin/deployments/unknown/portable-state",
      "/_littlebox/admin/deployments/unknown/database", "/_littlebox/session",
      "/_littlebox/auth/logout", "/_littlebox/files", "/_littlebox/files/unknown",
      "/_littlebox/actions/notes.list", "/_littlebox/unknown",
    ];
    for (const host of [new URL(url).host, appHost]) {
      for (const method of ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
        for (const route of paths) {
          const response = await fetch(url + route, {
            method, headers: { host, cookie, "x-littlebox-admin-key": setupKey }, redirect: "manual",
          });
          expect(response.status).toBe(410);
          expect(response.headers.has("access-control-allow-origin")).toBe(false);
          await response.arrayBuffer();
        }
      }
    }
  });

  test("MCP only accepts POST and the setup key never grants MCP access", async () => {
    for (const route of ["/mcp", "/_littlebox/mcp"]) {
      for (const method of ["GET", "HEAD", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
        const response = await fetch(url + route, { method });
        expect(response.status).toBe(405);
        expect(response.headers.has("access-control-allow-origin")).toBe(false);
        await response.arrayBuffer();
      }
    }
    const response = await fetch(`${url}/mcp`, { method: "POST", headers: { "x-littlebox-admin-key": setupKey, "content-type": "application/json" }, body: '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' });
    expect(response.status).toBe(401);
  });

  test("administration is unavailable on app hosts even with an owner session", async () => {
    for (const method of ["initialize", "tools/list"]) {
      const response = await fetch(`${url}/mcp`, {
        method: "POST", headers: { host: appHost, cookie, "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method }),
      });
      expect(response.status).toBe(403);
    }
    expect(await (await fetch(url, { headers: { host: appHost } })).text()).toContain("Public shell");
  });

  test("MCP rejects foreign origins and malformed protocol inputs without exposing CORS", async () => {
    const body = '{"jsonrpc":"2.0","id":1,"method":"tools/list"}';
    for (const origin of ["https://foreign.example", "null", "http://127.0.0.1:1"]) {
      const response = await fetch(`${url}/mcp`, { method: "POST", headers: { cookie, origin, "content-type": "application/json" }, body });
      expect(response.status).toBe(403);
      expect(response.headers.has("access-control-allow-origin")).toBe(false);
    }
    for (const input of ["{", "null", "[]", "false", '{"jsonrpc":"2.0","id":true,"method":"ping"}', '{"jsonrpc":"2.0","id":1}', '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"token_create","arguments":{"scopes":null}}}']) {
      const response = await fetch(`${url}/mcp`, { method: "POST", headers: { cookie, "content-type": "application/json" }, body: input });
      expect(response.status).toBe(200);
      const message = await response.json();
      expect(message.jsonrpc).toBe("2.0");
      expect(message.error.code).toBeLessThan(0);
    }
    const response = await fetch(`${url}/mcp`, { method: "POST", headers: { cookie, "content-type": "application/json" }, body: " ".repeat(200_001) });
    expect(response.status).toBe(413);
  });
});
