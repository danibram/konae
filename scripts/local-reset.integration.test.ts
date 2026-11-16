import { afterEach, expect, test } from "bun:test";
import Database from "bun:sqlite";
import { createHash, randomUUID } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { packageComponentFiles, type PackageComponentName } from "../src/lib/portable-package";
import { acquireLocalLifecycleLock, buildLocal, initLocal, localKernel, readSettings, resetData } from "./konae";
import { PULL_DIRECTORY, readPulled, type Pulled } from "./pull";

const enabled = process.env.KONAE_RUN_LOCAL_RESET_INTEGRATION === "1";
const roots: string[] = [];
afterEach(async () => { for (const root of roots.splice(0)) await rm(root, { recursive: true, force: true }); });

async function appFolder(name: string) {
  const root = await mkdtemp(path.join(tmpdir(), "konae-local-reset-"));
  roots.push(root);
  await initLocal(root, { name, template: "tasks" });
  const settingsPath = path.join(root, "konae.json");
  const settingsValue = JSON.parse(await readFile(settingsPath, "utf8"));
  settingsValue.signupPolicy = { mode: "open", allowlist: [] };
  await writeFile(settingsPath, JSON.stringify(settingsValue, null, 2));
  await buildLocal(root, { development: true });
  const settings = await readSettings(root);
  const probe = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const port = probe.port;
  probe.stop();
  const kernel = await localKernel(root, settings, { build: false, port: String(port) });
  return { root, settings, kernel };
}

async function startAndStop(kernel: Awaited<ReturnType<typeof localKernel>>, afterStart?: (port: number) => Promise<void>) {
  const child = Bun.spawn([kernel.binary], { env: kernel.env, stdout: "pipe", stderr: "pipe" });
  const output = { stdout: "", stderr: "" };
  const drain = async (stream: ReadableStream<Uint8Array>, key: "stdout" | "stderr") => {
    const decoder = new TextDecoder();
    for await (const chunk of stream) output[key] += decoder.decode(chunk);
  };
  const stdout = drain(child.stdout as ReadableStream<Uint8Array>, "stdout");
  const stderr = drain(child.stderr as ReadableStream<Uint8Array>, "stderr");
  try {
    for (let attempt = 0; attempt < 200 && !/listening on/.test(output.stdout); attempt++) {
      if (child.exitCode !== null) break;
      await Bun.sleep(50);
    }
    const match = output.stdout.match(/listening on (\d+)/);
    if (!match) throw Error(`Kernel did not start. stdout: ${output.stdout}\nstderr: ${output.stderr}`);
    if (afterStart) await afterStart(Number(match[1]));
  } finally {
    if (child.exitCode === null) { child.kill("SIGTERM"); await child.exited; }
    await Promise.all([stdout, stderr]);
  }
}

async function rawOperator(kernel: Awaited<ReturnType<typeof localKernel>>, appId: string, mode: "live" | "test") {
  const child = Bun.spawn([kernel.binary, "test", "calls", "--app", appId], {
    env: { ...kernel.env, KONAE_EXECUTION_MODE: mode }, stdout: "pipe", stderr: "pipe",
  });
  const [, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  return { code: child.exitCode, stdout, stderr };
}

function addMailRows(file: string, appId: string, prefix: string) {
  const db = new Database(file);
  db.exec("PRAGMA journal_mode = WAL");
  const insert = db.query(`INSERT INTO jobs
    (id, purpose, recipient, scope, app_name, token_hash, link_nonce, link_sealed, link_expires_at, state, next_attempt_at, created_at, updated_at)
    VALUES ($id, $purpose, $recipient, $scope, $app, $hash, $nonce, $sealed, $expires, $state, $next, $now, $now)`);
  const now = Date.now();
  for (const [suffix, purpose, state] of [["auth", "verification", "pending"], ["notice", "notify", "pending"], ["done", "notify", "delivered"], ["other", "notify", "pending"]]) {
    insert.run({
      $id: `${prefix}-${suffix}`, $purpose: purpose, $recipient: `${suffix}@example.test`,
      $scope: suffix === "other" ? "deployment:app_other" : `deployment:${appId}`,
      $app: "Reset test", $hash: "fixture-hash", $nonce: Buffer.from("nonce"), $sealed: Buffer.from("sealed"),
      $expires: now + 86400_000, $state: state, $next: now, $now: now,
    });
  }
  db.close();
}

function addCallRows(file: string, appId: string) {
  const db = new Database(file);
  db.exec("PRAGMA journal_mode = WAL");
  const now = Date.now();
  const insert = db.query(`INSERT INTO calls (id, deployment, action, input, state, next_attempt_at, then_action, then_state,
      execution_mode, created_at, updated_at) VALUES (?, ?, 'hook.send', '{}', ?, ?, 'hook.logged', ?, 'test', ?, ?)`);
  insert.run("call-pending", appId, "pending", now, "processing", now, now);
  insert.run("call-complete", appId, "delivered", now, "done", now, now);
  insert.run("call-other", "app_other", "pending", now, "processing", now, now);
  db.close();
}

async function createPull(root: string, settings: { id: string; visibility: "public" | "private" }, sourceData: string): Promise<Pulled> {
  const pull = path.join(root, PULL_DIRECTORY);
  const data = path.join(pull, "data");
  const files = path.join(data, "files");
  await mkdir(files, { recursive: true });
  await cp(path.join(sourceData, "database.sqlite"), path.join(data, "database.sqlite"));
  await cp(path.join(sourceData, "identity.sqlite"), path.join(data, "identity.sqlite"));
  const raw = {
    database: new Uint8Array(await readFile(path.join(sourceData, "database.sqlite"))),
    identity: new Uint8Array(await readFile(path.join(sourceData, "identity.sqlite"))),
    files: new Uint8Array(),
  } satisfies Partial<Record<PackageComponentName, Uint8Array>>;
  const fileId = "00000000-0000-4000-8000-000000000001";
  const fileBytes = new TextEncoder().encode("pulled file");
  await writeFile(path.join(files, "index.json"), JSON.stringify([{ id: fileId, name: "pulled.txt", type: "text/plain", size: fileBytes.length, visibility: "public", createdAt: Date.now() }]));
  await writeFile(path.join(files, `${fileId}.blob`), fileBytes);
  const archivePath = path.join(pull, "files-built.tar");
  const tar = Bun.spawn(["tar", "-cf", archivePath, "-C", files, "index.json", `${fileId}.blob`], { stdout: "pipe", stderr: "pipe" });
  const [, tarError] = await Promise.all([tar.exited, new Response(tar.stderr).text()]);
  if (tar.exitCode !== 0) throw Error(`Could not create the files component: ${tarError}`);
  raw.files = new Uint8Array(await readFile(archivePath));
  await rm(archivePath, { force: true });
  const completeRaw = raw as Record<PackageComponentName, Uint8Array>;
  const contents = Object.entries(completeRaw).map(([name, bytes]) => ({
    name: name as PackageComponentName, bytes,
    component: { name: name as PackageComponentName, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") },
  }));
  for (const { name, bytes } of contents) await writeFile(path.join(pull, packageComponentFiles[name]), bytes);
  const manifest = {
    format: "konae-package" as const, version: 1 as const, packageId: `pkg_${randomUUID()}`, createdAt: Date.now(),
    deploymentId: settings.id, projectId: settings.id, visibility: settings.visibility, publishedRevision: 1,
    contents: contents.map(({ component }) => component), excluded: [],
  };
  const pulled: Pulled = { manifest, from: "https://published.example.test", at: Date.now(), snapshotId: `snapshot_${randomUUID()}` };
  await writeFile(path.join(pull, "pull.json"), JSON.stringify(pulled));
  return (await readPulled(root))!;
}

test.skipIf(!enabled)("compiled Kernel resets a fresh app and keeps an empty backup under the lifecycle lock", async () => {
  const { root, settings, kernel } = await appFolder("Empty reset");
  expect(await readFile(path.join(root, "data/database.sqlite")).then(() => true, () => false)).toBe(false);
  const record = await resetData(root, kernel, undefined);
  expect(record.callsRetired).toBe(0);
  expect(record.mailRetired).toBe(0);
  expect(await readFile(path.join(record.backup, "data/database.sqlite")).then(() => true, () => false)).toBe(false);
  expect(JSON.parse(await readFile(path.join(root, ".konae/local-state.json"), "utf8")).executionId).toBe(record.executionId);
  expect(settings.id).toMatch(/^app_/);

  const release = await acquireLocalLifecycleLock(root);
  try { await expect(resetData(root, kernel, undefined)).rejects.toThrow("holds"); }
  finally { await release(); }
});

test.skipIf(!enabled)("compiled Kernel imports raw verified data and retires only this app's pending effects with rollback", async () => {
  const { root, settings, kernel } = await appFolder("Reset import");
  await startAndStop(kernel, async (port) => {
    const host = `${settings.slug}.localhost:${port}`;
    const response = await fetch(`http://127.0.0.1:${port}/_konae/auth/signup`, {
      method: "POST", headers: { host, origin: `http://${host}`, "content-type": "application/json" },
      body: JSON.stringify({ email: "old@example.test", password: "correct horse battery" }),
    });
    expect(response.status, await response.clone().text()).toBe(200);
  });
  await kernel.run(["sql", "INSERT INTO tasks (title, done, created_at) VALUES ('old local row', 0, 10)"]);
  await kernel.run(["test", "calls"]);
  const liveQueues = await rawOperator(kernel, settings.id, "live");
  expect(liveQueues.code).not.toBe(0);
  expect(`${liveQueues.stdout}\n${liveQueues.stderr}`).toContain("KONAE_EXECUTION_MODE=test");

  const oldAccount = "old@example.test";
  const userId = "user_pulled_1";
  const identity = new Database(path.join(root, "data/identity.sqlite"));
  identity.query("UPDATE app_users SET id = ? WHERE email = ?").run(userId, oldAccount);
  identity.close();
  const source = await mkdtemp(path.join(tmpdir(), "konae-reset-pulled-data-"));
  roots.push(source);
  await mkdir(source, { recursive: true });
  await cp(path.join(root, "data/database.sqlite"), path.join(source, "database.sqlite"));
  await cp(path.join(root, "data/identity.sqlite"), path.join(source, "identity.sqlite"));
  const pulledDb = new Database(path.join(source, "database.sqlite"));
  pulledDb.query("UPDATE tasks SET title = 'published package row'").run();
  pulledDb.close();
  const pulled = await createPull(root, settings, source);

  addCallRows(path.join(kernel.data, "calls.sqlite"), settings.id);
  addMailRows(path.join(kernel.data, "mail-test.sqlite"), settings.id, "test");
  addMailRows(path.join(kernel.data, "mail.sqlite"), settings.id, "legacy");
  const originalMarker = JSON.stringify({ executionId: "old-execution", snapshotId: "old-snapshot" });
  await mkdir(path.join(root, ".konae"), { recursive: true });
  await writeFile(path.join(root, ".konae/pull-imported.json"), originalMarker);
  await writeFile(path.join(root, ".konae/local-state.json"), originalMarker);

  const extracted = path.join(root, PULL_DIRECTORY, "data/database.sqlite");
  const forged = new Database(extracted);
  forged.query("UPDATE tasks SET title = 'tampered extracted data'").run();
  forged.close();

  const rawDatabase = path.join(root, PULL_DIRECTORY, packageComponentFiles.database);
  const trustedRaw = await readFile(rawDatabase);
  const changedRaw = new Uint8Array(trustedRaw);
  changedRaw[changedRaw.length - 1] ^= 1;
  await writeFile(rawDatabase, changedRaw);
  await expect(resetData(root, kernel, pulled)).rejects.toThrow("no longer matches snapshot");
  await writeFile(rawDatabase, trustedRaw);

  const before = new Database(path.join(root, "data/database.sqlite"));
  before.query("SELECT id FROM tasks LIMIT 1").get();
  before.close();
  const retirementFailureKernel = { ...kernel, async run(args: string[], input?: string) {
    if (args[0] === "test" && args[1] === "reset-effects") throw Error("retirement failure fixture");
    return kernel.run(args, input);
  } };
  await expect(resetData(root, retirementFailureKernel, pulled)).rejects.toThrow("retirement failure fixture");
  expect(await readFile(path.join(root, ".konae/pull-imported.json"), "utf8")).toBe(originalMarker);
  expect(await readFile(path.join(root, ".konae/local-state.json"), "utf8")).toBe(originalMarker);
  const afterRollback = new Database(path.join(root, "data/database.sqlite"));
  expect(afterRollback.query("SELECT title FROM tasks").all()).toEqual([{ title: "old local row" }]);
  afterRollback.close();
  const callsAfterRollback = new Database(path.join(kernel.data, "calls.sqlite"));
  expect((callsAfterRollback.query("SELECT state FROM calls WHERE id = 'call-pending'").get() as { state: string }).state).toBe("pending");
  callsAfterRollback.close();
  for (const name of ["mail-test.sqlite", "mail.sqlite"]) {
    const queue = new Database(path.join(kernel.data, name));
    expect(queue.query("SELECT state, link_sealed IS NOT NULL AS sealed FROM jobs WHERE id LIKE '%-auth'").get()).toEqual({ state: "pending", sealed: 1 });
    queue.close();
  }

  const reset = await resetData(root, kernel, pulled);
  expect(reset.snapshotId).toBe(pulled.snapshotId);
  expect(reset.packageId).toBe(pulled.manifest.packageId);
  expect(reset.callsRetired).toBe(1);
  expect(reset.mailRetired).toBe(4);
  expect(JSON.parse(await readFile(path.join(root, ".konae/pull-imported.json"), "utf8"))).toMatchObject({ packageId: pulled.manifest.packageId, snapshotId: pulled.snapshotId, executionId: reset.executionId });

  const importedDb = new Database(path.join(root, "data/database.sqlite"));
  expect(importedDb.query("SELECT title FROM tasks").all()).toEqual([{ title: "published package row" }]);
  importedDb.close();
  const importedIdentity = new Database(path.join(root, "data/identity.sqlite"));
  expect(importedIdentity.query("SELECT email FROM app_users WHERE id = ?").get(userId)).toEqual({ email: oldAccount });
  importedIdentity.close();
  expect(await readFile(path.join(root, "data/files/index.json"), "utf8")).toContain("pulled.txt");
  expect(await readFile(path.join(root, "data/files/00000000-0000-4000-8000-000000000001.blob"), "utf8")).toBe("pulled file");

  const calls = new Database(path.join(kernel.data, "calls.sqlite"));
  expect(calls.query("SELECT state, error, then_state FROM calls WHERE id = 'call-pending'").get()).toEqual({ state: "failed", error: "local-reset", then_state: "skipped" });
  expect(calls.query("SELECT state FROM calls WHERE id = 'call-complete'").get()).toEqual({ state: "delivered" });
  expect(calls.query("SELECT state FROM calls WHERE id = 'call-other'").get()).toEqual({ state: "pending" });
  calls.close();

  for (const name of ["mail-test.sqlite", "mail.sqlite"]) {
    const mail = new Database(path.join(kernel.data, name));
    expect(mail.query("SELECT state, error_kind, link_nonce, link_sealed FROM jobs WHERE id LIKE '%-auth' OR id LIKE '%-notice'").all())
      .toEqual(expect.arrayContaining([
        { state: "failed", error_kind: "local-reset", link_nonce: null, link_sealed: null },
        { state: "failed", error_kind: "local-reset", link_nonce: null, link_sealed: null },
      ]));
    expect(mail.query("SELECT state FROM jobs WHERE id LIKE '%-done'").get()).toEqual({ state: "delivered" });
    expect(mail.query("SELECT state FROM jobs WHERE id LIKE '%-other'").get()).toEqual({ state: "pending" });
    mail.close();
  }

  const callsBackup = new Database(path.join(reset.backup, "queues/calls.sqlite"));
  expect(callsBackup.query("SELECT state FROM calls WHERE id = 'call-pending'").get()).toEqual({ state: "pending" });
  callsBackup.close();
  const backupMail = new Database(path.join(reset.backup, "queues/mail-test.sqlite"));
  expect(backupMail.query("SELECT state, link_sealed IS NOT NULL AS sealed FROM jobs WHERE id = 'test-auth'").get()).toEqual({ state: "pending", sealed: 1 });
  backupMail.close();
});
