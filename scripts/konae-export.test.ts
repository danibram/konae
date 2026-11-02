import { expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { createTar } from "../src/lib/tar";
import { exportLocal } from "./konae";

const settings = {
  id: "offline_export", name: "Offline export", slug: "offline-export", visibility: "private",
  signupPolicy: { mode: "invitation", allowlist: [] }, signInMethods: ["password"], kernel: "0.1.0", manifest: { version: 1 },
};

/** An app folder with konae.json and app/, as init writes it. */
async function appFolder(prefix: string) {
  const root = await mkdtemp(path.join(tmpdir(), prefix));
  await mkdir(path.join(root, "app"));
  await writeFile(path.join(root, "konae.json"), JSON.stringify(settings));
  await writeFile(path.join(root, "app", "app.js"), "console.log('portable');");
  await writeFile(path.join(root, "app", "schema.sql"), "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY);");
  await writeFile(path.join(root, "app", "actions.json"), JSON.stringify({ version: 1, actions: {} }));
  await mkdir(path.join(root, ".konae"));
  await writeFile(path.join(root, ".konae", "credentials.json"), "{}");
  return root;
}

async function listed(root: string, prefix = ""): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(path.join(root, prefix), { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...await listed(root, relative)); else result.push(relative);
  }
  return result.sort();
}

test("export writes the app folder offline: settings, sources, dist and the generated files, never .konae", async () => {
  const root = await appFolder("konae-cli-export-");
  const output = path.join(root, "..", `${path.basename(root)}-out`);
  try {
    expect(await exportLocal(root, output)).toEqual({ output, data: false });
    const files = await listed(output);
    expect(files).toEqual(expect.arrayContaining([
      "konae.json", "app/app.js", "app/actions.json", "app/schema.sql", "dist/index.html",
      "AGENTS.md", "action-contract.md", "actions.d.ts", "compose.yaml", ".env.example", ".gitignore", "package.json", "scripts/build.mjs",
    ]));
    expect(files.some((file) => file.startsWith(".konae") || file.startsWith("data/"))).toBe(false);
    expect(JSON.parse(await readFile(path.join(output, "konae.json"), "utf8"))).toEqual(settings);
    expect(await readFile(path.join(output, "dist/index.html"), "utf8")).toContain("<script data-konae-runtime>");
    await expect(exportLocal(root, output)).rejects.toThrow("Refusing to overwrite");
  } finally {
    await rm(root, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
  }
}, 30_000);

/** A Kernel stand-in that answers project_export with one database and the folder download. */
async function exportServer(refuse = false) {
  const database = new TextEncoder().encode("SQLite snapshot");
  const filesTar = createTar([{ path: "index.json", contents: "[]" }]);
  const digest = async (bytes: Uint8Array) => Buffer.from(await crypto.subtle.digest("SHA-256", bytes)).toString("hex");
  const manifest = {
    format: "konae-package", version: 1, packageId: "package-1", createdAt: 1, deploymentId: "offline_export", projectId: "offline_export",
    visibility: "private", signupPolicy: { mode: "closed", allowlist: [] }, publishedRevision: 3,
    contents: [{ name: "database", size: database.length, sha256: await digest(database) }, { name: "files", size: filesTar.length, sha256: await digest(filesTar) }], excluded: [],
  };
  const bodies: Record<string, Uint8Array> = { database, files: filesTar };
  const calls: string[] = [];
  const server = Bun.serve({
    port: 0, hostname: "127.0.0.1",
    async fetch(request) {
      const message = await request.json() as { id?: string; method: string; params?: { name: string; arguments: { transferId?: string } } };
      if (!message.id) return new Response(null, { status: 202 });
      if (message.method === "initialize") return Response.json({ jsonrpc: "2.0", id: message.id, result: { protocolVersion: "2025-11-25" } });
      const name = message.params?.name ?? "";
      calls.push(name);
      if (name === "project_export" && refuse) return Response.json({ jsonrpc: "2.0", id: message.id, error: { code: -32602, message: "Unknown tool or insufficient permission." } });
      const transfer = async (id: string) => ({ transferId: id, size: bodies[id].length, sha256: await digest(bodies[id]), chunkBytes: 65536 });
      const result = name === "project_inspect" ? { deployment: { id: "offline_export" } }
        : name === "project_export" ? { manifest, downloads: { database: await transfer("database"), files: await transfer("files") }, folder: { transferId: "folder", size: 1, sha256: "0".repeat(64), chunkBytes: 65536 }, sourcesAhead: false }
        : name === "transfer_read" ? (() => { const body = bodies[message.params!.arguments.transferId!]; return { data: Buffer.from(body).toString("base64"), offset: 0, nextOffset: body.length }; })() : {};
      return Response.json({ jsonrpc: "2.0", id: message.id, result: { structuredContent: { result } } });
    },
  });
  return { server, calls, database };
}

test("export --with-data writes the published app's data into data/ and releases every download", async () => {
  const { server, calls, database } = await exportServer();
  const root = await appFolder("konae-cli-live-export-");
  const output = `${root}-out`;
  try {
    expect(await exportLocal(root, output, { url: `http://127.0.0.1:${server.port}`, token: "owner-token" })).toEqual({ output, data: true });
    expect(new Uint8Array(await readFile(path.join(output, "data/database.sqlite")))).toEqual(database);
    expect(await readFile(path.join(output, "data/files/index.json"), "utf8")).toBe("[]");
    // The two data downloads and the folder download are each released.
    expect(calls.filter((call) => call === "transfer_cancel").length).toBeGreaterThanOrEqual(3);
  } finally {
    server.stop(true);
    await rm(root, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
  }
}, 30_000);

test("export --with-data names export_data when the credential lacks it", async () => {
  const { server, calls } = await exportServer(true);
  const root = await appFolder("konae-cli-refused-export-");
  const output = `${root}-out`;
  try {
    await expect(exportLocal(root, output, { url: `http://127.0.0.1:${server.port}`, token: "owner-token" })).rejects.toThrow("export_data");
    expect(calls).toEqual(["project_inspect", "project_export"]);
  } finally {
    server.stop(true);
    await rm(root, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
  }
}, 30_000);
