import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { exportLocal, publishLocal } from "../../scripts/littlebox";
import { McpClient, type Transfer } from "../../src/mcp/client";

// Run manually with `bun run tests/smoke/portable-export.ts --port 19450`.
const portIndex = process.argv.indexOf("--port");
const port = portIndex < 0 ? 19450 : Number(process.argv[portIndex + 1]);
if (!Number.isInteger(port) || port < 1024 || port > 65534) throw Error("Choose a source port between 1024 and 65534. The target uses the next port.");
const repo = path.resolve(import.meta.dir, "../..");
const root = await mkdtemp(path.join(tmpdir(), "littlebox-portable-proof-"));
const key = crypto.randomUUID() + crypto.randomUUID();
const password = crypto.randomUUID();
const sourceUrl = `http://127.0.0.1:${port}`;
const targetUrl = `http://127.0.0.1:${port + 1}`;
const binary = path.join(repo, "kernel-rs/target/debug/littlebox-kernel");
const processes: ReturnType<typeof Bun.spawn>[] = [];
const attachment = new TextEncoder().encode("Attachment survives export");
const sources = {
  "app.js": "document.body.dataset.ready='true';",
  "index.html": "<main>Portable demo</main>",
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY,user_id TEXT NOT NULL,body TEXT NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.list": {
      kind: "query", access: "authenticated", statement: "SELECT id,body FROM notes WHERE user_id=?1 ORDER BY id LIMIT 50",
      bindings: [{ source: "actor", field: "id" }],
    },
    "notes.add": {
      kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(user_id,body) VALUES(?1,?2)",
      bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }],
    },
  } }),
};

type Deployment = { id: string; hostname: string; signupPolicy: { mode: string; allowlist: string[] } };

async function command(args: string[], cwd = repo) {
  const child = Bun.spawn(args, { cwd, stdout: "pipe", stderr: "pipe" });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (status !== 0) throw Error(`${args.join(" ")} failed.\n${stderr}\n${stdout}`);
  return stdout;
}

async function startKernel(url: string, dataDirectory: string) {
  const logPath = path.join(root, `kernel-${new URL(url).port}.log`);
  const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("LITTLEBOX_") && !name.startsWith("VITE_")));
  const child = Bun.spawn([binary], {
    env: {
      ...environment, LITTLEBOX_BIND: "127.0.0.1", LITTLEBOX_KERNEL_PORT: new URL(url).port,
      LITTLEBOX_PUBLIC_ORIGIN: url, LITTLEBOX_HOST_SUFFIX: "localhost",
      LITTLEBOX_DATA_DIR: dataDirectory, LITTLEBOX_ADMIN_KEY: key,
    },
    stdout: "ignore", stderr: Bun.file(logPath),
  });
  processes.push(child);
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_littlebox/health`)).ok) return child; } catch {}
    if (child.exitCode !== null) throw Error(`Kernel stopped before startup.\n${await readFile(logPath, "utf8")}`);
    await Bun.sleep(100);
  }
  throw Error(`Kernel did not start.\n${await readFile(logPath, "utf8")}`);
}

async function owner(url: string) {
  const response = await fetch(`${url}/_littlebox/owner/signup`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ email: "owner@proof.test", password: key, setupKey: key }),
  });
  assert.equal(response.status, 200, await response.text());
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  return new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

function runtime(url: string, hostname: string, cookie?: string) {
  return new McpClient(`${url}/_littlebox/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers);
    headers.set("host", hostname);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

async function sign(url: string, hostname: string, operation: "signup" | "login", email = "writer@proof.test", expected = 200) {
  const response = await fetch(`${url}/_littlebox/auth/${operation}`, {
    method: "POST", headers: { host: hostname, "content-type": "application/json" },
    body: JSON.stringify({ email, password }),
  });
  assert.equal(response.status, expected, await response.text());
  return response.headers.get("set-cookie")?.split(";")[0];
}

async function sourceProject(name: string) {
  const directory = path.join(root, name);
  await mkdir(directory);
  const config = { url: sourceUrl, project: { id: name, name, slug: name, manifest: { version: 1, visibility: "public" } } };
  for (const [filename, contents] of Object.entries({ ...sources, "littlebox.json": JSON.stringify(config) })) {
    await writeFile(path.join(directory, filename), contents);
  }
  return { directory, config };
}

async function unpack(archive: string, directory: string) {
  await mkdir(directory);
  await command(["tar", "-xf", archive, "-C", directory]);
  await symlink(path.join(repo, "node_modules"), path.join(directory, "node_modules"), "dir");
  await writeFile(path.join(directory, ".env"), `LITTLEBOX_PORT=${port + 1}\nLITTLEBOX_ADMIN_KEY=${key}\n`, { mode: 0o600 });
}

async function deployment(directory: string, dataDirectory: string): Promise<Deployment> {
  const marker = JSON.parse(await readFile(path.join(directory, "littlebox/deployment.json"), "utf8"));
  return JSON.parse(await readFile(path.join(dataDirectory, "deployments", marker.deploymentId, "deployment.json"), "utf8"));
}

try {
  await command(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"]);
  const sourceKernel = await startKernel(sourceUrl, path.join(root, "source-data"));
  const admin = await owner(sourceUrl);
  const { token } = await admin.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish"], ttlSeconds: 3600 });
  const source = await sourceProject("portable-demo");
  const published = await publishLocal(source.directory, source.config, token);
  const inspection = await admin.call<{ deployment: Deployment }>("project_inspect", { projectId: source.config.project.id });
  const originalCookie = await sign(sourceUrl, inspection.deployment.hostname, "signup");
  const writer = runtime(sourceUrl, inspection.deployment.hostname, originalCookie);
  await writer.call("actions_invoke", { name: "notes.add", input: { body: "Before the move" } });
  const upload = await writer.upload(attachment);
  const saved = await writer.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain" });
  await writer.cancel(upload.transferId);
  await admin.call("signup_policy_set", { deploymentId: published.id, mode: "closed" });
  const closedArchive = path.join(root, "closed.tar");
  await exportLocal(source.directory, source.config, closedArchive, token);

  const allowlistSource = await sourceProject("portable-allowlist");
  const allowlistPublished = await publishLocal(allowlistSource.directory, allowlistSource.config, token);
  await admin.call("signup_policy_set", { deploymentId: allowlistPublished.id, mode: "allowlist", allowlist: ["@team.test"] });
  const allowlistArchive = path.join(root, "allowlist.tar");
  await exportLocal(allowlistSource.directory, allowlistSource.config, allowlistArchive, token);
  sourceKernel.kill();
  await sourceKernel.exited;

  const kit = path.join(root, "kit");
  const targetData = path.join(kit, "data");
  await unpack(closedArchive, kit);
  await command(["node", "scripts/build.mjs"], kit);
  // This compiles the exported sources. Only Cargo's dependency cache is shared.
  await command(["cargo", "build", "--offline", "--locked", "--manifest-path", "kernel-rs/Cargo.toml", "--target-dir", path.join(repo, "kernel-rs/target")], kit);
  await startKernel(targetUrl, targetData);
  await command(["node", "scripts/deploy.mjs"], kit);
  const moved = await deployment(kit, targetData);
  assert.deepEqual(moved.signupPolicy, { mode: "closed", allowlist: [] });
  await sign(targetUrl, moved.hostname, "signup", "stranger@proof.test", 403);
  const cookie = await sign(targetUrl, moved.hostname, "login");
  const targetWriter = runtime(targetUrl, moved.hostname, cookie);
  assert.deepEqual(await targetWriter.call("actions_invoke", { name: "notes.list" }), [{ id: 1, body: "Before the move" }]);
  assert.deepEqual(await targetWriter.download(await targetWriter.call<Transfer>("files_read", { fileId: saved.id })), attachment);
  const oldSession = await runtime(targetUrl, moved.hostname, originalCookie).call<{ role: string }>("auth_session");
  assert.equal(oldSession.role, "anonymous");
  await targetWriter.call("actions_invoke", { name: "notes.add", input: { body: "After the move" } });
  await writeFile(path.join(kit, "app/app.js"), "document.body.dataset.rebuilt='true';");
  await command(["node", "scripts/build.mjs"], kit);
  await command(["node", "scripts/deploy.mjs"], kit);
  assert.equal((await targetWriter.call<unknown[]>("actions_invoke", { name: "notes.list" })).length, 2);
  assert.deepEqual(await targetWriter.download(await targetWriter.call<Transfer>("files_read", { fileId: saved.id })), attachment);
  assert.deepEqual((await deployment(kit, targetData)).signupPolicy, moved.signupPolicy);

  const allowlistKit = path.join(root, "allowlist-kit");
  await unpack(allowlistArchive, allowlistKit);
  await command(["node", "scripts/deploy.mjs"], allowlistKit);
  const allowed = await deployment(allowlistKit, targetData);
  assert.deepEqual(allowed.signupPolicy, { mode: "allowlist", allowlist: ["@team.test"] });
  await sign(targetUrl, allowed.hostname, "signup", "outsider@proof.test", 403);
  await sign(targetUrl, allowed.hostname, "signup", "member@team.test");
  console.log("Passed fresh-kit compilation, account/password and data/file migration, session invalidation, republish persistence, Closed import and Allowlist import.");
} finally {
  for (const child of processes) if (child.exitCode === null) child.kill();
  await Promise.all(processes.map((child) => child.exited));
  await rm(root, { recursive: true, force: true });
}
