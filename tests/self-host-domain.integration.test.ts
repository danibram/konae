import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { exportLocal, publishLocal } from "../scripts/konae";
import { McpClient } from "../src/mcp/client";

// An app leaves its installation as a self-host kit with its data and answers
// on its own domain there. The kit's Kernel has an HTTPS public origin, the
// deploy script signs in on that origin's host and verifies the app's hostname
// against a DNS stub, and the app's accounts and rows work on the alias.
const repo = path.resolve(import.meta.dir, "..");
const binary = path.join(repo, "kernel-rs/target/debug/konae-kernel");
const sourcePort = Number(process.env.KONAE_SELFHOST_TEST_PORT) || 19620 + Math.floor(Math.random() * 40) * 2;
const sourceUrl = `http://127.0.0.1:${sourcePort}`;
const kitPort = sourcePort + 1;
const setupKey = crypto.randomUUID() + crypto.randomUUID();
const password = "self-host-domain-password";
const kernelHost = "konae.example.org";
const appHost = "notes.example.org";
const processes: ReturnType<typeof Bun.spawn>[] = [];
let root: string;
let dns: Awaited<ReturnType<typeof Bun.udpSocket>>;
// The TXT values the stub serves for _konae.notes.example.org.
let txt: string[] = [];

const sources = {
  "index.html": "<main>Self-hosted notes</main><script type=\"module\" src=\"./app.js\"></script>",
  "app.js": "document.body.dataset.ready='true';",
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY,user_id TEXT NOT NULL,body TEXT NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,body FROM notes WHERE user_id=?1 ORDER BY id LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "notes.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(user_id,body) VALUES(?1,?2)", bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }] },
  } }),
};

// Answers a TXT query for one name, NXDOMAIN for anything else.
function dnsAnswer(query: Uint8Array) {
  let at = 12;
  const labels: string[] = [];
  while (query[at] !== 0) { labels.push(new TextDecoder().decode(query.subarray(at + 1, at + 1 + query[at]))); at += query[at] + 1; }
  const question = query.subarray(12, at + 5);
  const name = labels.join(".");
  const values = name === `_konae.${appHost}` ? txt : [];
  const header = new Uint8Array([query[0], query[1], 0x81, values.length ? 0x80 : 0x83, 0, 1, 0, values.length, 0, 0, 0, 0]);
  const records = values.map((value) => {
    const text = new TextEncoder().encode(value);
    return Buffer.concat([new Uint8Array([0xc0, 0x0c, 0, 16, 0, 1, 0, 0, 0, 60, 0, text.length + 1, text.length]), text]);
  });
  return Buffer.concat([header, question, ...records]);
}

function cleanEnvironment() {
  return Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KONAE_") && !name.startsWith("VITE_")));
}

async function command(args: string[], cwd = repo) {
  const child = Bun.spawn(args, { cwd, stdout: "pipe", stderr: "pipe", env: cleanEnvironment() });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (status !== 0) throw Error(`${args.join(" ")} failed.\n${stderr}\n${stdout}`);
  return stdout;
}

async function startKernel(port: number, environment: Record<string, string>) {
  const logPath = path.join(root, `kernel-${port}.log`);
  const child = Bun.spawn([binary], {
    env: { ...cleanEnvironment(), KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: String(port), KONAE_DNS_RESOLVER: `127.0.0.1:${dns.port}`, ...environment },
    stdout: "ignore", stderr: Bun.file(logPath),
  });
  processes.push(child);
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`http://127.0.0.1:${port}/_konae/health`)).ok) return child; } catch {}
    if (child.exitCode !== null) throw Error(`Kernel stopped before startup.\n${await readFile(logPath, "utf8")}`);
    await Bun.sleep(100);
  }
  throw Error(`Kernel did not start.\n${await readFile(logPath, "utf8")}`);
}

// Bun's fetch sends the Host it is given, as the operator's proxy would.
function at(port: number, host: string, pathname: string, init: RequestInit = {}) {
  const headers = new Headers(init.headers);
  headers.set("host", host);
  return fetch(`http://127.0.0.1:${port}${pathname}`, { ...init, headers });
}

beforeAll(async () => {
  expect(await Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" }).exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-self-host-domain-"));
  dns = await Bun.udpSocket({ hostname: "127.0.0.1", socket: { data(socket, data, port, address) { socket.send(dnsAnswer(new Uint8Array(data)), port, address); } } });
}, 300_000);

afterAll(async () => {
  for (const child of processes) if (child.exitCode === null) child.kill();
  await Promise.all(processes.map((child) => child.exited));
  dns?.close();
  if (root) await rm(root, { recursive: true, force: true });
});

test("an exported kit serves the app with its accounts and rows on the app's own domain", async () => {
  // The app on its first installation. It gets one account, one row and a claimed hostname below.
  const source = await startKernel(sourcePort, { KONAE_DATA_DIR: path.join(root, "source-data"), KONAE_PUBLIC_ORIGIN: sourceUrl, KONAE_HOST_SUFFIX: "localhost", KONAE_ADMIN_KEY: setupKey });
  const signup = await fetch(`${sourceUrl}/_konae/owner/signup`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "owner@self-host.test", password: setupKey, setupKey }) });
  expect(signup.status).toBe(200);
  const ownerCookie = signup.headers.get("set-cookie")!.split(";")[0];
  const admin = new McpClient(`${sourceUrl}/mcp`, { fetch: (input, init) => { const headers = new Headers(init?.headers); headers.set("cookie", ownerCookie); return fetch(input, { ...init, headers }); } });
  const { token } = await admin.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish", "export_data", "manage_lifecycle"], ttlSeconds: 3600 });

  // A project published from its folder.
  const projectId = "self-host-notes";
  const appDirectory = path.join(root, "app");
  await mkdir(appDirectory);
  for (const [name, text] of Object.entries(sources)) await writeFile(path.join(appDirectory, name), text);
  const config = { url: sourceUrl, project: { id: projectId, name: "Notes", slug: "notes", manifest: { version: 1, visibility: "public" } } };
  await writeFile(path.join(appDirectory, "konae.json"), JSON.stringify(config));
  const published = await publishLocal(appDirectory, config, token, () => {});
  await admin.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
  const generated = new URL(published.url).host.split(":")[0];
  const writerSignup = await at(sourcePort, generated, "/_konae/auth/signup", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "writer@self-host.test", password }) });
  expect(writerSignup.status).toBe(200);
  const writer = new McpClient(`${sourceUrl}/_konae/mcp`, { fetch: (input, init) => { const headers = new Headers(init?.headers); headers.set("host", generated); headers.set("cookie", writerSignup.headers.get("set-cookie")!.split(";")[0]); return fetch(input, { ...init, headers }); } });
  await writer.call("actions_invoke", { name: "notes.add", input: { body: "Written before the move" } });
  expect((await admin.call<{ status: string }>("domain_set", { deploymentId: published.id, hostname: appHost })).status).toBe("pending");
  const archive = path.join(root, "kit.tar");
  await exportLocal(appDirectory, config, archive, token);
  source.kill();
  await source.exited;

  // The kit on another machine, configured for the domain before its first deploy.
  const kit = path.join(root, "kit");
  await mkdir(kit);
  await command(["tar", "-xf", archive, "-C", kit]);
  await symlink(path.join(repo, "node_modules"), path.join(kit, "node_modules"), "dir");
  expect(await readFile(path.join(kit, "compose.yaml"), "utf8")).toContain("KONAE_PUBLIC_ORIGIN: ${KONAE_PUBLIC_ORIGIN:-http://localhost:${KONAE_PORT:-8787}}");
  const kitEnv = { KONAE_PORT: String(kitPort), KONAE_ADMIN_KEY: setupKey, KONAE_PUBLIC_ORIGIN: `https://${kernelHost}`, KONAE_HOST_SUFFIX: "apps.example.org", KONAE_APP_DOMAIN: appHost };
  await writeFile(path.join(kit, ".env"), Object.entries(kitEnv).map(([name, value]) => `${name}=${value}\n`).join(""), { mode: 0o600 });
  await command(["node", "scripts/build.mjs"], kit);
  // The README's native start: .env, then the data directory and the port.
  await startKernel(kitPort, { ...kitEnv, KONAE_DATA_DIR: path.join(kit, "data") });

  // The record still holds the token the first installation would have needed.
  txt = ["konae-verify-first-installation"];
  const first = await command(["node", "scripts/deploy.mjs"], kit);
  expect(first).toContain(`The TXT record at _konae.${appHost} holds "konae-verify-first-installation"`);
  expect(first).toContain("https://notes-");
  expect(first).toContain(".apps.example.org");
  const record = first.match(/Set TXT (\S+) to ([^\s.]+)\./);
  expect(record?.[1]).toBe(`_konae.${appHost}`);
  expect(first).toContain("Then run npm run deploy again.");
  expect((await at(kitPort, appHost, "/")).status).toBe(404);

  txt = ["konae-verify-first-installation", record![2]];
  const second = await command(["node", "scripts/deploy.mjs"], kit);
  expect(second).toContain(`The app also answers at https://${appHost}.`);

  // What the proxy forwards for https://notes.example.org.
  const page = await at(kitPort, appHost, "/");
  expect(page.status).toBe(200);
  expect(await page.text()).toContain("Self-hosted notes");
  expect(page.headers.get("strict-transport-security")).toBe("max-age=31536000");
  const login = await at(kitPort, appHost, "/_konae/auth/login", { method: "POST", headers: { "content-type": "application/json", origin: `https://${appHost}` }, body: JSON.stringify({ email: "writer@self-host.test", password }) });
  expect(login.status).toBe(200);
  const session = login.headers.get("set-cookie")!;
  expect(session).toStartWith("__Host-konae_app_session=");
  expect(session).toContain("Secure");
  const onAlias = new McpClient(`http://127.0.0.1:${kitPort}/_konae/mcp`, { fetch: (input, init) => { const headers = new Headers(init?.headers); headers.set("host", appHost); headers.set("origin", `https://${appHost}`); headers.set("cookie", session.split(";")[0]); return fetch(input, { ...init, headers }); } });
  expect(await onAlias.call("actions_invoke", { name: "notes.list" })).toEqual([{ id: 1, body: "Written before the move" }]);
  await onAlias.call("actions_invoke", { name: "notes.add", input: { body: "Written on the alias" } });

  // Owner sign-in stays on the Kernel's host, never on the app's.
  const ownerOnApp = await at(kitPort, appHost, "/_konae/owner/login", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "owner@selfhost.local", password: setupKey }) });
  expect(ownerOnApp.status).toBe(403);

  // A later deploy publishes new code and keeps the rows and the alias.
  await writeFile(path.join(kit, "app/app.js"), "document.body.dataset.rebuilt='true';");
  await command(["node", "scripts/build.mjs"], kit);
  const third = await command(["node", "scripts/deploy.mjs"], kit);
  expect(third).toContain(`The app also answers at https://${appHost}.`);
  expect((await onAlias.call<unknown[]>("actions_invoke", { name: "notes.list" })).length).toBe(2);
}, 300_000);
