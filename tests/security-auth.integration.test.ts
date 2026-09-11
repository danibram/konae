import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";

const processes: ReturnType<typeof Bun.spawn>[] = [];
const roots: string[] = [];
const password = "security-auth-password-2026";
let sequence = 0;

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
}, 120_000);

afterAll(async () => {
  for (const child of processes) if (child.exitCode === null) child.kill();
  await Promise.all(processes.map(child => child.exited));
  await Promise.all(roots.map(root => rm(root, { recursive: true, force: true })));
});

async function installation(https: boolean, separateMcp = false) {
  const port = 19620 + sequence++;
  const url = `http://127.0.0.1:${port}`;
  const host = separateMcp ? `control.localhost:${port}` : `localhost:${port}`;
  const root = await mkdtemp(path.join(tmpdir(), "littlebox-security-auth-"));
  roots.push(root);
  const key = crypto.randomUUID();
  const child = Bun.spawn(["kernel-rs/target/debug/littlebox-kernel"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("LITTLEBOX_") && !name.startsWith("VITE_"))),
      LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: String(port), LITTLEBOX_DATA_DIR: root,
      LITTLEBOX_ADMIN_KEY: key, LITTLEBOX_PUBLIC_ORIGIN: `${https ? "https" : "http"}://localhost:${port}`,
      LITTLEBOX_HOST_SUFFIX: "localhost",
      ...(separateMcp ? { LITTLEBOX_MCP_ORIGIN: `https://control.localhost:${port}` } : {}),
    }, stdout: "ignore", stderr: "inherit",
  });
  processes.push(child);
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (child.exitCode !== null) throw Error("Security test Kernel stopped before startup.");
    try { ready = (await fetch(`${url}/_littlebox/health`, { headers: { host } })).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  if (!ready) throw Error("Security test Kernel did not become ready.");
  const json = (route: string, body: object, headers: Record<string, string> = {}) => fetch(`${url}${route}`, {
    method: "POST", headers: { host, "content-type": "application/json", ...headers }, body: JSON.stringify(body),
  });
  const ownerResponse = await json("/_littlebox/owner/signup", { email: "owner@security.test", password, setupKey: key }, { "x-forwarded-proto": "http" });
  expect(ownerResponse.status).toBe(200);
  const ownerSetCookie = ownerResponse.headers.get("set-cookie")!;
  const cookie = ownerSetCookie.split(";")[0];
  const client = (endpoint: string, selectedHost: string, selectedCookie?: string) => new McpClient(`${url}${endpoint}`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", selectedHost);
    if (selectedCookie) headers.set("cookie", selectedCookie);
    return fetch(input, { ...init, headers });
  } });
  const owner = client("/mcp", host, cookie);
  return { url, host, json, ownerResponse, ownerSetCookie, cookie, client, owner };
}

function assertCookie(header: string, name: string, secure: boolean) {
  expect(header.startsWith(name + "=")).toBe(true);
  expect(header).toContain("; Path=/");
  expect(header).toContain("; HttpOnly");
  expect(header.includes("; Secure")).toBe(secure);
  expect(header.toLowerCase()).not.toContain("domain=");
}

describe("authentication cookie isolation", () => {
  test("HTTPS owners use a host-prefixed cookie, refuse legacy cookies and clear the same cookie", async () => {
    const app = await installation(true);
    assertCookie(app.ownerSetCookie, "__Host-littlebox_owner_session", true);
    const session = await app.owner.call<{ ownerId: string }>("session_inspect");
    expect(session.ownerId).toBeTruthy();
    const token = app.cookie.split("=")[1];
    await expect(app.client("/mcp", app.host, `littlebox_owner_session=${token}`).call("session_inspect")).rejects.toThrow();
    const logout = await app.json("/_littlebox/owner/logout", {}, { cookie: app.cookie });
    expect(logout.status).toBe(200);
    assertCookie(logout.headers.get("set-cookie")!, "__Host-littlebox_owner_session", true);
    expect(logout.headers.get("set-cookie")).toContain("Max-Age=0");
    await expect(app.owner.call("session_inspect")).rejects.toThrow();
  });

  test("HTTPS app sessions keep Secure despite a missing or downgraded proxy header", async () => {
    const app = await installation(true);
    const id = "security_cookie_app";
    await app.owner.call("project_create", { projectId: id, name: "Cookie test" });
    const upload = await app.owner.upload(new TextEncoder().encode(JSON.stringify({
      project: { id, name: "Cookie test", slug: "cookie-test", manifest: { version: 1, visibility: "public" }, files: {
        "schema.sql": "CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY);", "actions.json": '{"version":1,"actions":{}}',
      } }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Cookie test</main>" } },
    })), id);
    let deployment: { hostname: string };
    try {
      const plan = await app.owner.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
      deployment = await app.owner.call("release_apply", { planId: plan.planId });
    } finally { await app.owner.cancel(upload.transferId); }
    let memberCookie = "";
    for (const forwarded of [undefined, "http", "https"]) {
      const response = await app.json(memberCookie ? "/_littlebox/auth/login" : "/_littlebox/auth/signup", { email: "member@security.test", password }, {
        host: deployment.hostname, ...(forwarded ? { "x-forwarded-proto": forwarded } : {}),
      });
      expect(response.status).toBe(200);
      assertCookie(response.headers.get("set-cookie")!, "__Host-littlebox_app_session", true);
      memberCookie = response.headers.get("set-cookie")!.split(";")[0];
    }
    const member = app.client("/_littlebox/mcp", deployment.hostname, memberCookie);
    expect((await member.call<{ role: string }>("auth_session")).role).toBe("member");
    const legacy = app.client("/_littlebox/mcp", deployment.hostname, memberCookie.replace("__Host-littlebox_app_session", "littlebox_app_session"));
    expect((await legacy.call<{ role: string }>("auth_session")).role).toBe("anonymous");
    const response = await app.json("/_littlebox/mcp", { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "auth_logout", arguments: {} } }, { host: deployment.hostname, cookie: memberCookie });
    expect(response.status).toBe(200);
    assertCookie(response.headers.get("set-cookie")!, "__Host-littlebox_app_session", true);
    expect((await member.call<{ role: string }>("auth_session")).role).toBe("anonymous");
    expect((await fetch(`${app.url}/_littlebox/owner/login`, { headers: { host: deployment.hostname } })).status).toBe(403);
    expect((await app.json("/_littlebox/owner/logout", {}, { host: deployment.hostname })).status).toBe(403);
  });

  test("HTTP local owners keep a usable local cookie", async () => {
    const app = await installation(false);
    assertCookie(app.ownerSetCookie, "littlebox_owner_session", false);
    expect((await app.owner.call<{ ownerId: string }>("session_inspect")).ownerId).toBeTruthy();
  });

  test("a separate HTTPS MCP origin secures its owner cookie", async () => {
    const app = await installation(false, true);
    assertCookie(app.ownerSetCookie, "__Host-littlebox_owner_session", true);
    expect((await app.owner.call<{ ownerId: string }>("session_inspect")).ownerId).toBeTruthy();
  });
});
