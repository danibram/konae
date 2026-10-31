import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { exportLocal } from "./konae";

const config = {
  url: "http://127.0.0.1:1",
  project: { id: "offline_export", name: "Offline export", slug: "offline-export", manifest: { version: 1 } },
};

test("CLI exports a source-only kit offline, keeps it private and refuses overwrite", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-cli-export-"));
  const output = path.join(root, "selfhost.tar");
  try {
    await writeFile(path.join(root, "app.js"), "console.log('portable');");
    await writeFile(path.join(root, "schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);");
    await writeFile(path.join(root, "actions.json"), JSON.stringify({ version: 1, actions: {} }));
    expect(await exportLocal(root, config, output)).toBe(output);
    expect((await stat(output)).mode & 0o777).toBe(0o600);
    const archive = await readFile(output);
    const tar = archive.toString("utf8");
    const names: string[] = [];
    for (let offset = 0; offset + 512 <= archive.length && archive[offset] !== 0;) {
      names.push(archive.subarray(offset, offset + 100).toString("utf8").split("\0")[0]);
      const size = Number.parseInt(archive.subarray(offset + 124, offset + 136).toString("utf8"), 8);
      offset += 512 + Math.ceil(size / 512) * 512;
    }
    expect(tar).toContain("kernel-rs/src/limits.rs");
    expect(tar).toContain('"visibility": "private"');
    expect(tar).toContain("console.log('portable')");
    expect(names).not.toContain("konae/initial-identity.sqlite");
    await expect(exportLocal(root, config, output)).rejects.toMatchObject({ code: "EEXIST" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 30_000);

function readArchive(archive: Buffer) {
  const files = new Map<string, Buffer>();
  for (let offset = 0; offset + 512 <= archive.length && archive[offset] !== 0;) {
    const name = archive.subarray(offset, offset + 100).toString("utf8").split("\0")[0];
    const size = Number.parseInt(archive.subarray(offset + 124, offset + 136).toString("utf8"), 8);
    files.set(name, archive.subarray(offset + 512, offset + 512 + size));
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  return files;
}

async function packageServer(visibility: unknown, refuse = false) {
  const database = new TextEncoder().encode("SQLite snapshot");
  const sha256 = Buffer.from(await crypto.subtle.digest("SHA-256", database)).toString("hex");
  const manifest = {
    format: "konae-package", version: 1, packageId: "package-1", createdAt: 1, deploymentId: "deployed-private", projectId: "offline_export",
    visibility, signupPolicy: { mode: "closed", allowlist: [] }, publishedRevision: 3, contents: [{ name: "database", size: database.length, sha256 }], excluded: [],
  };
  const calls: string[] = [];
  const server = Bun.serve({
    port: 0, hostname: "127.0.0.1",
    async fetch(request) {
      const message = await request.json() as { id?: string; method: string; params?: { name: string } };
      if (!message.id) return new Response(null, { status: 202 });
      if (message.method === "initialize") return Response.json({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: "2025-11-25" } });
      const name = message.params?.name ?? "";
      calls.push(name);
      if (name === "package_export" && refuse) return Response.json({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: "Unknown tool or insufficient permission." } });
      const result = name === "project_inspect" ? { deployment: { id: "deployed-private", manifest: { version: 1, visibility: "public" } } }
        : name === "package_export" ? { manifest, downloads: { database: { transferId: "database", size: database.length, sha256, chunkBytes: 65536 } } }
        : name === "transfer_read" ? { data: Buffer.from(database).toString("base64"), offset: 0, nextOffset: database.length } : {};
      return Response.json({ jsonrpc: "2.0", id: message.id, result: { structuredContent: { result } } });
    },
  });
  return { server, calls, database };
}

async function exportWith(visibility: unknown, refuse = false) {
  const { server, calls, database } = await packageServer(visibility, refuse);
  const root = await mkdtemp(path.join(tmpdir(), "konae-cli-live-export-"));
  await writeFile(path.join(root, "app.js"), "console.log('private');");
  await writeFile(path.join(root, "schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);");
  await writeFile(path.join(root, "actions.json"), JSON.stringify({ version: 1, actions: {} }));
  const output = path.join(root, "selfhost.tar");
  const stale = { ...config, url: `http://127.0.0.1:${server.port}`, project: { ...config.project, manifest: { version: 1, visibility: "public" } } };
  const done = async () => { server.stop(true); await rm(root, { recursive: true, force: true }); };
  return { run: () => exportLocal(root, stale, output, "owner-token"), output, calls, database, done };
}

test("export --with-data writes the package manifest and streams each component into the kit", async () => {
  const { run, output, calls, database, done } = await exportWith("private");
  try {
    await run();
    expect((await stat(output)).mode & 0o777).toBe(0o600);
    const files = readArchive(await readFile(output));
    expect(JSON.parse(files.get("konae/project.json")!.toString()).manifest.visibility).toBe("private");
    expect(JSON.parse(files.get("konae/package/manifest.json")!.toString())).toMatchObject({ version: 1, publishedRevision: 3, signupPolicy: { mode: "closed" } });
    expect(new Uint8Array(files.get("konae/package/database.sqlite")!)).toEqual(database);
    expect(files.has("konae/initial-database.sqlite")).toBe(false);
    expect(calls).toEqual(["project_inspect", "package_export", "transfer_read", "transfer_cancel", "transfer_cancel"]);
  } finally { await done(); }
}, 30_000);

for (const visibility of [undefined, "invalid"]) test(`export --with-data refuses a package with ${visibility ?? "no"} visibility`, async () => {
  const { run, output, calls, done } = await exportWith(visibility);
  try {
    await expect(run()).rejects.toThrow("no valid visibility");
    await expect(stat(output)).rejects.toMatchObject({ code: "ENOENT" });
    expect(calls).toEqual(["project_inspect", "package_export", "transfer_cancel"]);
  } finally { await done(); }
}, 30_000);

test("export --with-data names export_data when the credential lacks it", async () => {
  const { run, output, calls, done } = await exportWith("private", true);
  try {
    await expect(run()).rejects.toThrow("export_data");
    await expect(stat(output)).rejects.toMatchObject({ code: "ENOENT" });
    expect(calls).toEqual(["project_inspect", "package_export"]);
  } finally { await done(); }
}, 30_000);
