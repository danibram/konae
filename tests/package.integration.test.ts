import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtemp, open, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { McpClient, checksum, encodeBase64, type Transfer } from "../src/mcp/client";
import { readTar } from "../src/lib/tar";
import { downloadToFile } from "../scripts/package-files";
import type { PackageManifest } from "../src/lib/portable-package";
import type { UsageSummary } from "../src/lib/usage";

// The chunked package of ADR-0008 and every refusal its import checks.
// tests/smoke/package-quota.ts exports a 256 MiB app with its data and runs
// it on the kernel binary.
let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let url: string;
let owner: McpClient;
const setupKey = "package-integration-setup-secret";
const password = "package-integration-password";
let nextClient = 0;
const freshClient = () => { nextClient++; return `198.19.${Math.floor(nextClient / 250)}.${nextClient % 250 + 1}`; };

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-package-"));
  const port = Number(process.env.KONAE_PACKAGE_TEST_PORT) || 19800 + Math.floor(Math.random() * 90);
  url = `http://127.0.0.1:${port}`;
  kernel = Bun.spawn(["kernel-rs/target/debug/konae"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))), KONAE_DATA_DIR: path.join(root, "data"), KONAE_KERNEL_PORT: String(port), KONAE_BIND: "127.0.0.1", KONAE_ADMIN_KEY: setupKey, KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost", KONAE_TRUSTED_PROXIES: "127.0.0.1" },
    stdout: "ignore", stderr: "inherit",
  });
  for (let attempt = 0; attempt < 100; attempt++) {
    try { if ((await fetch(`${url}/_konae/health`)).ok) break; } catch {}
    if (kernel.exitCode !== null) throw Error("The package test Kernel stopped.");
    await Bun.sleep(100);
  }
  const response = await fetch(`${url}/_konae/owner/signup`, { method: "POST", headers: { "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email: "owner@package.test", password, setupKey }) });
  expect(response.status).toBe(200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  owner = new McpClient(`${url}/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}, 300_000);

afterAll(async () => { kernel?.kill(); if (kernel) await kernel.exited; if (root) await rm(root, { recursive: true, force: true }); });

function payload(id: string) {
  return {
    project: { id, name: "Package proof", slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files: {
      "schema.sql": "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);",
      "actions.json": JSON.stringify({ version: 1, actions: {
        "notes.list": { kind: "query", access: "authenticated", statement: "SELECT id,title FROM notes WHERE user_id=?1 ORDER BY id LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
        "notes.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title,user_id) VALUES(?1,?2)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
      } }),
    } }, visibility: "public", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>Package proof</main>" } },
  };
}

async function release(client: McpClient, request: object, projectId: string) {
  const upload = await client.upload(new TextEncoder().encode(JSON.stringify(request)), projectId);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: upload.transferId });
    return { planId: plan.planId, receipt: await client.call<{ id: string; hostname: string }>("release_apply", { planId: plan.planId }) };
  } finally { await client.cancel(upload.transferId).catch(() => {}); }
}

async function publish(id: string) {
  await owner.call("project_create", { projectId: id, name: "Package proof" });
  return (await release(owner, payload(id), id)).receipt;
}

function runtime(hostname: string, cookie?: string) {
  return new McpClient(`${url}/_konae/mcp`, { fetch: (input, init) => {
    const headers = new Headers(init?.headers); headers.set("host", hostname);
    if (cookie) headers.set("cookie", cookie);
    return fetch(input, { ...init, headers });
  } });
}

async function account(hostname: string, route: "signup" | "login", email: string) {
  const response = await fetch(`${url}/_konae/auth/${route}`, { method: "POST", headers: { host: hostname, "content-type": "application/json", "x-forwarded-for": freshClient() }, body: JSON.stringify({ email, password }) });
  expect(response.status).toBe(200);
  return response.headers.get("set-cookie")!.split(";")[0];
}

async function delegate(scopes: string[], projectIds: string[]) {
  const { token } = await owner.call<{ token: string }>("token_create", { scopes, projectIds, ttlSeconds: 600 });
  return new McpClient(`${url}/mcp`, { token });
}

type Exported = { manifest: PackageManifest; downloads: Record<string, Transfer>; folder: Transfer; sourcesAhead: boolean };

/** Downloads each data component to a file, and reads the folder download: konae.json, app/ and dist/. */
async function download(client: McpClient, exported: Exported, directory: string) {
  const files: Record<string, string> = {};
  for (const content of exported.manifest.contents) {
    files[content.name] = path.join(directory, content.name);
    await downloadToFile(client, exported.downloads[content.name], files[content.name]);
  }
  try {
    folders.push(readTar(await client.download(exported.folder)).map((entry) => entry.path));
  } finally { await client.cancel(exported.folder.transferId).catch(() => {}); }
  return files;
}
/** The paths of each folder download, newest last. */
const folders: string[][] = [];

async function uploadFile(client: McpClient, file: string, content: { name: string; size: number; sha256: string }, projectId: string) {
  const transfer = await client.call<Transfer>("transfer_begin", { projectId, size: content.size, sha256: content.sha256, component: content.name });
  const handle = await open(file, "r");
  try {
    const buffer = new Uint8Array(transfer.chunkBytes);
    for (let offset = 0; offset < content.size;) {
      const { bytesRead } = await handle.read(buffer, 0, Math.min(buffer.length, content.size - offset), offset);
      await client.call("transfer_write", { transferId: transfer.transferId, offset, data: encodeBase64(buffer.subarray(0, bytesRead)) });
      offset += bytesRead;
    }
  } finally { await handle.close(); }
  await client.call("transfer_complete", { transferId: transfer.transferId });
  return transfer.transferId;
}

async function uploadPackage(client: McpClient, manifest: PackageManifest, files: Record<string, string>, projectId: string) {
  const components: Record<string, string> = {};
  for (const content of manifest.contents) components[content.name] = await uploadFile(client, files[content.name], content, projectId);
  return components;
}

/** A source app with one account, one row and one file, exported as a package into `directory`. */
async function sourcePackage(id: string, directory: string) {
  const source = await publish(id);
  const cookie = await account(source.hostname, "signup", `writer@${id}.test`);
  const writer = runtime(source.hostname, cookie);
  await writer.call("actions_invoke", { name: "notes.create", input: { title: "Moved with the package" } });
  const upload = await writer.upload(new TextEncoder().encode("attachment bytes"));
  const file = await writer.call<{ id: string }>("files_upload", { transferId: upload.transferId, name: "note.txt", type: "text/plain" });
  await writer.cancel(upload.transferId);
  const key = await owner.call<{ keyId: string; key: string }>("api_keys_create", { deploymentId: source.id, name: "Mirror", files: ["public"] });
  const exported = await owner.call<Exported>("project_export", { deploymentId: source.id });
  return { source, cookie, fileId: file.id, key, exported, files: await download(owner, exported, directory) };
}

describe("chunked package", () => {
  test("an export compares every saved file with the release and clears the warning after a revert", async () => {
    const id = "pkg_source_digest";
    const request = payload(id);
    const original = { ...request.project.files, "index.html": "<main>Package proof</main>", "app.js": "console.log('published');\n", "metadata.txt": "published metadata" };
    const manifest = { ...request.project.manifest, runtime: { database: { mode: "declared-actions", actions: [] }, auth: { mode: "optional" }, files: { access: "deployment" } } };
    const source = { ...request.project, manifest, files: original, directories: [], updatedAt: 1 };
    await owner.call("project_create", { projectId: id, name: source.name });
    let revision = 0;
    const save = async (files: Record<string, string>) => {
      const upload = await owner.upload(new TextEncoder().encode(JSON.stringify({ ...source, files, updatedAt: revision + 1 })), id);
      try {
        const saved = await owner.call<{ revision: number }>("project_sources_save", { projectId: id, transferId: upload.transferId, expectedRevision: revision });
        revision = saved.revision;
      } finally { await owner.cancel(upload.transferId); }
    };
    await save(original);
    const { receipt } = await release(owner, { ...request, project: source }, id);
    const ahead = async () => {
      const exported = await owner.call<Exported>("project_export", { deploymentId: receipt.id });
      try { return exported.sourcesAhead; }
      finally {
        for (const transfer of [exported.folder, ...Object.values(exported.downloads)]) await owner.cancel(transfer.transferId);
      }
    };
    expect(await ahead()).toBe(false);
    await save({ ...original, "app.js": "console.log('edited');\n" });
    expect(await ahead()).toBe(true);
    await save(original);
    expect(await ahead()).toBe(false);
    await save({ ...original, "metadata.txt": "edited metadata" });
    expect(await ahead()).toBe(true);
    await save(original);
    expect(await ahead()).toBe(false);
  });

  test("moves rows, accounts and files into a new app, drops sessions, and a repeated apply returns its receipt", async () => {
    const directory = await mkdtemp(path.join(root, "export-"));
    const { source, cookie, fileId, key, exported, files } = await sourcePackage("pkg_source", directory);
    const manifest = exported.manifest;
    expect(manifest).toMatchObject({ format: "konae-package", version: 1, deploymentId: source.id, projectId: "pkg_source", visibility: "public", publishedRevision: 1 });
    // The signup, the action and the upload above wrote a Usage store, which travels too.
    expect(manifest.contents.map((content) => content.name)).toEqual(["database", "identity", "files", "usage"]);
    expect(manifest.excluded).toContain("app sessions");
    // The folder download is the rest of the app folder, and never a secret or the installation's records.
    const folder = folders.at(-1)!;
    // An app published without an editor save keeps its release's sources in app/.
    expect(folder).toEqual(expect.arrayContaining(["konae.json", "app/actions.json", "app/schema.sql", "dist/index.html"]));
    expect(folder.every((entry) => entry === "konae.json" || entry.startsWith("app/") || entry.startsWith("dist/"))).toBe(true);
    expect(manifest.excluded).not.toContain("Usage and Signals");
    for (const content of manifest.contents) expect(await checksum(new Uint8Array(await readFile(files[content.name])))).toBe(content.sha256);
    const identity = new Database(files.identity, { readonly: true });
    try {
      expect(identity.query("SELECT COUNT(*) n FROM app_users").get()).toEqual({ n: 1 });
      expect(identity.query("SELECT COUNT(*) n FROM app_sessions").get()).toEqual({ n: 0 });
      // The key leaves already revoked, so the component opens nothing.
      expect(identity.query("SELECT id, revoked_reason FROM api_keys WHERE revoked_at IS NOT NULL").all()).toEqual([{ id: key.keyId, revoked_reason: "import" }]);
    } finally { identity.close(); }

    await owner.call("project_create", { projectId: "pkg_target", name: "Imported" });
    const components = await uploadPackage(owner, manifest, files, "pkg_target");
    const first = await release(owner, { ...payload("pkg_target"), initialPackage: { manifest, components } }, "pkg_target");
    expect(first.receipt.signupPolicy).toEqual(manifest.signupPolicy);
    expect(await owner.call("release_apply", { planId: first.planId })).toEqual(first.receipt);
    const inspected = await owner.call<{ deployment: { id: string } }>("project_inspect", { projectId: "pkg_target" });
    expect(inspected.deployment.id).toBe(first.receipt.id);
    const today = new Date().toISOString().slice(0, 10);
    const yesterday = new Date(Date.now() - 86_400_000).toISOString().slice(0, 10);
    const usage = await owner.call<{ totals: { kind: string; name: string; count: number }[] }>("usage_read", { deploymentId: first.receipt.id, from: yesterday, to: today });
    expect(usage.totals).toEqual(expect.arrayContaining([
      { kind: "signup", name: "", count: 1 }, { kind: "action", name: "notes.create", count: 1 }, { kind: "upload", name: "", count: 1 },
    ]));

    const monitor = await owner.call<UsageSummary>("usage_summary", { deploymentId: first.receipt.id, from: yesterday, to: today });
    expect(monitor.coverage.startedAt).toBeNumber();
    expect(monitor.metrics).toEqual(expect.arrayContaining([expect.objectContaining({ name: "notes.create", origin: "runtime", count: 1 })]));
    expect(monitor.accounts).toMatchObject({ active: 1, opens: 0, actions: 1, top: [
      { email: "writer@pkg_source.test", actions: 1 },
    ] });
    expect(monitor.storage?.files).toBe(1);

    const moved = runtime(first.receipt.hostname, await account(first.receipt.hostname, "login", "writer@pkg_source.test"));
    expect(await moved.call("actions_invoke", { name: "notes.list" })).toMatchObject([{ title: "Moved with the package" }]);
    const continued = await owner.call<UsageSummary>("usage_summary", { deploymentId: first.receipt.id, from: yesterday, to: today });
    expect(continued.accounts).toMatchObject({ active: 1, actions: 2, top: [
      { account: monitor.accounts!.top[0].account, email: "writer@pkg_source.test", actions: 2 },
    ] });
    expect(continued.metrics).toEqual(expect.arrayContaining([
      expect.objectContaining({ name: "notes.create", origin: "runtime", count: 1 }),
      expect.objectContaining({ name: "notes.list", origin: "runtime", count: 1 }),
    ]));
    expect(new TextDecoder().decode(await moved.download(await moved.call<Transfer>("files_read", { fileId })))).toBe("attachment bytes");
    expect(await runtime(first.receipt.hostname, cookie).call("auth_session")).toMatchObject({ role: "anonymous" });
    // The destination lists the origin's key as revoked at import, and refuses it.
    const keys = await owner.call<{ keys: { keyId: string; name: string; files: string[]; revokedAt: number | null; revokedReason: string | null }[] }>("api_keys_list", { deploymentId: first.receipt.id });
    expect(keys.keys).toEqual([expect.objectContaining({ keyId: key.keyId, name: "Mirror", files: ["public"], revokedAt: expect.any(Number), revokedReason: "import" })]);
    const refused = await fetch(`${url}/_konae/files/${fileId}`, { headers: { host: first.receipt.hostname, "x-forwarded-for": freshClient(), authorization: `Bearer ${key.key}` } });
    expect(refused.status).toBe(401);
    for (const id of Object.values(components)) await owner.cancel(id);
  }, 60_000);

  test("refuses a package for a project that already has a deployment", async () => {
    const directory = await mkdtemp(path.join(root, "export-"));
    const { exported, files } = await sourcePackage("pkg_over_source", directory);
    await publish("pkg_existing");
    const components = await uploadPackage(owner, exported.manifest, files, "pkg_existing");
    await expect(release(owner, { ...payload("pkg_existing"), initialPackage: { manifest: exported.manifest, components } }, "pkg_existing")).rejects.toThrow("first deployment");
    for (const id of Object.values(components)) await owner.cancel(id);
  }, 60_000);

  test("refuses a component whose bytes or checksum do not match the manifest", async () => {
    const directory = await mkdtemp(path.join(root, "export-"));
    const { exported, files } = await sourcePackage("pkg_checksum_source", directory);
    await owner.call("project_create", { projectId: "pkg_checksum", name: "Checksum" });
    const database = exported.manifest.contents.find((content) => content.name === "database")!;

    // Bytes that differ from the declared SHA-256 never complete.
    const tampered = path.join(directory, "tampered.sqlite");
    const bytes = new Uint8Array(await readFile(files.database));
    bytes[bytes.length - 1] ^= 0xff;
    await writeFile(tampered, bytes);
    await expect(uploadFile(owner, tampered, database, "pkg_checksum")).rejects.toThrow("SHA-256 mismatch");

    // An upload that completed with its own checksum but not the manifest's is refused before any Deployment exists.
    const components = await uploadPackage(owner, exported.manifest, files, "pkg_checksum");
    await owner.cancel(components.database);
    components.database = await uploadFile(owner, tampered, { ...database, sha256: await checksum(bytes) }, "pkg_checksum");
    await expect(release(owner, { ...payload("pkg_checksum"), initialPackage: { manifest: exported.manifest, components } }, "pkg_checksum")).rejects.toThrow("does not match the size and SHA-256 in the manifest");

    // A manifest from a future version is refused with the version it names.
    await expect(release(owner, { ...payload("pkg_checksum"), initialPackage: { manifest: { ...exported.manifest, version: 3 }, components } }, "pkg_checksum")).rejects.toThrow("Package version 3 is not supported");
    await expect(release(owner, { ...payload("pkg_checksum"), initialPackage: { manifest: { ...exported.manifest, version: 2 }, components } }, "pkg_checksum")).rejects.toThrow("Package version 2 is not supported");
    expect((await owner.call<{ deployment: unknown }>("project_inspect", { projectId: "pkg_checksum" })).deployment).toBeNull();
    for (const id of Object.values(components)) await owner.cancel(id).catch(() => {});
  }, 60_000);

  test("a component may not exceed its quota", async () => {
    await owner.call("project_create", { projectId: "pkg_quota", name: "Quota" });
    const sha256 = "a".repeat(64);
    await expect(owner.call("transfer_begin", { projectId: "pkg_quota", size: 64 * 1024 * 1024 + 1, sha256, component: "identity" })).rejects.toThrow("for the identity component");
    await expect(owner.call("transfer_begin", { projectId: "pkg_quota", size: 32 * 1024 * 1024 + 1, sha256 })).rejects.toThrow("outside the limit");
    const accepted = await owner.call<Transfer>("transfer_begin", { projectId: "pkg_quota", size: 256 * 1024 * 1024, sha256, component: "database" });
    await owner.cancel(accepted.transferId);
  });

  test("downloading state needs export_data; read, write and publish do not grant it", async () => {
    const source = await publish("pkg_permission");
    await publish("pkg_other");
    for (const scopes of [["read"], ["publish"], ["read", "write", "publish"]]) {
      const client = await delegate(scopes, ["pkg_permission"]);
      for (const name of ["project_export"]) {
        await expect(client.call(name, { deploymentId: source.id })).rejects.toThrow("Unknown tool or insufficient permission");
      }
    }
    const elsewhere = await delegate(["export_data"], ["pkg_other"]);
    await expect(elsewhere.call("project_export", { deploymentId: source.id })).rejects.toThrow("outside this token's grants");
    const exporter = await delegate(["export_data"], ["pkg_permission"]);
    const exported = await exporter.call<Exported>("project_export", { deploymentId: source.id });
    for (const transfer of [...Object.values(exported.downloads), exported.folder]) await exporter.cancel(transfer.transferId);
  }, 30_000);

  test("a read credential cannot read the transfers of an export, and each download is audited once", async () => {
    const source = await publish("pkg_leak");
    const writer = runtime(source.hostname, await account(source.hostname, "signup", "writer@leak.test"));
    await writer.call("actions_invoke", { name: "notes.create", input: { title: "first" } });
    // About 300 KiB of rows, so the database download takes several blocks.
    await owner.call("database_execute", { deploymentId: source.id, operation: "sql", input: { sql: "INSERT INTO notes(title,user_id) WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM c WHERE x<300) SELECT hex(randomblob(500)),'bulk' FROM c" } });
    const exported = await owner.call<Exported>("project_export", { deploymentId: source.id });
    expect(exported.downloads.database.size).toBeGreaterThan(4 * 64 * 1024);
    const reader = await delegate(["read"], ["pkg_leak"]);
    for (const transfer of Object.values(exported.downloads)) {
      await expect(reader.call("transfer_read", { transferId: transfer.transferId, offset: 0 })).rejects.toThrow("Permission denied for export_data");
      await expect(reader.call("transfer_status", { transferId: transfer.transferId })).rejects.toThrow("Permission denied for export_data");
    }
    const directory = await mkdtemp(path.join(root, "export-"));
    await download(owner, exported, directory);
    const db = new Database(path.join(root, "data", "mcp.sqlite"));
    try {
      const rows = db.query("SELECT detail,ok FROM audit WHERE tool='package_download' AND target=?1 ORDER BY id").all(source.id) as { detail: string; ok: number }[];
      const details = (ok: number) => rows.filter((row) => row.ok === ok).map((row) => row.detail).sort();
      // The source app recorded Usage, so its package has a fourth component.
      expect(details(0)).toEqual(["database", "files", "identity", "usage"]);
      expect(details(1)).toEqual(["database", "files", "folder", "identity", "usage"]);
      const reads = db.query("SELECT COUNT(*) n FROM audit WHERE tool='transfer_read'").get() as { n: number };
      expect(reads.n).toBe(0);
      // Component uploads in the earlier tests wrote one row per component, not one per block.
      const writes = db.query("SELECT COUNT(*) n FROM audit WHERE tool='transfer_write'").get() as { n: number };
      const completes = db.query("SELECT COUNT(*) n FROM audit WHERE tool='transfer_complete' AND ok=1").get() as { n: number };
      expect(writes.n).toBeLessThanOrEqual(completes.n + 2);
      expect(db.query("SELECT COUNT(*) n FROM audit WHERE tool='project_export' AND target=?1 AND ok=1").get(source.id)).toEqual({ n: 1 });
    } finally { db.close(); }
  }, 30_000);
});
