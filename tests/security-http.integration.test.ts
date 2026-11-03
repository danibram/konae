import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient } from "../src/mcp/client";
import editorConfig from "../vite.config";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let cookie: string;
let appHost: string;
let client: McpClient;
const setupKey = crypto.randomUUID();

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-http-security-"));
  const port = 19660 + Math.floor(Math.random() * 20);
  url = `http://127.0.0.1:${port}`;
  kernel = Bun.spawn(["kernel-rs/target/debug/konae"], {
    env: {
      ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))),
      KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url,
      KONAE_DATA_DIR: root, KONAE_ADMIN_KEY: setupKey, KONAE_HOST_SUFFIX: "localhost",
    },
    stdout: "ignore", stderr: "inherit",
  });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (kernel.exitCode !== null) throw Error("Security test Kernel stopped before startup.");
    try { ready = (await fetch(`${url}/_konae/health`)).ok; } catch {}
    if (ready) break;
    await Bun.sleep(100);
  }
  expect(ready).toBe(true);
  const response = await fetch(`${url}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@security.test", password: crypto.randomUUID(), setupKey }),
  });
  expect(response.status).toBe(200);
  cookie = response.headers.get("set-cookie")!.split(";")[0];
  client = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
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
      "/_konae/admin/deploy", "/_konae/admin/storage/test",
      "/_konae/admin/deployments/unknown/database/export",
      "/_konae/admin/deployments/unknown/portable-state",
      "/_konae/admin/deployments/unknown/database", "/_konae/session",
      "/_konae/auth/logout", "/_konae/files", "/_konae/files/",
      "/_konae/actions/notes.list", "/_konae/unknown",
    ];
    for (const host of [new URL(url).host, appHost]) {
      for (const method of ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
        for (const route of paths) {
          const response = await fetch(url + route, {
            method, headers: { host, cookie, "x-konae-admin-key": setupKey }, redirect: "manual",
          });
          expect(response.status).toBe(410);
          expect(response.headers.has("access-control-allow-origin")).toBe(false);
          await response.arrayBuffer();
        }
        // The file read path answers GET only, and an unknown file is not found.
        const file = await fetch(`${url}/_konae/files/${crypto.randomUUID()}`, {
          method, headers: { host, cookie, "x-konae-admin-key": setupKey }, redirect: "manual",
        });
        expect(file.status).toBe(method === "GET" || method === "HEAD" ? 404 : 405);
        expect(file.headers.has("access-control-allow-origin")).toBe(false);
        await file.arrayBuffer();
      }
    }
  });

  test("MCP only accepts POST and the setup key never grants MCP access", async () => {
    for (const route of ["/mcp", "/_konae/mcp"]) {
      for (const method of ["GET", "HEAD", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
        const response = await fetch(url + route, { method });
        expect(response.status).toBe(405);
        expect(response.headers.has("access-control-allow-origin")).toBe(false);
        await response.arrayBuffer();
      }
    }
    const response = await fetch(`${url}/mcp`, { method: "POST", headers: { "x-konae-admin-key": setupKey, "content-type": "application/json" }, body: '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' });
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

async function publish(id: string, visibility: "public" | "private", files: Record<string, string>) {
  const project = { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility } };
  await client.call("project_create", { projectId: id, name: id });
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify({
    project: { ...project, files: { "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);", "actions.json": '{"version":1,"actions":{}}' } },
    visibility, artifact: { version: 1, entrypoint: "index.html", files },
  })), id);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    return (await client.call<{ hostname: string }>("release_apply", { planId: plan.planId })).hostname;
  } finally { await client.cancel(transfer.transferId); }
}

/** The CSP hash source of every inline script, computed the way a browser does. */
function inlineScriptHashes(html: string) {
  return [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script/gi)]
    .map(match => match[1].replace(/\r\n?/g, "\n"))
    .filter(Boolean)
    .map(text => `'sha256-${new Bun.CryptoHasher("sha256").update(text).digest("base64")}'`);
}

function directives(policy: string | null) {
  expect(policy).toBeTruthy();
  return new Map(policy!.split(";").map(part => part.trim().split(/\s+/)).map(([name, ...values]) => [name, values]));
}

const runtimeCall = (method: string) => JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: method, arguments: {} } });
const adminCall = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" });

describe("isolation of hosted code", () => {
  test("an app response carries a CSP that runs only its own inline scripts", async () => {
    const index = `<!doctype html><html><head><style>p{color:red}</style></head><body><p>CSP app</p><script data-konae-runtime>window.miniapp = {};</script><script type="module">document.querySelector("p").textContent = "ran";</script></body></html>`;
    const host = await publish("csp_app", "public", { "index.html": index, "logo.svg": '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>' });
    const page = await fetch(url, { headers: { host } });
    expect(await page.text()).toBe(index);
    const policy = directives(page.headers.get("content-security-policy"));
    const hashes = inlineScriptHashes(index);
    expect(hashes).toHaveLength(2);
    expect(policy.get("script-src")).toEqual(["'self'", ...hashes]);
    expect(policy.get("default-src")).toEqual(["'self'"]);
    expect(policy.get("connect-src")).toEqual(["'self'"]);
    expect(policy.get("frame-ancestors")).toEqual(["'none'"]);
    expect(policy.get("object-src")).toEqual(["'none'"]);
    expect(policy.get("base-uri")).toEqual(["'none'"]);
    expect(policy.get("img-src")).toEqual(["'self'", "blob:", "data:"]);
    expect(policy.get("script-src")).not.toContain("'unsafe-inline'");
    expect(policy.get("script-src")).not.toContain("'unsafe-eval'");
    expect(page.headers.get("x-frame-options")).toBe("DENY");
    expect(page.headers.get("strict-transport-security")).toBeNull();
    // A sub-resource and the 404 for an unknown path carry the policy too.
    // The SVG's own script gets no hash, so it cannot run when opened directly.
    for (const [route, status] of [["/logo.svg", 200], ["/some/client/route", 404]] as const) {
      const response = await fetch(url + route, { headers: { host } });
      expect(response.status).toBe(status);
      const scripts = directives(response.headers.get("content-security-policy")).get("script-src")!;
      expect(scripts).not.toContain("'unsafe-inline'");
      if (route === "/logo.svg") expect(scripts).toEqual(["'self'"]);
      expect(response.headers.get("x-frame-options")).toBe("DENY");
      await response.arrayBuffer();
    }
  });

  test("the private sign-in page runs only its own script and cannot be framed", async () => {
    const host = await publish("csp_private", "private", { "index.html": "<!doctype html><main>Private shell</main>" });
    const page = await fetch(url, { headers: { host } });
    const html = await page.text();
    expect(html).toContain("Sign in to this app");
    const policy = directives(page.headers.get("content-security-policy"));
    expect(policy.get("script-src")).toEqual(["'self'", ...inlineScriptHashes(html)]);
    expect(policy.get("frame-ancestors")).toEqual(["'none'"]);
  });

  test("the editor host cannot be framed by an app", async () => {
    const login = await fetch(`${url}/_konae/owner/login`);
    const html = await login.text();
    const policy = directives(login.headers.get("content-security-policy"));
    expect(policy.get("frame-ancestors")).toEqual(["'none'"]);
    expect(policy.get("script-src")).toEqual(["'self'", ...inlineScriptHashes(html)]);
    expect(login.headers.get("x-frame-options")).toBe("DENY");
    for (const response of [
      await fetch(`${url}/mcp`, { method: "POST", headers: { cookie, "content-type": "application/json" }, body: adminCall }),
      await fetch(`${url}/.well-known/oauth-authorization-server`),
      await fetch(`${url}/oauth/authorize`),
    ]) {
      expect(directives(response.headers.get("content-security-policy")).get("frame-ancestors")).toEqual(["'none'"]);
      expect(response.headers.get("x-frame-options")).toBe("DENY");
      await response.arrayBuffer();
    }
  });

  test("the editor's dev and preview servers forbid framing", () => {
    for (const server of [editorConfig.server, editorConfig.preview]) {
      expect(server?.headers).toEqual({ "Content-Security-Policy": "frame-ancestors 'none'", "X-Frame-Options": "DENY" });
    }
  });

  test("administrative MCP refuses an app origin before it reads any credential", async () => {
    const editor = new URL(url).host;
    for (const host of [editor, appHost]) {
      for (const headers of [{ cookie }, {}, { cookie, "x-konae-admin-key": setupKey }]) {
        const response = await fetch(`${url}/mcp`, {
          method: "POST", body: adminCall,
          headers: { ...headers, host, origin: `http://${appHost}`, "sec-fetch-site": "same-site", "content-type": "application/json" },
        });
        expect(response.status).toBe(403);
        expect((await response.json()).error).toBe("Administration refuses requests from app origins.");
      }
    }
    // A host under the app suffix that is not yet a deployment is refused the same way.
    const unknown = await fetch(`${url}/mcp`, { method: "POST", body: adminCall, headers: { cookie, origin: `http://${editor.replace("127.0.0.1", "unpublished-app.localhost")}`, "content-type": "application/json" } });
    expect(unknown.status).toBe(403);
    expect((await unknown.json()).error).toBe("Administration refuses requests from app origins.");
    // Owner sign-in, sign-out and the OAuth flow refuse an app origin too.
    for (const route of ["/_konae/owner/login", "/_konae/owner/signup", "/_konae/owner/logout", "/oauth/token"]) {
      const form = route === "/oauth/token";
      const response = await fetch(url + route, {
        method: "POST", body: form ? "grant_type=authorization_code" : "{}",
        headers: { cookie, origin: `http://${appHost}`, "content-type": form ? "application/x-www-form-urlencoded" : "application/json" },
      });
      expect(response.status).toBe(403);
      await response.arrayBuffer();
    }
    // The editor's own origin still reaches administration.
    const own = await fetch(`${url}/mcp`, { method: "POST", body: adminCall, headers: { cookie, origin: url, "content-type": "application/json" } });
    expect(own.status).toBe(200);
  });

  test("forged Origin and Host headers do not reach administration", async () => {
    const editor = new URL(url).host;
    const cases = [
      { host: appHost, origin: url },
      { host: appHost, origin: `http://${appHost}` },
      { host: appHost },
      { host: `unpublished-app.localhost:${new URL(url).port}`, origin: `http://unpublished-app.localhost:${new URL(url).port}` },
      { host: editor, origin: "https://foreign.example" },
      { host: editor, origin: "null" },
      { host: "foreign.example", origin: "http://foreign.example" },
    ];
    for (const headers of cases) {
      const response = await fetch(`${url}/mcp`, { method: "POST", body: adminCall, headers: { ...headers, cookie, "content-type": "application/json" } });
      expect(response.status).toBe(403);
      await response.arrayBuffer();
    }
  });

  test("an app cannot call another app's runtime or reuse its session", async () => {
    const other = await publish("csp_other", "public", { "index.html": "<!doctype html><main>Other app</main>" });
    const signup = await fetch(`${url}/_konae/auth/signup`, {
      method: "POST", headers: { host: appHost, origin: `http://${appHost}`, "content-type": "application/json" },
      body: JSON.stringify({ email: "member@cross-app.test", password: "cross-app-password-2026" }),
    });
    expect(signup.status).toBe(200);
    const member = signup.headers.get("set-cookie")!.split(";")[0];
    const own = await fetch(`${url}/_konae/mcp`, { method: "POST", headers: { host: appHost, cookie: member, "content-type": "application/json" }, body: runtimeCall("auth_session") });
    expect((await own.json()).result.structuredContent.result.role).toBe("member");
    // A page on one app posting to another app's runtime is refused on origin.
    const crossed = await fetch(`${url}/_konae/mcp`, {
      method: "POST", body: runtimeCall("auth_session"),
      headers: { host: other, origin: `http://${appHost}`, "sec-fetch-site": "same-site", cookie: member, "content-type": "application/json" },
    });
    expect(crossed.status).toBe(403);
    // The session of one app is unknown to every other app.
    const reused = await fetch(`${url}/_konae/mcp`, { method: "POST", headers: { host: other, cookie: member, "content-type": "application/json" }, body: runtimeCall("auth_session") });
    expect((await reused.json()).result.structuredContent.result.role).toBe("anonymous");
    // Sign-in on another app's host is refused from a foreign app origin as well.
    const login = await fetch(`${url}/_konae/auth/login`, {
      method: "POST", headers: { host: other, origin: `http://${appHost}`, "content-type": "application/json" },
      body: JSON.stringify({ email: "member@cross-app.test", password: "cross-app-password-2026" }),
    });
    expect(login.status).toBe(403);
  });
});
