import assert from "node:assert/strict";
import { Database } from "bun:sqlite";
import { mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { exportLocal, publishLocal } from "../../scripts/konae";
import { McpClient, type Transfer } from "../../src/mcp/client";

// Delivery H's quota proof: an app whose database reached its 256 MiB cap,
// with 20 MiB of files, leaves through `konae export --with-data` and
// arrives through the kit's `npm run deploy`, with peak memory sampled on
// every process that touches the package.
// Run with `bun run tests/smoke/package-quota.ts --port 19460`; the target
// Kernel uses the next port. It needs about 2 GB of free disk.
const portIndex = process.argv.indexOf("--port");
const port = portIndex < 0 ? 19460 : Number(process.argv[portIndex + 1]);
if (!Number.isInteger(port) || port < 1024 || port > 65534) throw Error("Choose a source port between 1024 and 65534. The target uses the next port.");
const repo = path.resolve(import.meta.dir, "../..");
const root = await mkdtemp(path.join(tmpdir(), "konae-package-quota-"));
const key = crypto.randomUUID() + crypto.randomUUID();
const password = crypto.randomUUID();
const sourceUrl = `http://127.0.0.1:${port}`;
const targetUrl = `http://127.0.0.1:${port + 1}`;
const binary = path.join(repo, "kernel-rs/target/debug/konae-kernel");
const processes: ReturnType<typeof Bun.spawn>[] = [];
const mib = 1024 * 1024;
const sources = {
  "app.js": "document.body.dataset.ready='true';",
  "index.html": "<main>Quota proof</main>",
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY,user_id TEXT NOT NULL,body TEXT NOT NULL); CREATE TABLE IF NOT EXISTS blobs(id INTEGER PRIMARY KEY,user_id TEXT NOT NULL,body BLOB NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,body FROM notes WHERE user_id=?1 ORDER BY id LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "notes.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(user_id,body) VALUES(?1,?2)", bindings: [{ source: "actor", field: "id" }, { source: "input", key: "body", type: "string", maxLength: 200 }] },
    "blobs.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO blobs(user_id,body) VALUES(?1,zeroblob(65536))", bindings: [{ source: "actor", field: "id" }] },
  } }),
};

/** Samples a process's resident set with `ps` every 100 ms and keeps the peak in MiB. */
function sampleRss(pid: number) {
  let peak = 0;
  let running = true;
  const done = (async () => {
    while (running) {
      const child = Bun.spawn(["ps", "-o", "rss=", "-p", String(pid)], { stdout: "pipe", stderr: "ignore" });
      const kib = Number((await new Response(child.stdout).text()).trim());
      if (Number.isFinite(kib)) peak = Math.max(peak, kib / 1024);
      await Bun.sleep(100);
    }
  })();
  return async () => { running = false; await done; return Math.round(peak); };
}

async function command(args: string[], cwd = repo, sample?: (pid: number) => () => Promise<number>) {
  const child = Bun.spawn(args, { cwd, stdout: "pipe", stderr: "pipe" });
  const stop = sample?.(child.pid);
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  const peak = await stop?.();
  if (status !== 0) throw Error(`${args.join(" ")} failed.\n${stderr}\n${stdout}`);
  return { stdout, peak };
}

async function startKernel(url: string, dataDirectory: string) {
  const logPath = path.join(root, `kernel-${new URL(url).port}.log`);
  const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KONAE_") && !name.startsWith("VITE_")));
  const child = Bun.spawn([binary], {
    env: { ...environment, KONAE_BIND: "127.0.0.1", KONAE_KERNEL_PORT: new URL(url).port, KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost", KONAE_DATA_DIR: dataDirectory, KONAE_ADMIN_KEY: key },
    stdout: "ignore", stderr: Bun.file(logPath),
  });
  processes.push(child);
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) return child; } catch {}
    if (child.exitCode !== null) throw Error(`Kernel stopped before startup.\n${await readFile(logPath, "utf8")}`);
    await Bun.sleep(100);
  }
  throw Error(`Kernel did not start.\n${await readFile(logPath, "utf8")}`);
}

async function ownerSession(url: string, email: string, route: "signup" | "login") {
  const response = await fetch(`${url}/_konae/owner/${route}`, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email, password: key, setupKey: key }),
  });
  assert.equal(response.status, 200, await response.text());
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  return new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

function runtime(url: string, hostname: string, cookie?: string) {
  return new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

async function sign(url: string, hostname: string, operation: "signup" | "login") {
  const response = await fetch(`${url}/_konae/auth/${operation}`, {
    method: "POST", headers: { host: hostname, "content-type": "application/json" }, body: JSON.stringify({ email: "writer@quota.test", password }),
  });
  assert.equal(response.status, 200, await response.text());
  return response.headers.get("set-cookie")!.split(";")[0];
}

const sql = (client: McpClient, deploymentId: string, statement: string) =>
  client.call<{ rows?: Record<string, unknown>[] }>("database_execute", { deploymentId, operation: "sql", input: { sql: statement } });

/** Inserts rows of `size` random bytes, `count` per statement, until the database refuses with its size cap. */
async function fill(client: McpClient, deploymentId: string, size: number, count: number) {
  for (;;) {
    try {
      await sql(client, deploymentId, `INSERT INTO blobs(user_id,body) WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<${count}) SELECT 'fill',randomblob(${size}) FROM c`);
    } catch (error) {
      if (error instanceof Error && /full/.test(error.message)) return;
      throw error;
    }
  }
}

async function counts(client: McpClient, deploymentId: string) {
  const result = await sql(client, deploymentId, "SELECT (SELECT COUNT(*) FROM blobs) blobs, (SELECT SUM(length(body)) FROM blobs) bytes, (SELECT COUNT(*) FROM notes) notes");
  return JSON.stringify(result.rows);
}

try {
  await command(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"]);
  const sourceData = path.join(root, "source-data");
  const sourceKernel = await startKernel(sourceUrl, sourceData);
  const admin = await ownerSession(sourceUrl, "owner@quota.test", "signup");
  const { token } = await admin.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish", "export_data"], ttlSeconds: 7200 });
  const directory = path.join(root, "quota-app");
  await mkdir(directory);
  const config = { url: sourceUrl, project: { id: "quota-app", name: "Quota app", slug: "quota-app", manifest: { version: 1, visibility: "private" } } };
  for (const [filename, contents] of Object.entries({ ...sources, "konae.json": JSON.stringify(config) })) await writeFile(path.join(directory, filename), contents);
  const published = await publishLocal(directory, config, token);
  await admin.call("signup_policy_set", { deploymentId: published.id, mode: "open" });
  const inspected = await admin.call<{ deployment: { hostname: string } }>("project_inspect", { projectId: "quota-app" });
  const hostname = inspected.deployment.hostname;
  const originalCookie = await sign(sourceUrl, hostname, "signup");
  const writer = runtime(sourceUrl, hostname, originalCookie);
  await writer.call("actions_invoke", { name: "notes.add", input: { body: "Before the quota" } });

  // Ten 2 MiB files fill the 20 MiB file quota exactly.
  const fileIds: string[] = [];
  for (let index = 0; index < 10; index++) {
    const bytes = new Uint8Array(2 * mib).fill(index + 1);
    const upload = await writer.upload(bytes);
    fileIds.push((await writer.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: `part-${index}.bin`, type: "application/octet-stream" })).id);
    await writer.cancel(upload.transferId);
  }
  const extra = await writer.upload(new Uint8Array(1));
  await assert.rejects(writer.call("files_upload", { transferId: extra.transferId, name: "over.bin" }), /20 MiB file quota/);
  await writer.cancel(extra.transferId);

  // Owner SQL fills the database through the Kernel until its 256 MiB cap refuses a write.
  const fillStarted = performance.now();
  await fill(admin, published.id, mib, 8);
  await fill(admin, published.id, 64 * 1024, 8);
  await fill(admin, published.id, 4096, 1);
  await fill(admin, published.id, 100, 1);
  const fillSeconds = (performance.now() - fillStarted) / 1000;
  await assert.rejects(writer.call("actions_invoke", { name: "blobs.add" }), /full/);
  const databaseBytes = (await stat(path.join(sourceData, "deployments", published.id, "database.sqlite"))).size;
  assert.ok(databaseBytes > 255 * mib, `The database holds ${databaseBytes} bytes, short of the cap.`);
  const before = await counts(admin, published.id);

  const archive = path.join(root, "quota.tar");
  const stopSource = sampleRss(sourceKernel.pid);
  const stopClient = sampleRss(process.pid);
  const exportStarted = performance.now();
  await exportLocal(directory, config, archive, token);
  const exportSeconds = (performance.now() - exportStarted) / 1000;
  const [sourcePeak, exportClientPeak] = [await stopSource(), await stopClient()];
  const archiveBytes = (await stat(archive)).size;
  sourceKernel.kill();
  await sourceKernel.exited;
  await rm(sourceData, { recursive: true, force: true });

  const kit = path.join(root, "kit");
  await mkdir(kit);
  await command(["tar", "-xf", archive, "-C", kit]);
  await rm(archive);
  await symlink(path.join(repo, "node_modules"), path.join(kit, "node_modules"), "dir");
  await writeFile(path.join(kit, ".env"), `KONAE_PORT=${port + 1}\nKONAE_ADMIN_KEY=${key}\n`, { mode: 0o600 });
  const targetData = path.join(kit, "data");
  const targetKernel = await startKernel(targetUrl, targetData);
  const stopTarget = sampleRss(targetKernel.pid);
  const importStarted = performance.now();
  const { peak: deployPeak } = await command(["node", "scripts/deploy.mjs"], kit, sampleRss);
  const importSeconds = (performance.now() - importStarted) / 1000;
  const targetPeak = await stopTarget();

  const marker = JSON.parse(await readFile(path.join(kit, "konae/deployment.json"), "utf8"));
  const moved = JSON.parse(await readFile(path.join(targetData, "deployments", marker.deploymentId, "deployment.json"), "utf8"));
  const target = await ownerSession(targetUrl, "owner@selfhost.local", "login");
  assert.equal(await counts(target, marker.deploymentId), before, "Row counts and byte totals differ after the import.");
  assert.ok((await stat(path.join(targetData, "deployments", marker.deploymentId, "database.sqlite"))).size > 255 * mib);
  assert.deepEqual(moved.signupPolicy, { mode: "open", allowlist: [] });
  assert.equal(moved.manifest.visibility, "private");
  const cookie = await sign(targetUrl, moved.hostname, "login");
  const targetWriter = runtime(targetUrl, moved.hostname, cookie);
  assert.deepEqual(await targetWriter.call("actions_invoke", { name: "notes.list" }), [{ id: 1, body: "Before the quota" }]);
  const files = await targetWriter.call<{ id: string }[]>("files_list");
  assert.equal(files.length, 10);
  assert.deepEqual(await targetWriter.download(await targetWriter.call<Transfer>("files_read", { fileId: fileIds[9] })), new Uint8Array(2 * mib).fill(10));
  await assert.rejects(targetWriter.call("actions_invoke", { name: "blobs.add" }), /full/);
  await assert.rejects(runtime(targetUrl, moved.hostname, originalCookie).call("actions_invoke", { name: "notes.list" }), /Sign in|401|authentication/i);
  const audit = new Database(path.join(targetData, "mcp.sqlite"));
  const writes = (audit.query("SELECT COUNT(*) n FROM audit WHERE tool='transfer_write'").get() as { n: number }).n;
  audit.close();
  assert.ok(writes < 20, `The import wrote ${writes} transfer_write audit rows.`);
  targetKernel.kill();
  await targetKernel.exited;

  const report = {
    databaseMiB: +(databaseBytes / mib).toFixed(1), filesMiB: 20, archiveMiB: +(archiveBytes / mib).toFixed(1),
    fillSeconds: +fillSeconds.toFixed(1), exportSeconds: +exportSeconds.toFixed(1), importSeconds: +importSeconds.toFixed(1),
    peakRssMiB: { sourceKernel: sourcePeak, exportClient: exportClientPeak, targetKernel: targetPeak, deployScript: deployPeak },
    importAuditWriteRows: writes,
  };
  console.log(JSON.stringify(report, null, 2));
  // Each process, baseline included, stays under the 256 MiB database it moved.
  for (const [name, peak] of Object.entries(report.peakRssMiB)) assert.ok(peak < 192, `${name} peaked at ${peak} MiB.`);
  console.log("Passed the 256 MiB database and 20 MiB files export and import with the write quota exhausted, with bounded memory.");
} finally {
  for (const child of processes) if (child.exitCode === null) child.kill();
  await Promise.all(processes.map((child) => child.exited));
  await rm(root, { recursive: true, force: true });
}
