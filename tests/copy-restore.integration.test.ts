import { beforeAll, describe, expect, test } from "bun:test";
import { mkdtemp, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient, type Transfer } from "../src/mcp/client";

import { checkWork, checkReceiver, checkRevocations, checkWorkers, seedWork } from "./recovery-work";
import type { PackageManifest } from "../src/lib/portable-package";

const binary = path.resolve(process.env.CARGO_TARGET_DIR ?? "kernel-rs/target", "debug/konae");
const setupKey = "copy-restore-setup-secret";
const password = "copy-restore-password-2026";
const attachment = new TextEncoder().encode("Attachment survives the copy");
const s3Endpoint = process.env.KONAE_TEST_S3_ENDPOINT;
const s3Bucket = process.env.KONAE_TEST_S3_BUCKET ?? "";
const s3Env = {
  KONAE_S3_ENDPOINT: s3Endpoint ?? "",
  KONAE_S3_ACCESS_KEY: process.env.KONAE_TEST_S3_ACCESS_KEY ?? "",
  KONAE_S3_SECRET_KEY: process.env.KONAE_TEST_S3_SECRET_KEY ?? "",
  KONAE_S3_BUCKET: s3Bucket,
};
const minio = Boolean(s3Endpoint);
if (!minio) console.warn("SKIP S3 restore: no disposable KONAE_TEST_S3_ENDPOINT supplied.");
else if (new URL(s3Endpoint!).hostname !== "127.0.0.1" || Object.values(s3Env).some(value => !value)) {
  throw Error("Provide a loopback MinIO endpoint and its disposable bucket and credentials.");
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--locked", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
}, 300_000);

function kernelEnv(root: string, extra: Record<string, string> = {}) {
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_")));
  return { ...inherited, KONAE_DATA_DIR: root, KONAE_BIND: "127.0.0.1", KONAE_SETUP_KEY: setupKey, KONAE_HOST_SUFFIX: "localhost", KONAE_DOMAIN: "paths.lab.test", ...extra };
}

async function startKernel(root: string, extra: Record<string, string> = {}) {
  const port = 19500 + Math.floor(Math.random() * 400);
  const url = `http://127.0.0.1:${port}`;
  const endpoint = extra.KONAE_S3_ENDPOINT;
  const sandbox = '(version 1)(allow default)(deny network-outbound)' +
    (endpoint ? `(allow network-outbound (remote tcp "localhost:${new URL(endpoint).port}"))` : "");
  const command = process.platform === "darwin" ? ["/usr/bin/sandbox-exec", "-p", sandbox, binary] : [binary];
  const child = Bun.spawn(command, { env: kernelEnv(root, { KONAE_KERNEL_PORT: String(port), KONAE_PUBLIC_ORIGIN: url, KONAE_MAIL_TRANSPORT: "development", ...extra }), stdout: "ignore", stderr: "inherit" });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) return { url, child }; } catch {}
    if (child.exitCode !== null) throw Error("The Kernel stopped during startup.");
    await Bun.sleep(100);
  }
  child.kill();
  throw Error("The Kernel did not start.");
}

async function stop(child: ReturnType<typeof Bun.spawn>) {
  child.kill();
  await child.exited;
}

async function restore(from: string, root: string, extra: Record<string, string> = {}, copy?: string) {
  const child = Bun.spawn([binary, "restore", "--from", from, ...(copy ? ["--copy", copy] : [])], { env: kernelEnv(root, extra), stdout: "pipe", stderr: "pipe" });
  const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
  return { status, output: stdout + stderr };
}

function withHeaders(endpoint: string, headers: Record<string, string>) {
  return new McpClient(endpoint, { fetch: (input, init) => {
    const merged = new Headers(init?.headers);
    for (const [name, value] of Object.entries(headers)) merged.set(name, value);
    return fetch(input, { ...init, headers: merged });
  } });
}

async function sign(url: string, route: string, body: object, host?: string) {
  const response = await fetch(`${url}${route}`, { method: "POST", headers: { "content-type": "application/json", ...(host ? { host } : {}) }, body: JSON.stringify(body) });
  expect(response.status).toBe(200);
  return response.headers.get("set-cookie")!.split(";")[0];
}

const project = {
  "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);",
  "actions.json": JSON.stringify({ version: 1, actions: {
    "notes.tick": { kind: "mutation", access: "anonymous", statement: "INSERT INTO notes(title,user_id) VALUES('scheduled','timer')", bindings: [], schedule: { daily: "10:00" } },
    "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,title FROM notes WHERE user_id=?1 LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES(?1,?2)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
  } }),
};

const projectId = `copy_proof_${crypto.randomUUID().slice(0, 8)}`;
const historicalBytes = new TextEncoder().encode("Private file before the import");

async function backups(owner: McpClient) {
  return owner.call<{ id: string; holds: string[] }[]>("database_execute", { projectId, operation: "list-backups" });
}

async function snapshotPackage(owner: McpClient) {
  const exported = await owner.call<{ manifest: PackageManifest; downloads: Record<string, Transfer>; folder: Transfer }>("project_export", { projectId });
  const components: Record<string, string> = {};
  for (const { name } of exported.manifest.contents) {
    const bytes = await owner.download(exported.downloads[name]);
    components[name] = (await owner.upload(bytes, projectId, name)).transferId;
    await owner.cancel(exported.downloads[name].transferId);
  }
  await owner.cancel(exported.folder.transferId);
  return { manifest: exported.manifest, components };
}

async function moduleStates(owner: McpClient) {
  const { modules } = await owner.call<{ modules: Record<string, { state: unknown }> }>("modules_read", { projectId });
  return Object.fromEntries(Object.entries(modules).map(([name, module]) => [name, module.state]));
}

async function savedSources(owner: McpClient) {
  const { revision, transfer } = await owner.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId });
  const document = JSON.parse(new TextDecoder().decode(await owner.download(transfer)));
  await owner.cancel(transfer.transferId);
  return { revision, files: document.files };
}

async function verifyHistory(from: string, copy: string, env: Record<string, string> = {}) {
  const child = Bun.spawn([binary, "verify-history", "--from", from, "--copy", copy], {
    env: kernelEnv("/unused", env), stdout: "pipe", stderr: "inherit",
  });
  const output = await new Response(child.stdout).text();
  expect(await child.exited).toBe(0);
  const report = JSON.parse(output);
  expect(report.copy).toBe(copy);
  expect(report.internalBackups.map((r: { status: string; scope: string }) => [r.status, r.scope]).sort())
    .toEqual([["recovered", "data"], ["recovered", "database"], ["recovered", "sources"]]);
}

async function publishAndCopy(root: string, copyEnv: Record<string, string>) {
  const { url, child } = await startKernel(root, { KONAE_COPY_INTERVAL_SECONDS: "1", ...copyEnv });
  try {
    const owner = withHeaders(`${url}/mcp`, { cookie: await sign(url, "/_konae/owner/signup", { email: "owner@copy.test", password, setupKey }) });
    expect((await owner.call<{ can: Record<string, boolean> }>("session_inspect")).can).toMatchObject({ write: true, publish: true, export_data: true });
    await owner.call("project_create", { projectId, name: "Copy proof" });
    await owner.call("project_inspect", { projectId });
    const payload = { project: { id: projectId, name: "Copy proof", slug: "copy-proof", manifest: { version: 1, visibility: "public" }, files: project }, visibility: "public", initialModules: { auth: { on: true }, files: { on: true } }, artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Copied app</main>" } } };
    const transfer = await owner.upload(new TextEncoder().encode(JSON.stringify(payload)), projectId);
    const observed = await owner.call<{ sourcesRevision: number }>("project_inspect", { projectId });
    const plan = await owner.call<{ planId: string }>("release_plan", { transferId: transfer.transferId, observedSourcesRevision: observed.sourcesRevision });
    const { id, hostname } = await owner.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId });
    await owner.cancel(transfer.transferId);
    expect(await owner.call("project_inspect", { projectId })).toMatchObject({ deployment: { hostname } });

    const cookie = await sign(url, "/_konae/auth/signup", { email: "writer@copy.test", password }, hostname);
    const writer = withHeaders(`${url}/_konae/mcp`, { host: hostname, cookie });
    const flat = await owner.call<{ id: string }>("database_execute", { projectId, operation: "create-backup" });
    expect(await backups(owner)).toContainEqual(expect.objectContaining({ id: flat.id, holds: [] }));
    await writer.call("actions_invoke", { name: "notes.create", input: { title: "Before the copy" } });
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toHaveLength(1);
    const upload = await writer.upload(attachment);
    const file = await writer.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain", visibility: "private" });
    await writer.cancel(upload.transferId);

    expect(await writer.download(await writer.call<Transfer>("files_read", { fileId: file.id }))).toEqual(attachment);
    const exported = await snapshotPackage(owner);
    await writer.call("actions_invoke", { name: "notes.create", input: { title: "Before the import" } });
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toHaveLength(2);
    const oldUpload = await writer.upload(historicalBytes);
    const heldFile = await writer.call<{ id: string }>("files_upload", { transferId: oldUpload.transferId, name: "old.txt", type: "text/plain", visibility: "private" });
    await writer.cancel(oldUpload.transferId);
    expect(await writer.download(await writer.call<Transfer>("files_read", { fileId: heldFile.id }))).toEqual(historicalBytes);
    const data = await owner.call<{ backup: { id: string } }>("project_import", { projectId, replace: "data", package: exported });
    expect(await backups(owner)).toContainEqual(expect.objectContaining({ id: data.backup.id, holds: ["data"] }));
    for (const transferId of Object.values(exported.components)) await owner.cancel(transferId);
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toHaveLength(1);
    await expect(writer.call("files_read", { fileId: heldFile.id })).rejects.toThrow("File not found");
    const original = await savedSources(owner);
    const next = { artifact: payload.artifact, visibility: payload.visibility, project: { ...payload.project, files: { ...project, "history.txt": "second revision" } } };
    const release = await owner.upload(new TextEncoder().encode(JSON.stringify(next)), projectId);
    const sources = await owner.call<{ backup: { id: string } }>("project_import", { projectId, replace: "sources", transferId: release.transferId });
    expect(await backups(owner)).toContainEqual(expect.objectContaining({ id: sources.backup.id, holds: ["sources"] }));
    await owner.cancel(release.transferId);
    const current = await savedSources(owner);
    expect(current.revision).toBeGreaterThan(original.revision);
    expect(current.files["history.txt"]).toBe("second revision");

    const draft = await owner.upload(new TextEncoder().encode(JSON.stringify({
      ...payload.project, directories: [], updatedAt: Date.now(), files: { ...project, "index.html": "<main>Unpublished draft</main>" },
    })), projectId);
    await owner.call("project_sources_save", { projectId, expectedRevision: current.revision, transferId: draft.transferId });
    const atCut = await savedSources(owner);
    expect(atCut.files["index.html"]).toContain("Unpublished draft");
    const issueKey = (name: string) => owner.call<{ key: string; keyId: string }>("api_keys_create", { projectId, name, files: ["private"] });
    const good = await issueKey("kept");
    const revoked = await issueKey("retired");
    await sign(url, "/_konae/auth/signup", { email: "retired@copy.test", password }, hostname);
    const users = await owner.call<{ users: {email: string; userId: string}[] }>("app_users_list", { projectId });
    const userId = (email: string) => users.users.find(user => user.email === email)!.userId;
    const entry = await owner.call<{ url: string }>("app_user_enter", { projectId, userId: userId("writer@copy.test") });
    await owner.call("path_set", { projectId, enabled: true, name: "copied", acceptSharedOrigin: true });
    expect(await owner.call("path_read", { projectId })).toMatchObject({ enabled: true, name: "copied" });
    const modules = await moduleStates(owner);
    await owner.call("maintenance_set", { projectId, on: true });
    expect(await owner.call("maintenance_read", { projectId })).toMatchObject({ maintenance: true });
    const reset = await seedWork(root, id);
    checkWork(root, id, false);
    const written = Date.now();
    let copies: { targets: { kind: string; lastCopyId: string; lastAttemptAt: number | null; lastSuccessAt: number | null; lastError: string | null }[]; alert: string | null } | undefined;
    for (let attempt = 0; attempt < 200; attempt++) {
      copies = (await (await fetch(`${url}/_konae/health`)).json()).copies;
      expect(copies!.targets.map(target => target.kind).sort()).toEqual(
        copyEnv.KONAE_COPY_S3 ? ["directory", "s3"] : ["directory"]);
      const done = copies!.targets.every((target) => target.lastAttemptAt !== null && target.lastAttemptAt > written && target.lastSuccessAt !== null && target.lastSuccessAt >= target.lastAttemptAt);
      if (done) break;
      await Bun.sleep(100);
    }
    expect(copies!.alert).toBeNull();
    expect(copies!.targets.every((target) => target.lastAttemptAt! > written && target.lastSuccessAt! >= target.lastAttemptAt!)).toBe(true);
    const copy = copies!.targets[0].lastCopyId;
    expect(copies!.targets.every(target => target.lastCopyId === copy)).toBe(true);
    const image = path.join(copyEnv.KONAE_COPY_DIR, copy);
    checkWork(image, id, false, "frozen");
    checkRevocations(image, id, false, "frozen");
    expect(await Bun.file(path.join(image, "google.sqlite")).exists()).toBe(false);
    await owner.call("maintenance_set", { projectId, on: false });
    await owner.call("app_user_suspend", { projectId, userId: userId("retired@copy.test") });
    await owner.call("api_keys_revoke", { projectId, keyId: revoked.keyId });
    expect(await owner.call("api_keys_list", { projectId })).toMatchObject({ keys: expect.arrayContaining([
      expect.objectContaining({ keyId: revoked.keyId, revokedAt: expect.any(Number) }),
    ]) });
    return { copy, reset, id, hostname, cookie, entry: new URL(entry.url).hash.slice(7), good, revoked, atCut, modules, fileId: file.id, heldFile: heldFile.id, data: data.backup.id, sources: sources.backup.id, flat: flat.id, original };
  } finally {
    await stop(child);
  }
}

async function expectSameApp(root: string, app: Awaited<ReturnType<typeof publishAndCopy>>) {
  checkWork(root, app.id, true);
  const paused = await startKernel(root, { KONAE_EXECUTION_MODE: "test" });
  try {
    const owner = withHeaders(`${paused.url}/mcp`, { cookie: await sign(paused.url, "/_konae/owner/login", { email: "owner@copy.test", password }) });
    expect((await fetch(paused.url, { headers: { host: app.hostname } })).status).toBe(503);
    expect(await owner.call("maintenance_read", { projectId })).toMatchObject({ maintenance: true, installation: null });
    await owner.call("maintenance_set", { projectId, on: false });
    expect(await owner.call("maintenance_read", { projectId })).toMatchObject({ maintenance: false });
  } finally { await stop(paused.child); }
  const { url, child } = await startKernel(root);
  try {
    await checkWorkers(root, app.id);
    const owner = withHeaders(`${url}/mcp`, { cookie: await sign(url, "/_konae/owner/login", { email: "owner@copy.test", password }) });
    expect(await owner.call("path_read", { projectId })).toMatchObject({ enabled: true, name: "copied" });
    expect(await moduleStates(owner)).toEqual(app.modules);
    const shell = await fetch(`${url}/`, { headers: { host: app.hostname } });
    expect(shell.status).toBe(200);
    expect(await shell.text()).toContain("<main>Copied app</main>");
    const alias = await fetch(url, { headers: { host: "alias.lab.test" } });
    expect(await alias.text()).toContain("<main>Copied app</main>");
    const moved = await fetch(url, { headers: { host: "old.lab.test" }, redirect: "manual" });
    expect(moved.status).toBe(308);
    expect(new URL(moved.headers.get("location")!).hostname).toBe(app.hostname);
    const byPath = await fetch(url + "/p/copied/", { headers: { host: "paths.lab.test" } });
    expect(await byPath.text()).toContain("<main>Copied app</main>");
    const stale = await withHeaders(`${url}/_konae/mcp`, { host: app.hostname, cookie: app.cookie }).call<{ role: string }>("auth_session");
    expect(stale.role).toBe("anonymous");
    const cookie = await sign(url, "/_konae/auth/login", { email: "writer@copy.test", password }, app.hostname);
    const writer = withHeaders(`${url}/_konae/mcp`, { host: app.hostname, cookie });
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toEqual([{ id: 1, title: "Before the copy" }]);
    expect(await writer.download(await writer.call<Transfer>("files_read", { fileId: app.fileId }))).toEqual(attachment);
    const other = await sign(url, "/_konae/auth/signup", { email: "reader@copy.test", password }, app.hostname);
    expect(await withHeaders(`${url}/_konae/mcp`, { host: app.hostname, cookie: other }).call("actions_invoke", { name: "notes.list" })).toEqual([]);
    const asset = (headers: Record<string,string>) => fetch(`${url}/_konae/files/${app.fileId}`, { headers: { host: app.hostname, ...headers } });
    expect((await asset({ cookie: other })).status).toBe(404);
    expect(new Uint8Array(await (await asset({ authorization: `Bearer ${app.good.key}` })).arrayBuffer())).toEqual(attachment);
    expect((await asset({ authorization: `Bearer ${app.revoked.key}` })).status).toBe(401);
    const post = (route: string, body: object) => fetch(url + route, {method: "POST", headers: {host: app.hostname, origin: `http://${app.hostname}`, "content-type": "application/json"}, body: JSON.stringify(body)});
    expect((await post("/_konae/auth/login", { email: "retired@copy.test", password })).status).toBe(403);
    expect((await post("/_konae/enter", { token: app.entry })).status).toBe(410);
    expect((await post("/_konae/auth/reset", { token: app.reset, password })).status).toBe(410);
    checkReceiver(root);
    const inspection = await owner.call<{ deployment: { hostname: string } }>("project_inspect", { projectId });
    expect(inspection.deployment.hostname).toBe(app.hostname);
    expect(await savedSources(owner)).toEqual(app.atCut);
    const restorePoint = async (backupId: string) => {
      const before = await backups(owner);
      expect(before.map(b => b.id)).toContain(backupId);
      await owner.call("database_execute", { projectId, operation: "restore-backup", input: { backupId } });
      expect(await backups(owner)).toHaveLength(before.length + 1);
    };
    await restorePoint(app.data);
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toEqual([
      { id: 1, title: "Before the copy" }, { id: 2, title: "Before the import" },
    ]);
    expect(await writer.download(await writer.call<Transfer>("files_read", { fileId: app.heldFile }))).toEqual(historicalBytes);
    const stranger = withHeaders(`${url}/_konae/mcp`, { host: app.hostname, cookie: await sign(url, "/_konae/auth/signup", { email: "other@copy.test", password }, app.hostname) });
    await expect(stranger.call("files_read", { fileId: app.heldFile })).rejects.toThrow("File not found");
    await restorePoint(app.sources);
    expect(await savedSources(owner)).toEqual({ files: app.original.files, revision: app.atCut.revision + 1 });
    await restorePoint(app.flat);
    expect(await writer.call("actions_invoke", { name: "notes.list" })).toEqual([]);
  } finally {
    await stop(child);
  }
}

async function checkHeld(root: string, host: string) {
  checkWork(root, projectId, true);
  checkRevocations(root, projectId, false);
  const { child, url } = await startKernel(root, { KONAE_EXECUTION_MODE: "test" });
  try {
    expect((await fetch(url, { headers: { host } })).status).toBe(503);
    expect((await fetch(url + "/_konae/owner/login", { method: "POST" })).status).toBe(503);
    checkWork(root, projectId, true);
  } finally { await stop(child); }
}

async function reconcile(source: string, root: string, extra: Record<string,string> = {}) {
  const child = Bun.spawn([binary, "reconcile", "--from", source], { env: kernelEnv(root, extra), stdout: "pipe", stderr: "inherit" });
  const message = await new Response(child.stdout).text();
  expect(await child.exited).toBe(0);
  expect(message).toContain("2 rows changed");
  checkRevocations(root, projectId, true);
  checkWork(root, projectId, true);
}

describe("copies off the volume", () => {
  test("a directory copy restores into an empty data directory and serves the same app", async () => {
    const scratch = await mkdtemp(path.join(tmpdir(), "konae-copy-dir-"));
    try {
      const copies = path.join(scratch, "second-disk");
      const app = await publishAndCopy(path.join(scratch, "source"), { KONAE_COPY_DIR: copies });
      const copy = app.copy;
      await rm(path.join(scratch, "source"), { recursive: true });
      await verifyHistory(copies, copy);
      const restored = path.join(scratch, "restored");
      const marker = path.join(copies, "revocations/started.json");
      await rename(marker, marker + ".away");
      const first = await restore(copies, restored, {}, copy);
      expect(first.output).toContain("starts in maintenance");
      await checkHeld(restored, app.hostname);
      await rename(marker + ".away", marker);
      await reconcile(copies, restored);
      expect(first.output).toContain("checksums verified");
      expect(first.status).toBe(0);
      const sentinel = path.join(restored, "occupied.txt");
      await Bun.write(sentinel, "existing bytes");
      const again = await restore(copies, restored);
      expect(again.status).not.toBe(0);
      expect(again.output).toContain("not empty");
      expect(await Bun.file(sentinel).text()).toBe("existing bytes");
      await expectSameApp(restored, app);
    } finally {
      await rm(scratch, { recursive: true, force: true });
    }
  }, 240_000);

  test.skipIf(!minio)("an S3 copy restores into an empty data directory and serves the same app", async () => {
    const scratch = await mkdtemp(path.join(tmpdir(), "konae-copy-s3-"));
    const bucket = new Bun.S3Client({ endpoint: s3Endpoint, bucket: s3Bucket, accessKeyId: s3Env.KONAE_S3_ACCESS_KEY, secretAccessKey: s3Env.KONAE_S3_SECRET_KEY });
    const keys = async (prefix: string) => {
      const found: string[] = [];
      let continuationToken: string | undefined;
      do {
        const page = await bucket.list({ prefix, continuationToken });
        found.push(...(page.contents ?? []).map((object) => object.key));
        continuationToken = page.isTruncated ? page.nextContinuationToken : undefined;
      } while (continuationToken);
      return found;
    };
    const copiesOf = async (deployment: string) => {
      const ids: string[] = [];
      for (const key of (await keys("copies/")).filter((key) => /^copies\/[^/]+\/manifest\.json$/.test(key))) {
        const manifest = await bucket.file(key).json().catch(() => undefined) as { files?: { path: string }[] } | undefined;
        if (manifest?.files?.some((file) => file.path.startsWith(`apps/${deployment}/`))) ids.push(key.split("/")[1]);
      }
      return ids.sort();
    };
    let deployment: string | undefined;
    try {
      const sealedEnv = { ...s3Env, KONAE_COPY_SECRET: "a disposable encryption key for nested restore" };
      expect(await keys("")).toHaveLength(0);
      const mirror = path.join(scratch, "cut");
      const app = await publishAndCopy(path.join(scratch, "source"), { ...sealedEnv, KONAE_COPY_S3: "1", KONAE_COPY_DIR: mirror });
      deployment = app.id;
      const copy = app.copy;
      const local = await Bun.file(path.join(mirror, copy, "manifest.json")).json();
      const sealed = await bucket.file(`copies/${copy}/manifest.json`).json();
      const contents = (manifest: typeof local) => manifest.files.map(({path, size, sha256}: {path:string;size:number;sha256:string}) => [path,size,sha256]);
      expect(contents(sealed)).toEqual(contents(local));
      await rm(mirror, { recursive: true });
      const liveObjects = await keys(`files/${deployment}/`);
      expect(liveObjects.length).toBeGreaterThan(0);
      await rm(path.join(scratch, "source"), { recursive: true });
      await Promise.all(liveObjects.map(key => bucket.delete(key)));
      const restored = path.join(scratch, "restored");
      await verifyHistory(`s3://${s3Bucket}/copies/`, copy!, sealedEnv);
      const source = `s3://${s3Bucket}/copies/`;
      const ledger = (await keys("copies/revocations/")).find(k => !k.endsWith("started.json"))!;
      const signed = await bucket.file(ledger).json();
      const { mac, ...unsigned } = signed;
      expect(typeof mac).toBe("string");
      await bucket.write(ledger, JSON.stringify(unsigned));
      const first = await restore(source, restored, sealedEnv, copy);
      expect(first.output).toContain("no MAC");
      await checkHeld(restored, app.hostname);
      const missingKey = Bun.spawn([binary, "reconcile", "--from", source], { env: kernelEnv(restored, s3Env), stdout: "ignore", stderr: "pipe" });
      expect(await new Response(missingKey.stderr).text()).toContain("stays in maintenance");
      expect(await missingKey.exited).not.toBe(0);
      checkRevocations(restored, projectId, false);
      await bucket.write(ledger, JSON.stringify(signed));
      await reconcile(source, restored, sealedEnv);
      expect(first.output).toContain("checksums verified");
      expect(first.status).toBe(0);
      await expectSameApp(restored, app);
    } finally {
      await rm(scratch, { recursive: true, force: true });
      if (deployment) {
        const copies = await copiesOf(deployment).catch(() => [] as string[]);
        const added = [...(await Promise.all(copies.map((copy) => keys(`copies/${copy}/`)))).flat(), ...await keys(`files/${deployment}/`).catch(() => [] as string[])];
        await Promise.all(added.map((key) => bucket.delete(key)));
      }
    }
  }, 240_000);
});
