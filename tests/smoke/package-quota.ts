import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { configOf, exportLocal, initLocal, publishLocal, readSettings } from "../../scripts/konae";
import type { AppSettings } from "../../src/lib/app-folder";
import type { McpClient, Transfer } from "../../src/mcp/client";
import { account, buildKernel, runtime, startKernel, type Kernel } from "../kernel-harness";

// The quota proof of ADR-0008. An app whose database reached its 256 MiB cap,
// with 20 MiB of files, leaves Konae through `exportLocal` with its data and
// runs on the `kernel` binary alone. The exported folder holds the same row
// counts, byte totals and files, an app user signs in with their password,
// and the script samples peak resident memory on the source Kernel, the
// export client and the target `kernel`.
// Run with `bun run tests/smoke/package-quota.ts --port 19460`. The target
// `kernel` uses the next port. It needs about 2 GB of free disk.
const portIndex = process.argv.indexOf("--port");
const port = portIndex < 0 ? 19460 : Number(process.argv[portIndex + 1]);
if (!Number.isInteger(port) || port < 1024 || port > 65534) throw Error("Choose a source port between 1024 and 65534. The target uses the next port.");
const repo = path.resolve(import.meta.dir, "../..");
const kernelBinary = path.join(repo, "kernel-rs/target/debug/kernel");
const root = await mkdtemp(path.join(tmpdir(), "konae-package-quota-"));
const setupKey = crypto.randomUUID() + crypto.randomUUID();
const email = "writer@quota.test";
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
let source: Kernel | undefined;
let target: ReturnType<typeof Bun.spawn> | undefined;

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

/** The process that listens on `port`. The harness keeps its Kernel's process to itself. */
async function listener(port: number) {
  const child = Bun.spawn(["lsof", "-t", `-iTCP:${port}`, "-sTCP:LISTEN"], { stdout: "pipe", stderr: "ignore" });
  const pid = Number((await new Response(child.stdout).text()).trim().split("\n")[0]);
  if (!Number.isInteger(pid) || pid <= 0) throw Error(`No process listens on port ${port}.`);
  return pid;
}

const sql = (client: McpClient, deploymentId: string, statement: string) =>
  client.call<SqlAnswer>("database_execute", { deploymentId, operation: "sql", input: { sql: statement } });

type Counts = { blobs: number; bytes: number; notes: number };
type SqlAnswer = { rows?: Counts[]; result?: { rows?: Counts[] } };

/** The one row of COUNTS, from an answer that holds it at the top or under `result`. */
function countsOf(answer: SqlAnswer) {
  const rows = answer.rows ?? answer.result?.rows;
  assert.ok(rows?.length === 1, `The counts query answered ${JSON.stringify(answer)}.`);
  return rows[0];
}

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

const COUNTS = "SELECT (SELECT COUNT(*) FROM blobs) blobs, (SELECT SUM(length(body)) FROM blobs) bytes, (SELECT COUNT(*) FROM notes) notes LIMIT 1";

/** Runs one `kernel` command beside the target Kernel and returns its parsed output. */
async function command(env: Record<string, string>, ...args: string[]) {
  const child = Bun.spawn([kernelBinary, ...args], { env, stdout: "pipe", stderr: "pipe" });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  if (status !== 0) throw Error(`kernel ${args.join(" ")} failed.\n${stderr}\n${stdout}`);
  return JSON.parse(stdout);
}

try {
  await buildKernel();
  source = await startKernel({ port, data: path.join(root, "source"), setupKey, ownerEmail: "owner@quota.test" });
  const sourceKernel = source;
  const { token } = await sourceKernel.owner.call<{ token: string }>("token_create", { scopes: ["read", "write", "publish", "export_data"], ttlSeconds: 7200 });

  // An app folder as `konae init` writes it, with this script's sources in app/.
  const directory = path.join(root, "quota-app");
  await initLocal(directory, { name: "Quota app" });
  await rm(path.join(directory, "app"), { recursive: true });
  await mkdir(path.join(directory, "app"));
  for (const [filename, contents] of Object.entries(sources)) await writeFile(path.join(directory, "app", filename), contents);
  const initial = await readSettings(directory);
  const settings: AppSettings = { ...initial, visibility: "private", signupPolicy: { mode: "open", allowlist: [] } };
  await writeFile(path.join(directory, "konae.json"), `${JSON.stringify(settings, null, 2)}\n`);
  const published = await publishLocal(directory, configOf(settings, sourceKernel.url), token);
  const inspected = await sourceKernel.owner.call<{ deployment: { hostname: string } }>("project_inspect", { projectId: settings.id });
  const hostname = inspected.deployment.hostname;
  const originalCookie = await account(sourceKernel, hostname, "signup", email);
  const writer = runtime(sourceKernel, hostname, originalCookie);
  await writer.call("actions_invoke", { name: "notes.add", input: { body: "Before the quota" } });

  // Ten 2 MiB files fill the 20 MiB file quota exactly.
  const fileIds: string[] = [];
  for (let index = 0; index < 10; index++) {
    const upload = await writer.upload(new Uint8Array(2 * mib).fill(index + 1));
    fileIds.push((await writer.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: `part-${index}.bin`, type: "application/octet-stream" })).id);
    await writer.cancel(upload.transferId);
  }
  const extra = await writer.upload(new Uint8Array(1));
  await assert.rejects(writer.call("files_upload", { transferId: extra.transferId, name: "over.bin" }), /20 MiB file quota/);
  await writer.cancel(extra.transferId);

  // Owner SQL fills the database through the Kernel until its 256 MiB cap refuses a write.
  const fillStarted = performance.now();
  await fill(sourceKernel.owner, published.id, mib, 8);
  await fill(sourceKernel.owner, published.id, 64 * 1024, 8);
  await fill(sourceKernel.owner, published.id, 4096, 1);
  await fill(sourceKernel.owner, published.id, 100, 1);
  const fillSeconds = (performance.now() - fillStarted) / 1000;
  await assert.rejects(writer.call("actions_invoke", { name: "blobs.add" }), /full/);
  const databaseBytes = (await stat(path.join(sourceKernel.data, "apps", settings.id, "data/database.sqlite"))).size;
  assert.ok(databaseBytes > 255 * mib, `The database holds ${databaseBytes} bytes, short of the cap.`);
  const before = countsOf(await sql(sourceKernel.owner, published.id, COUNTS));
  assert.ok(before.blobs > 0 && before.bytes > 250 * mib && before.notes === 1, `The source holds ${JSON.stringify(before)}.`);

  // The export lands where a Kernel looks for its apps, in a data directory no Konae ever used.
  const targetData = path.join(root, "target");
  const folder = path.join(targetData, "apps", settings.id);
  const clientBaseline = Math.round(Number((await Bun.$`ps -o rss= -p ${process.pid}`.text()).trim()) / 1024);
  const stopSource = sampleRss(await listener(port));
  const stopClient = sampleRss(process.pid);
  const exportStarted = performance.now();
  await exportLocal(directory, folder, { url: sourceKernel.url, token });
  const exportSeconds = (performance.now() - exportStarted) / 1000;
  const [sourcePeak, exportClientPeak] = [await stopSource(), await stopClient()];
  await sourceKernel.stop();
  source = undefined;
  await rm(sourceKernel.data, { recursive: true, force: true });

  assert.equal(existsSync(path.join(folder, ".konae")), false, "The export carries the installation's .konae/.");
  assert.equal(existsSync(path.join(folder, "data/secrets.sqlite")), false, "The export carries the app's secrets.");
  assert.ok((await stat(path.join(folder, "data/database.sqlite"))).size > 255 * mib);
  const exported = JSON.parse(await readFile(path.join(folder, "konae.json"), "utf8")) as AppSettings;
  assert.equal(exported.visibility, "private");
  assert.deepEqual(exported.signupPolicy, { mode: "open", allowlist: [] });
  const storedFiles = (await readdir(path.join(folder, "data/files"), { recursive: true, withFileTypes: true })).filter((entry) => entry.isFile() && entry.name !== "index.json");
  const storedBytes = (await Promise.all(storedFiles.map((entry) => stat(path.join(entry.parentPath, entry.name))))).reduce((sum, file) => sum + file.size, 0);
  assert.equal(storedFiles.length, 10, "The export holds a different number of files.");
  assert.equal(storedBytes, 20 * mib, "The exported files hold a different byte total.");

  // The `kernel` binary serves the folder alone.
  const targetPort = port + 1;
  const url = `http://127.0.0.1:${targetPort}`;
  const env = {
    ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_") && !key.startsWith("VITE_"))) as Record<string, string>,
    KONAE_DATA_DIR: targetData, KONAE_KERNEL_PORT: String(targetPort), KONAE_BIND: "127.0.0.1", KONAE_PUBLIC_ORIGIN: url,
    KONAE_HOST_SUFFIX: "localhost", KONAE_TRUSTED_PROXIES: "127.0.0.1", KONAE_MAIL_TRANSPORT: "development",
  };
  target = Bun.spawn([kernelBinary], { env, stdout: "pipe", stderr: "inherit" });
  const targetKernel = target;
  const stopTarget = sampleRss(targetKernel.pid);
  const importStarted = performance.now();
  let log = "";
  const listening = (async () => {
    for await (const block of targetKernel.stdout as ReadableStream<Uint8Array>) {
      log += new TextDecoder().decode(block);
      if (log.includes(`listening on ${targetPort}`)) return;
    }
    throw Error(`The kernel binary stopped during startup.\n${log}`);
  })();
  await Promise.race([listening, Bun.sleep(60_000).then(() => { throw Error(`The kernel binary did not start.\n${log}`); })]);
  const startSeconds = (performance.now() - importStarted) / 1000;
  const served = log.match(/Serving .+ at http:\/\/([^:/]+)/);
  assert.ok(served, `The kernel binary names no app it serves.\n${log}`);
  const targetHost = served[1];
  const app = { url, data: targetData } as Kernel;

  assert.deepEqual(countsOf(await command(env, "sql", COUNTS)), before, "Row counts and byte totals differ after the export.");
  const cookie = await account(app, targetHost, "login", email);
  const targetWriter = runtime(app, targetHost, cookie);
  assert.deepEqual(await targetWriter.call("actions_invoke", { name: "notes.list" }), [{ id: 1, body: "Before the quota" }]);
  const files = await targetWriter.call<{ id: string }[]>("files_list");
  assert.deepEqual(files.map((file) => file.id).sort(), [...fileIds].sort());
  assert.deepEqual(await targetWriter.download(await targetWriter.call<Transfer>("files_read", { fileId: fileIds[9] })), new Uint8Array(2 * mib).fill(10));
  await assert.rejects(targetWriter.call("actions_invoke", { name: "blobs.add" }), /full/);
  await assert.rejects(runtime(app, targetHost, originalCookie).call("actions_invoke", { name: "notes.list" }), /Sign in|401|authentication/i);
  const targetPeak = await stopTarget();
  targetKernel.kill();
  await targetKernel.exited;

  const report = {
    databaseMiB: +(databaseBytes / mib).toFixed(1), filesMiB: storedBytes / mib, counts: before,
    fillSeconds: +fillSeconds.toFixed(1), exportSeconds: +exportSeconds.toFixed(1), targetStartSeconds: +startSeconds.toFixed(1),
    exportClientBeforeExportMiB: clientBaseline,
    peakRssMiB: { sourceKernel: sourcePeak, exportClient: exportClientPeak, targetKernel: targetPeak },
  };
  console.log(JSON.stringify(report, null, 2));
  // Each process, baseline included, stays under the 256 MiB database it moved.
  for (const [name, peak] of Object.entries(report.peakRssMiB)) assert.ok(peak < 192, `${name} peaked at ${peak} MiB.`);
  console.log("Passed. The 256 MiB database and 20 MiB of files left through the export and ran on the kernel binary alone, with bounded memory.");
} finally {
  await source?.stop();
  if (target && target.exitCode === null) { target.kill(); await target.exited; }
  await rm(root, { recursive: true, force: true });
}
