import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { exportLocal } from "./littlebox";

const config = {
  url: "http://127.0.0.1:1",
  project: { id: "offline_export", name: "Offline export", slug: "offline-export", manifest: { version: 1 } },
};

test("CLI exports a source-only kit offline, keeps it private and refuses overwrite", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "littlebox-cli-export-"));
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
    expect(names).not.toContain("littlebox/initial-identity.sqlite");
    await expect(exportLocal(root, config, output)).rejects.toMatchObject({ code: "EEXIST" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 30_000);

for (const [inspectedVisibility, snapshotVisibility] of [["public", "private"], ["private", "private"], ["private", undefined], ["private", "invalid"]]) test(`export handles snapshot visibility ${snapshotVisibility ?? "absent"} after ${inspectedVisibility} inspection`, async () => {
  const portable = new TextEncoder().encode(JSON.stringify({ result: {
    initialDatabase: Buffer.from("SQLite snapshot").toString("base64"),
    initialSignupPolicy: { mode: "closed", allowlist: [] }, visibility: snapshotVisibility,
  } }));
  const sha256 = Buffer.from(await crypto.subtle.digest("SHA-256", portable)).toString("hex");
  const calls: string[] = [];
  const server = Bun.serve({
    port: 0, hostname: "127.0.0.1",
    async fetch(request) {
      const message = await request.json() as { id?: string; method: string; params?: { name: string } };
      if (!message.id) return new Response(null, { status: 202 });
      if (message.method === "initialize") return Response.json({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: "2025-11-25" } });
      const name = message.params?.name ?? "";
      calls.push(name);
      const result = name === "project_inspect" ? { deployment: { id: "deployed-private", manifest: { version: 1, visibility: inspectedVisibility } } }
        : name === "project_export" ? { transferId: "snapshot", size: portable.length, sha256, chunkBytes: 65536 }
        : name === "transfer_read" ? { data: Buffer.from(portable).toString("base64"), offset: 0, nextOffset: portable.length } : {};
      return Response.json({ jsonrpc: "2.0", id: message.id, result: { structuredContent: { result } } });
    },
  });
  const root = await mkdtemp(path.join(tmpdir(), "littlebox-cli-live-export-"));
  try {
    await writeFile(path.join(root, "app.js"), "console.log('private');");
    await writeFile(path.join(root, "schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);");
    await writeFile(path.join(root, "actions.json"), JSON.stringify({ version: 1, actions: {} }));
    const output = path.join(root, "selfhost.tar");
    const stale = { ...config, url: `http://127.0.0.1:${server.port}`, project: { ...config.project, manifest: { version: 1, visibility: "public" } } };
    if (snapshotVisibility === "invalid") {
      await expect(exportLocal(root, stale, output, "owner-token")).rejects.toThrow("invalid visibility");
      expect(calls).toEqual(["project_inspect", "project_export", "transfer_read", "transfer_cancel"]);
      return;
    }
    await exportLocal(root, stale, output, "owner-token");
    const archive = await readFile(output);
    const files = new Map<string, string>();
    for (let offset = 0; offset + 512 <= archive.length && archive[offset] !== 0;) {
      const name = archive.subarray(offset, offset + 100).toString("utf8").split("\0")[0];
      const size = Number.parseInt(archive.subarray(offset + 124, offset + 136).toString("utf8"), 8);
      files.set(name, archive.subarray(offset + 512, offset + 512 + size).toString("utf8"));
      offset += 512 + Math.ceil(size / 512) * 512;
    }
    expect(JSON.parse(files.get("littlebox/project.json")!).manifest.visibility).toBe("private");
    expect(JSON.parse(files.get("littlebox/initial-signup-policy.json")!)).toEqual({ mode: "closed", allowlist: [] });
    expect(calls).toEqual(["project_inspect", "project_export", "transfer_read", "transfer_cancel"]);
  } finally {
    server.stop(true);
    await rm(root, { recursive: true, force: true });
  }
}, 30_000);
