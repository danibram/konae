import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createServer, type ViteDevServer } from "vite";
import { devOwnerPlugin } from "./dev-owner";

const binary = process.env.KONAE_KERNEL_BIN;
const integrationTest = binary ? test : test.skip;
let root: string;
let kernelUrl: string;
let kernel: ReturnType<typeof Bun.spawn>;
const editors: ViteDevServer[] = [];

function freePort() {
  const probe = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch: () => new Response() });
  const port = probe.port!;
  probe.stop(true);
  return port;
}

beforeAll(async () => {
  if (!binary) return;
  root = await mkdtemp(path.join(tmpdir(), "konae-dev-owner-"));
  kernelUrl = `http://127.0.0.1:${freePort()}`;
  kernel = Bun.spawn([path.resolve(binary)], {
    env: { ...process.env, KONAE_DATA_DIR: root, KONAE_KERNEL_PORT: new URL(kernelUrl).port,
      KONAE_PUBLIC_ORIGIN: kernelUrl, KONAE_SETUP_KEY: "local-development-key", KONAE_ACCOUNT_VERIFICATION: "local" },
    stdout: "ignore", stderr: "pipe",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    if (kernel.exitCode !== null) throw new Error("The test Kernel stopped during startup.");
    try {
      if ((await fetch(`${kernelUrl}/_konae/owner/session`)).ok) return;
    } catch { /* The Kernel has not started listening yet. */ }
    await Bun.sleep(25);
  }
  throw new Error("The test Kernel did not start.");
});

afterAll(async () => {
  await Promise.all(editors.map((editor) => editor.close()));
  if (kernel) { kernel.kill(); await kernel.exited; }
  if (root) await rm(root, { recursive: true, force: true });
});

async function editor(autoLogin = true, target = kernelUrl, host = "127.0.0.1") {
  const server = await createServer({
    configFile: false, root,
    plugins: [devOwnerPlugin(target, autoLogin ? path.join(root, ".env.dev-owner") : undefined)],
    server: { host, port: freePort(), strictPort: true,
      proxy: { "/_konae": { target: kernelUrl }, "/mcp": { target: kernelUrl } } },
  });
  editors.push(server);
  await server.listen();
  return { server, url: `http://127.0.0.1:${server.config.server.port}` };
}

integrationTest("development sign-in sets a normal owner cookie and persists the owner across restarts", async () => {
  const first = await editor();
  const response = await fetch(`${first.url}/_konae/owner/session`);
  expect(response.status).toBe(200);
  const session = await response.json();
  expect(session).toMatchObject({ signedIn: true, displayName: "dev@konae.local" });
  const cookie = response.headers.get("set-cookie")!;
  expect(cookie).toContain("HttpOnly");
  expect(cookie).toContain("SameSite=Strict");
  expect((await stat(path.join(root, ".env.dev-owner"))).mode & 0o777).toBe(0o600);
  const readBack = await fetch(`${first.url}/_konae/owner/session`, { headers: { cookie: cookie.split(";")[0] } });
  expect(await readBack.json()).toEqual(session);
  expect(readBack.headers.get("set-cookie")).toBeNull();
  const projects = await fetch(`${first.url}/mcp`, {
    method: "POST", headers: { "content-type": "application/json", cookie: cookie.split(";")[0] },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "projects_list", arguments: {} } }),
  });
  expect(projects.status).toBe(200);
  expect((await projects.json()).result.isError).not.toBe(true);
  await first.server.close();
  const restarted = await editor();
  expect(await (await fetch(`${restarted.url}/_konae/owner/session`)).json()).toEqual(session);
});

integrationTest("development credentials cannot be downloaded through Vite", async () => {
  const { url } = await editor();
  await fetch(`${url}/_konae/owner/session`);
  for (const route of ["/.env.dev-owner", `/@fs${root}/.env.dev-owner`]) {
    expect((await fetch(`${url}${route}`)).status).toBe(403);
  }
});

integrationTest("development sign-in keeps another owner's session", async () => {
  const signup = await fetch(`${kernelUrl}/_konae/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "other@example.test", password: "Other-test-password", setupKey: "local-development-key" }),
  });
  expect(signup.status).toBe(200);
  const { url } = await editor();
  const response = await fetch(`${url}/_konae/owner/session`, { headers: { cookie: signup.headers.get("set-cookie")!.split(";")[0] } });
  expect(await response.json()).toMatchObject({ signedIn: true, displayName: "other@example.test" });
  expect(response.headers.get("set-cookie")).toBeNull();
});

integrationTest("app hosts and foreign origins cannot start a development owner session", async () => {
  const { url } = await editor();
  for (const headers of [{ host: `app.localhost:${new URL(url).port}` }, { origin: "https://example.test" }, { "sec-fetch-site": "cross-site" }]) {
    const response = await fetch(`${url}/_konae/owner/session`, { headers });
    expect(response.status).toBe(403);
    expect(response.headers.get("set-cookie")).toBeNull();
  }
});

integrationTest("manual sign-in remains available without the development option", async () => {
  const { url } = await editor(false);
  const response = await fetch(`${url}/_konae/owner/session`);
  expect(await response.json()).toEqual({ signedIn: false });
  expect(response.headers.get("set-cookie")).toBeNull();
});

integrationTest("development sign-in refuses a remote Kernel or a public editor bind", async () => {
  await expect(editor(true, "https://example.test")).rejects.toThrow("requires a Kernel on localhost");
  await expect(editor(true, kernelUrl, "0.0.0.0")).rejects.toThrow("bind to 127.0.0.1");
});
