import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { encodeBase64, type Transfer } from "../src/mcp/client";
import type { PackageManifest } from "../src/lib/portable-package";
import { account, buildKernel, release, runtime, startKernel, type Kernel } from "./kernel-harness";

let root: string;
let kernel: Kernel;

const actions = { version: 1, actions: {
  "notes.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO notes(title, user_id) VALUES(?1, ?2)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 100 }, { source: "actor", field: "id" }] },
  "notes.all": { kind: "query", access: "authenticated", statement: "SELECT title FROM notes ORDER BY id LIMIT 50", bindings: [] },
} };
const schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL, extra TEXT);";
const slimmer = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY, title TEXT NOT NULL, user_id TEXT NOT NULL);";
const payload = (id: string, schemaSql = schema, page = "v1") => ({
  project: { id, name: "Imported", slug: "imported", manifest: { version: 1, visibility: "public" }, files: { "schema.sql": schemaSql, "actions.json": JSON.stringify(actions) } },
  visibility: "public",
  artifact: { version: 1, entrypoint: "index.html", files: { "index.html": `<!doctype html><main>${page}</main>` } },
});

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "konae-import-"));
  const port = Number(process.env.KONAE_IMPORT_TEST_PORT) || 19900 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "import-setup-secret", ownerEmail: "owner@import.test" });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

async function exportedPackage(deploymentId: string, projectId: string) {
  const exported = await kernel.owner.call<{ manifest: PackageManifest; downloads: Record<string, Transfer>; folder: Transfer }>("project_export", { projectId: projectId });
  const components: Record<string, string> = {};
  try {
    for (const content of exported.manifest.contents) {
      const bytes = await kernel.owner.download(exported.downloads[content.name]);
      components[content.name] = (await kernel.owner.upload(bytes, projectId, content.name)).transferId;
    }
  } finally {
    for (const transfer of Object.values(exported.downloads)) await kernel.owner.cancel(transfer.transferId).catch(() => {});
    await kernel.owner.cancel(exported.folder.transferId).catch(() => {});
  }
  return { manifest: exported.manifest, components };
}

test("an import replaces data, sources or both after a backup, keeps sessions, and restore brings the app back", async () => {
  const id = "import_app";
  await kernel.owner.call("project_create", { projectId: id, name: "Imported" });
  const { receipt } = await release(kernel.owner, payload(id), id);
  const deploymentId = receipt.id;
  const anaCookie = await account(kernel, receipt.hostname, "signup", "ana@import.test");
  const ana = runtime(kernel, receipt.hostname, anaCookie);
  await ana.call("actions_invoke", { name: "notes.add", input: { title: "first" } });
  const exported = await exportedPackage(deploymentId, id);

  await ana.call("actions_invoke", { name: "notes.add", input: { title: "second" } });
  await account(kernel, receipt.hostname, "signup", "bo@import.test");
  const titles = async () => (await ana.call<{ title: string }[]>("actions_invoke", { name: "notes.all", input: {} })).map((row) => row.title);
  const emails = async () => (await kernel.owner.call<{ users: { email: string }[] }>("app_users_list", { projectId: id })).users.map((user) => user.email);
  expect(await titles()).toEqual(["first", "second"]);

  await kernel.owner.call("maintenance_set", { projectId: id, on: true });
  const data = await kernel.owner.call<{ imported: boolean; backup: { id: string; label: string; holds: string[] } }>("project_import", { projectId: id, replace: "data", package: exported });
  for (const transferId of Object.values(exported.components)) await kernel.owner.cancel(transferId).catch(() => {});
  expect(data).toMatchObject({ imported: true, backup: { label: "Before import", holds: ["data"] } });
  await kernel.owner.call("maintenance_set", { projectId: id, on: false });
  expect(await titles()).toEqual(["first"]);
  expect(await emails()).toEqual(["ana@import.test"]);

  const backups = await kernel.owner.call<{ id: string; holds: string[] }[]>("database_execute", { projectId: id, operation: "list-backups" });
  expect(backups.find((backup) => backup.id === data.backup.id)?.holds).toEqual(["data"]);
  const restored = await kernel.owner.call<{ holds: string[]; before: { label: string } }>("database_execute", { projectId: id, operation: "restore-backup", input: { backupId: data.backup.id } });
  expect(restored).toMatchObject({ holds: ["data"], before: { label: "Before restore" } });
  expect(await titles()).toEqual(["first", "second"]);
  expect(await emails()).toEqual(["ana@import.test", "bo@import.test"]);

  const logo = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 0xff]);
  const plain = payload(id, slimmer, "v2");
  const dropping = { ...plain, project: { ...plain.project, files: { ...plain.project.files, "public/logo.png": encodeBase64(logo) }, encodings: { "public/logo.png": "base64" } } };
  const upload = await kernel.owner.upload(new TextEncoder().encode(JSON.stringify(dropping)), id);
  const asked = await kernel.owner.call<{ imported: boolean; schema: { confirmation: string; losses: unknown[] } }>("project_import", { projectId: id, replace: "sources", transferId: upload.transferId });
  expect(asked.imported).toBe(false);
  expect(asked.schema.losses).toHaveLength(1);
  expect(await titles()).toEqual(["first", "second"]);
  const sources = await kernel.owner.call<{ imported: boolean; backup: { holds: string[] } }>("project_import", { projectId: id, replace: "sources", transferId: upload.transferId, confirmSchema: asked.schema.confirmation });
  await kernel.owner.cancel(upload.transferId);
  expect(sources).toMatchObject({ imported: true, backup: { holds: ["sources"] } });
  const read = await kernel.owner.call<{ revision: number; transfer: Transfer }>("project_sources_read", { projectId: id });
  expect(read.revision).toBeGreaterThan(0);
  const saved = JSON.parse(new TextDecoder().decode(await kernel.owner.download(read.transfer))) as { files: Record<string, string>; encodings?: Record<string, string> };
  expect(saved.files["schema.sql"]).toBe(slimmer);
  expect(saved.encodings).toEqual({ "public/logo.png": "base64" });
  expect(new Uint8Array(await readFile(path.join(root, "data", "apps", id, "app", "public", "logo.png")))).toEqual(logo);

  const again = await exportedPackage(deploymentId, id);
  const both = payload(id, schema, "v3");
  const { artifact: baseArtifact, ...releasePayload } = both;
  const artifact = {
    ...baseArtifact,
    files: { ...baseArtifact.files, "public/logo.png": encodeBase64(logo) },
    encodings: { "public/logo.png": "base64" },
  };
  const { files, ...project } = releasePayload.project;
  const bothUpload = await kernel.owner.upload(new TextEncoder().encode(JSON.stringify({ ...releasePayload, project })), id);
  const sourceUpload = await kernel.owner.upload(new TextEncoder().encode(JSON.stringify({ files })), id);
  const artifactUpload = await kernel.owner.upload(new TextEncoder().encode(JSON.stringify(artifact)), id);
  const replaced = await kernel.owner.call<{ imported: boolean; backup: { holds: string[] } }>("project_import", { projectId: id, replace: "both", transferId: bothUpload.transferId, sourcesTransferId: sourceUpload.transferId, artifactTransferId: artifactUpload.transferId, package: again });
  for (const transferId of [bothUpload.transferId, sourceUpload.transferId, artifactUpload.transferId]) await kernel.owner.cancel(transferId);
  for (const transferId of Object.values(again.components)) await kernel.owner.cancel(transferId).catch(() => {});
  expect(replaced).toMatchObject({ imported: true, backup: { holds: ["data", "sources"] } });
  expect(new Uint8Array(await readFile(path.join(root, "data", "apps", id, "dist", "public", "logo.png")))).toEqual(logo);
  await ana.call("actions_invoke", { name: "notes.add", input: { title: "third" } });
  expect(await titles()).toEqual(["first", "second", "third"]);

  await expect(kernel.owner.call("project_import", { projectId: id, replace: "data" })).rejects.toThrow("Pass package exactly when replace is data or both.");
}, 180_000);

test("invalid release settings refuse before a both import changes live rows or files", async () => {
  const id = "import_invalid_contact";
  await kernel.owner.call("project_create", { projectId: id, name: "Invalid contact import" });
  const { receipt } = await release(kernel.owner, payload(id), id);
  const deploymentId = receipt.id;
  const ana = runtime(kernel, receipt.hostname, await account(kernel, receipt.hostname, "signup", "invalid-contact@import.test"));
  await ana.call("actions_invoke", { name: "notes.add", input: { title: "exported row" } });
  const exported = await exportedPackage(deploymentId, id);
  await ana.call("actions_invoke", { name: "notes.add", input: { title: "live row" } });

  const appPath = path.join(root, "data", "apps", id, "app", "actions.json");
  const distPath = path.join(root, "data", "apps", id, "dist", "index.html");
  const beforeApp = await readFile(appPath);
  const beforeDist = await readFile(distPath);
  const beforeProject = await kernel.owner.call<{ project: { name: string; slug: string; manifest: unknown }; sourcesRevision: number; deployment: { artifactVersion: string } }>("project_inspect", { projectId: id });
  const beforeContact = await kernel.owner.call("contact_email_read", { projectId: id });
  const invalid = { ...payload(id, schema, "must not publish"), contactEmail: "invalid" };
  const upload = await kernel.owner.upload(new TextEncoder().encode(JSON.stringify(invalid)), id);
  try {
    await expect(kernel.owner.call("project_import", {
      projectId: deploymentId,
      replace: "both",
      transferId: upload.transferId,
      package: exported,
    })).rejects.toThrow("The contact email is not a valid email address.");
  } finally {
    await kernel.owner.cancel(upload.transferId).catch(() => {});
    for (const transferId of Object.values(exported.components)) await kernel.owner.cancel(transferId).catch(() => {});
  }

  expect((await ana.call<{ title: string }[]>("actions_invoke", { name: "notes.all" })).map((row) => row.title)).toEqual(["exported row", "live row"]);
  expect(await readFile(appPath)).toEqual(beforeApp);
  expect(await readFile(distPath)).toEqual(beforeDist);
  expect(await kernel.owner.call("project_inspect", { projectId: id })).toMatchObject(beforeProject);
  expect(await kernel.owner.call("contact_email_read", { projectId: id })).toEqual(beforeContact);
}, 120_000);
