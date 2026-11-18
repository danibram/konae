import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const fixtureEnvironment = Object.fromEntries(["PATH", "HOME", "TMPDIR", "LANG", "LC_ALL"].flatMap((name) => process.env[name] === undefined ? [] : [[name, process.env[name]]]));
export async function testInstallation(binary, settings = {}) {
  const root = settings.root ?? await mkdtemp(join(tmpdir(), "konae-gate27-"));
  const portAllocator = Bun.serve({ port: settings.port ?? 0, hostname: "127.0.0.1", fetch: () => new Response() });
  const port = portAllocator.port;
  portAllocator.stop(true);
  const origin = `http://127.0.0.1:${port}`;
  const options = {
    ...fixtureEnvironment,
    KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_DATA_DIR: root,
    KONAE_PUBLIC_ORIGIN: origin, KONAE_HOST_SUFFIX: "localhost", KONAE_ACCOUNT_VERIFICATION: "local",
    KONAE_SETUP_KEY: "local-development-key", KONAE_MAIL_TRANSPORT: "development",
    ...settings.env,
  };
  const child = Bun.spawn([binary], { env: options, stdout: "ignore", stderr: "inherit" });
  let requestId = 0;
  const close = async () => {
    if (child.exitCode === null) child.kill();
    await child.exited;
    await rm(root, { force: true, recursive: true });
  };

  async function request(path, { method = "GET", cookie, originHeader = origin, body, headers = {} } = {}) {
    const payload = body === undefined || ArrayBuffer.isView(body) ? body : JSON.stringify(body);
    const response = await fetch(origin + path, {
      method, redirect: "manual", signal: AbortSignal.timeout(30000),
      headers: {
        origin: originHeader, ...(cookie ? { cookie } : {}),
        ...(payload !== undefined ? { "content-type": ArrayBuffer.isView(body) ? "application/octet-stream" : "application/json" } : {}),
        ...headers,
      },
      ...(payload === undefined ? {} : { body: payload }),
    });
    const text = await response.text();
    let json;
    try { json = JSON.parse(text); } catch {}
    return { status: response.status, json, text, headers: response.headers };
  }

  async function owner(label, signIn = false) {
    const response = await request(signIn ? "/_konae/owner/login" : "/_konae/owner/signup", {
      method: "POST",
      body: { email: `gate27-${label}@fixture.test`, password: "only-a-fixture-password", ...(!signIn ? {setupKey:options.KONAE_SETUP_KEY} : {}) },
    });
    if (response.status !== 200) throw Error(`Fixture owner signup failed with ${response.status}`);
    const cookie = response.headers.get("set-cookie")?.split(";")[0];
    if (!cookie) throw Error("Fixture owner has no session");
    return { cookie };
  }

  async function mcp(cookie, name, args) {
    const response = await request("/mcp", {
      method: "POST", cookie,
      headers: { accept: "application/json, text/event-stream" },
      body: { jsonrpc: "2.0", id: ++requestId, method: "tools/call", params: { name, arguments: args } },
    });
    const frame = response.json;
    if (response.status !== 200 || frame?.result?.isError || !frame?.result) throw Error(`${name} failed with ${response.status}: ${response.text}`);
    return frame.result.structuredContent?.result ?? JSON.parse(frame.result.content[0].text);
  }

  try {
    const deadline = Date.now() + 30_000;
    while (Date.now() < deadline) {
      const ready = await fetch(origin + "/_konae/health").then((response) => response.ok, () => false);
      if (ready) return { origin, request, owner, mcp, close };
      if (child.exitCode !== null) throw Error("Local fixture exited before health");
      await Bun.sleep(100);
    }
    throw Error("Timed out waiting for local fixture");
  } catch (error) {
    await close();
    throw error;
  }
}
