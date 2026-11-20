import { expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { tmpdir } from "node:os";
import { createTar, readTar } from "../src/lib/tar";
import { readAppFolder } from "../src/lib/import-folder";
import { sourceBytes } from "../src/lib/source-files";
import { exportLocal } from "./konae";

const settings = {
  id: "offline_export", name: "Offline export", slug: "offline-export", visibility: "private",
  signupPolicy: { mode: "invitation", allowlist: [] }, signInMethods: ["password"], kernel: "0.1.0", manifest: { version: 1 },
};

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

async function customBuildSources(root: string) {
  const files = ["package.json", "package-lock.json", "tsconfig.json", "tsconfig.base.json", "scripts/app-build.mjs", "scripts/local-builder.mjs"];
  const sources: Record<string, string> = {
    "package.json": JSON.stringify({ name: "portable-fixture", private: true, type: "module", scripts: { build: "node scripts/app-build.mjs" }, devDependencies: { esbuild: "0.25.12" } }),
    "package-lock.json": JSON.stringify({ name: "portable-fixture", lockfileVersion: 3, requires: true, packages: { "": { name: "portable-fixture", devDependencies: { esbuild: "0.25.12" } } } }),
    "tsconfig.json": JSON.stringify({ extends: "./tsconfig.base.json", include: ["app/src"] }),
    "tsconfig.base.json": JSON.stringify({ compilerOptions: { baseUrl: ".", paths: { "@repo/*": ["app/vendor/*"], "@/*": ["app/src/*"] } } }),
    "scripts/app-build.mjs": 'import { buildApp } from "./local-builder.mjs"; await buildApp();',
    "scripts/local-builder.mjs": 'import { mkdir, writeFile } from "node:fs/promises"; await mkdir("dist", { recursive: true }); await writeFile("dist/build-executed.txt", "fresh-draft"); export async function buildApp() {}',
  };
  await writeFile(path.join(root, "app/build.json"), JSON.stringify({ version: 1, files }));
  for (const [file, contents] of Object.entries(sources)) {
    const target = path.join(root, "app/build", file);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, contents);
  }
  await mkdir(path.join(root, "app/src"), { recursive: true });
  await mkdir(path.join(root, "app/vendor"), { recursive: true });
  await writeFile(path.join(root, "app/src/main.ts"), 'import { label } from "@repo/ui"; export const app = label;');
  await writeFile(path.join(root, "app/vendor/ui.ts"), 'export const label = "fixture";');
  await writeFile(path.join(root, "app/functions.json"), JSON.stringify({ version: 1, functions: { health: { entry: "functions/health.ts", mount: "/api/health", methods: ["GET"], access: "anonymous" } } }));
  await mkdir(path.join(root, "app/functions"), { recursive: true });
  await writeFile(path.join(root, "app/functions/health.ts"), 'export const handle = () => new Response("draft");');
}

async function extractArchive(archive: Uint8Array, root: string) {
  for (const entry of readTar(archive)) {
    const target = path.join(root, entry.path);
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, entry.contents);
  }
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
    expect(await readFile(path.join(output, "dist/index.html"), "utf8")).not.toContain("data-konae-runtime");
    await expect(exportLocal(root, output)).rejects.toThrow("Refusing to overwrite");
  } finally {
    await rm(root, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
  }
}, 30_000);

test("custom build project exports, reimports and reexports without Git or inherited node_modules", async () => {
  const root = await appFolder("konae-cli-build-project-");
  const output = `${root}-out`;
  const imported = `${root}-imported`;
  const secondExport = `${root}-second-export`;
  try {
    await customBuildSources(root);
    const settingsWithRelease = { ...settings, released: { version: "published-v1" } };
    await writeFile(path.join(root, "konae.json"), JSON.stringify(settingsWithRelease));
    await mkdir(path.join(root, "dist"), { recursive: true });
    await writeFile(path.join(root, "dist/index.html"), "published-v1");
    await mkdir(path.join(root, "server"), { recursive: true });
    await writeFile(path.join(root, "server/manifest.json"), JSON.stringify({ version: "published-v1" }));
    await writeFile(path.join(root, "server/health.mjs"), 'export const handle = () => new Response("published-v1");');
    await mkdir(path.join(root, ".git"), { recursive: true });
    await writeFile(path.join(root, ".git/config"), "synthetic repository metadata");
    await mkdir(path.join(root, "data"), { recursive: true });
    await writeFile(path.join(root, "data/database.sqlite"), "synthetic database");
    await mkdir(path.join(root, "node_modules"), { recursive: true });
    await writeFile(path.join(root, "node_modules/.package-lock.json"), "synthetic cache");
    await writeFile(path.join(root, ".env"), "SYNTHETIC_VALUE=do-not-export");
    await writeFile(path.join(root, ".npmrc"), "//registry.npmjs.org/:_authToken=synthetic-token");

    await exportLocal(root, output);
    expect(await readFile(path.join(output, "package.json"), "utf8")).toBe(await readFile(path.join(root, "app/build/package.json"), "utf8"));
    expect(await readFile(path.join(output, "scripts/app-build.mjs"), "utf8")).toContain("buildApp");
    expect(await readFile(path.join(output, "dist/index.html"), "utf8")).toBe("published-v1");
    expect(await readFile(path.join(output, "server/health.mjs"), "utf8")).toContain("published-v1");
    expect((await listed(output)).some((file) => file.endsWith("build-executed.txt"))).toBe(false);
    expect(await listed(output)).not.toContain(".env");
    expect((await listed(output)).some((file) => file.startsWith(".konae/") || file.startsWith("data/") || file.startsWith("node_modules/") || file.startsWith(".git/"))).toBe(false);
    expect((await listed(output)).some((file) => [".env", ".npmrc"].includes(file))).toBe(false);

    const archive = createTar(await Promise.all((await listed(output)).map(async (file) => ({ path: file, contents: new Uint8Array(await readFile(path.join(output, file))) }))));
    const folder = readAppFolder(archive);
    await mkdir(path.join(imported, "app"), { recursive: true });
    await writeFile(path.join(imported, "konae.json"), JSON.stringify(folder.settings));
    for (const file of Object.keys(folder.sources.files)) {
      const target = path.join(imported, "app", file);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, sourceBytes(folder.sources, file));
    }
    for (const [file, bytes] of Object.entries(folder.dist)) {
      const target = path.join(imported, "dist", file);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, bytes);
    }
    for (const [file, bytes] of Object.entries(folder.server ?? {})) {
      const target = path.join(imported, "server", file);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, bytes);
    }
    expect(await readFile(path.join(imported, ".git"), "utf8").catch(() => "")).toBe("");
    expect(await readFile(path.join(imported, "node_modules/.package-lock.json"), "utf8").catch(() => "")).toBe("");
    await exportLocal(imported, secondExport);
    expect(await readFile(path.join(secondExport, "package-lock.json"), "utf8")).toBe(await readFile(path.join(root, "app/build/package-lock.json"), "utf8"));
    expect(await readFile(path.join(secondExport, "dist/index.html"), "utf8")).toBe("published-v1");
    expect(await readFile(path.join(secondExport, "server/health.mjs"), "utf8")).toContain("published-v1");
    expect((await listed(secondExport)).some((file) => file.endsWith("build-executed.txt"))).toBe(false);
  } finally {
    await rm(root, { recursive: true, force: true });
    await rm(output, { recursive: true, force: true });
    await rm(imported, { recursive: true, force: true });
    await rm(secondExport, { recursive: true, force: true });
  }
}, 30_000);

async function exportServer(refuse = false) {
  const database = new TextEncoder().encode("SQLite snapshot");
  const filesTar = createTar([{ path: "index.json", contents: "[]" }]);
  const folderArchive = createTar([
    { path: "konae.json", contents: JSON.stringify(settings) },
    { path: "app/app.js", contents: "console.log('published');" },
    { path: "app/actions.json", contents: JSON.stringify({ version: 1, actions: {} }) },
    { path: "dist/index.html", contents: "published" },
  ]);
  const digest = async (bytes: Uint8Array) => Buffer.from(await crypto.subtle.digest("SHA-256", bytes)).toString("hex");
  const manifest = {
    format: "konae-package", version: 1, packageId: "package-1", createdAt: 1, deploymentId: "offline_export", projectId: "offline_export",
    visibility: "private", signupPolicy: { mode: "closed", allowlist: [] }, publishedRevision: 3,
    contents: [{ name: "database", size: database.length, sha256: await digest(database) }, { name: "files", size: filesTar.length, sha256: await digest(filesTar) }], excluded: [],
  };
  const bodies: Record<string, Uint8Array> = { database, files: filesTar, folder: folderArchive };
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
        : name === "project_export" ? { manifest, downloads: { database: await transfer("database"), files: await transfer("files") }, folder: await transfer("folder"), sourcesAhead: false }
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
