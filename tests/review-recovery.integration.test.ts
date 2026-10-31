import { afterAll, beforeAll, expect, spyOn, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildProjectExport } from "../src/lib/export-project-kit";
import { createDefaultManifest } from "../src/deployments/manifest";
import { HostedDatabaseClient, type PortableProjectState } from "../src/deployments/client";
import { McpClient, platformMcp } from "../src/mcp/client";

let root: string;
let kernel: ReturnType<typeof Bun.spawn>;
let client: McpClient;
let url: string;
let cookie: string;
const model = 'import { sqliteTable, integer, text } from "drizzle-orm/sqlite-core"; export const notes = sqliteTable("notes", { id: integer("id").primaryKey({ autoIncrement: true }), title: text("title").notNull() });';
const databaseModel = { source: model, acceptedSource: model, revision: "baseline" };
const schema = "CREATE TABLE IF NOT EXISTS notes(id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL);";
const manifest = { ...createDefaultManifest(), visibility: "private" as const };
const files = { "schema.ts": model, "schema.sql": schema, "actions.json": '{"version":1,"actions":{}}' };

async function start() {
  kernel = Bun.spawn(["kernel-rs/target/debug/konae-kernel"], {
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KONAE_"))), KONAE_DATA_DIR: root, KONAE_KERNEL_PORT: new URL(url).port, KONAE_BIND: "127.0.0.1", KONAE_ADMIN_KEY: "review-test-setup", KONAE_PUBLIC_ORIGIN: url, KONAE_HOST_SUFFIX: "localhost", KONAE_BUN_PATH: process.execPath },
    stdout: "ignore", stderr: "inherit",
  });
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(url + "/_konae/health")).ok) return; } catch {}
    await Bun.sleep(100);
  }
  throw Error("Test Kernel did not start.");
}

beforeAll(async () => {
  const build = Bun.spawn(["cargo", "build", "--offline", "--manifest-path", "kernel-rs/Cargo.toml"], { stdout: "ignore", stderr: "inherit" });
  expect(await build.exited).toBe(0);
  root = await mkdtemp(path.join(tmpdir(), "konae-review-"));
  url = `http://127.0.0.1:${21000 + Math.floor(Math.random() * 1000)}`;
  await start();
  const signup = await fetch(url + "/_konae/owner/signup", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ email: "owner@selfhost.local", password: "review-test-setup", setupKey: "review-test-setup" }) });
  expect(signup.ok).toBe(true);
  cookie = signup.headers.get("set-cookie")!.split(";")[0];
  client = new McpClient(url + "/mcp", { fetch: (input, init) => fetch(input, { ...init, headers: { ...Object.fromEntries(new Headers(init?.headers)), cookie } }) });
}, 120_000);

afterAll(async () => { kernel?.kill(); if (kernel) await kernel.exited; if (root) await rm(root, { recursive: true, force: true }); });

async function publish(id: string, withModel = true, initialDomain?: string, projectFiles = files, releaseModel = databaseModel) {
  await client.call("project_create", { projectId: id, name: id, manifest });
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify({ project: { id, name: id, slug: id, files: projectFiles, manifest, ...(withModel ? { databaseModel: releaseModel } : {}) }, visibility: "private", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<main>test</main>" } }, ...(initialDomain ? { initialDomain } : {}) })), id);
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    return await client.call<{ id: string; databaseModel?: typeof databaseModel }>("release_apply", { planId: plan.planId });
  } finally { await client.cancel(transfer.transferId); }
}

async function save(id: string, acceptedSource = model, currentSource = acceptedSource) {
  const source = { id, name: id, slug: id, files, manifest, directories: [], updatedAt: 1, databaseModel: { source: currentSource, acceptedSource, revision: "editable" } };
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify(source)), id);
  try { await client.call("project_sources_save", { projectId: id, transferId: transfer.transferId, expectedRevision: 0 }); }
  finally { await client.cancel(transfer.transferId); }
}

async function inspect(id: string) {
  return (await client.call<{ deployment: { id: string; databaseModel?: typeof databaseModel } }>("project_inspect", { projectId: id })).deployment;
}

test("publish, restart, inspect and republish keep the deployed model despite edited sources", async () => {
  const first = await publish("published_model");
  expect(first.databaseModel).toEqual(databaseModel);
  await save("published_model", model.replace('title: text("title").notNull()', 'title: text("title").notNull(), extra: text("extra")'));
  kernel.kill(); await kernel.exited;
  const deploymentPath = path.join(root, "deployments", first.id, "deployment.json");
  const stored = JSON.parse(await readFile(deploymentPath, "utf8"));
  stored.url = `http://${stored.hostname}.localhost:1`;
  await writeFile(deploymentPath, JSON.stringify(stored));
  await start();
  const restarted = await client.call<{ deployment: { url: string } }>("project_inspect", { projectId: "published_model" });
  expect(new URL(restarted.deployment.url).port).toBe(new URL(url).port);
  expect((await inspect("published_model")).databaseModel).toEqual(databaseModel);
  expect((await publish("published_model")).id).toBe(first.id);
  expect((await inspect("published_model")).databaseModel).toEqual(databaseModel);
});

test("legacy recovery proves the model against the live schema and refuses changed schemas", async () => {
  await publish("legacy_model", false); await save("legacy_model", model, model.replace('title: text("title").notNull()', 'title: text("title").notNull(), extra: text("extra")'));
  expect((await inspect("legacy_model")).databaseModel?.acceptedSource).toBe(model);
  await publish("legacy_changed", false);
  await save("legacy_changed", model.replace('title: text("title").notNull()', 'title: text("title").notNull(), extra: text("extra")'));
  expect((await inspect("legacy_changed")).databaseModel).toBeUndefined();
});

test("portable export carries only the domain name and an import remains pending", async () => {
  const origin = await publish("domain_origin", true, "notes.example.org");
  const domain = await client.call<{ record: { value: string } }>("domain_read", { deploymentId: origin.id });
  const transfer = await client.call<Parameters<McpClient["download"]>[0]>("project_export", { deploymentId: origin.id });
  const exported = new TextDecoder().decode(await client.download(transfer));
  const state = JSON.parse(exported).result;
  expect(state.domain).toBe("notes.example.org");
  expect(exported).not.toContain(domain.record.value);
  expect(state.visibility).toBe("private");
  await client.call("domain_clear", { deploymentId: origin.id });
  const imported = await publish("domain_destination", true, state.domain);
  const pending = await client.call<{ status: string; record: { value: string } }>("domain_read", { deploymentId: imported.id });
  expect(pending.status).toBe("pending");
  expect(pending.record.value).not.toBe(domain.record.value);
  const persisted = JSON.parse(await readFile(path.join(root, "deployments", imported.id, "domain.json"), "utf8"));
  expect(persisted.verified).toBe(false);
});

test("the generated editor kit imports private data and prints the destination TXT record on repeated deploys", async () => {
  const origin = await publish("kit_origin", true, "kit.example.org");
  await client.call("database_execute", { deploymentId: origin.id, operation: "sql", input: { sql: "INSERT INTO notes(title) VALUES('portable row')" } });
  const transport = spyOn(platformMcp, "call").mockImplementation((name, args) => client.call(name, args));
  let state: PortableProjectState;
  try { state = await new HostedDatabaseClient(origin.id).exportPortableState(); }
  finally { transport.mockRestore(); }
  expect(state.domain).toBe("kit.example.org");
  const sourceDomain = await client.call<{ record: { value: string } }>("domain_read", { deploymentId: origin.id });
  await client.call("domain_clear", { deploymentId: origin.id });
  const directory = await mkdtemp(path.join(tmpdir(), "konae-kit-domain-"));
  try {
    const kit = buildProjectExport({ id: "kit_destination", name: "Kit", slug: "kit", manifest: { ...manifest, visibility: "public" }, files, databaseModel, directories: [], updatedAt: 1 },
      { version: 1, entrypoint: "index.html", files: { "index.html": "<script data-konae-runtime>window.miniapp={}</script>" } }, [],
      state);
    for (const entry of kit.entries) {
      const target = path.join(directory, entry.path);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, entry.contents);
    }
    await writeFile(path.join(directory, ".env"), `KONAE_PORT=${new URL(url).port}\nKONAE_ADMIN_KEY=review-test-setup\n`);
    for (let run = 0; run < 2; run++) {
      const child = Bun.spawn(["node", "scripts/deploy.mjs"], { cwd: directory, stdout: "pipe", stderr: "pipe" });
      const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      expect(stderr).toBe("");
      expect(code).toBe(0);
      const destination = await inspect("kit_destination");
      const domain = await client.call<{ status: string; record: { name: string; value: string } }>("domain_read", { deploymentId: destination.id });
      expect(domain.status).toBe("pending");
      expect(domain.record.value).not.toBe(sourceDomain.record.value);
      expect(stdout).toContain(domain.record.value);
      expect(stdout).toContain("set KONAE_APP_DOMAIN=kit.example.org in .env");
      const policy = await client.call<{ visibility: string }>("signup_policy_read", { deploymentId: destination.id });
      expect(policy.visibility).toBe("private");
      const data = await client.call<{ rows: { title: string }[] }>("database_execute", { deploymentId: destination.id, operation: "sql", input: { sql: "SELECT title FROM notes LIMIT 1" } });
      expect(data.rows[0].title).toBe("portable row");
    }
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("legacy recovery accepts the seeded shared app's boolean default and handwritten constraints", async () => {
  const tasksModel = 'import { sqliteTable, integer, text } from "drizzle-orm/sqlite-core"; export const tasks = sqliteTable("tasks", { id: integer("id").primaryKey({ autoIncrement: true }), title: text("title").notNull(), done: integer("done", { mode: "boolean" }).notNull().default(false), createdAt: integer("created_at").notNull() });';
  await publish("legacy_shared", false, undefined, { ...files, "schema.ts": tasksModel, "schema.sql": "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);" });
  await save("legacy_shared", tasksModel);
  const recovered = await inspect("legacy_shared");
  expect(recovered.databaseModel?.acceptedSource).toBe(tasksModel);
  expect(recovered.databaseModel?.revision).toMatch(/^[a-f0-9]{64}$/);
});

test("applying a schema plan persists its model across restart and restoring its backup restores the model", async () => {
  const first = await publish("migration_model");
  const candidate = model.replace('title: text("title").notNull()', 'title: text("title").notNull(), extra: text("extra")');
  const plan = await client.call<{ id: string; fromHash: string; toHash: string; candidateModelSource: string }>("schema_plan", { acceptedSource: model, candidateSource: candidate });
  expect(plan.candidateModelSource).toBe(candidate);
  const applied = await client.call<{ backup: { id: string; databaseModelSource: string; schemaRevision: string } }>("database_execute", { deploymentId: first.id, operation: "apply-migration", input: { ...plan, recoveryModelSource: model } });
  expect(applied.backup.databaseModelSource).toBe(model);
  expect(applied.backup.schemaRevision).toBe("baseline");
  kernel.kill(); await kernel.exited; await start();
  expect((await inspect("migration_model")).databaseModel).toEqual({ source: candidate, acceptedSource: candidate, revision: plan.toHash });
  await publish("migration_model", true, undefined, { ...files, "schema.ts": candidate }, { source: candidate, acceptedSource: candidate, revision: plan.toHash });
  await expect(client.call("database_execute", { deploymentId: first.id, operation: "apply-migration", input: { ...plan, recoveryModelSource: model } })).rejects.toThrow("deployed database model changed");
  const restored = await client.call<{ databaseModelSource: string; schemaRevision: string }>("database_execute", { deploymentId: first.id, operation: "restore-backup", input: { backupId: applied.backup.id } });
  expect(restored.databaseModelSource).toBe(model);
  expect(restored.schemaRevision).toBe("baseline");
  kernel.kill(); await kernel.exited; await start();
  expect((await inspect("migration_model")).databaseModel).toEqual(databaseModel);
  const tables = await client.call<{ name: string; columns: { name: string }[] }[]>("database_execute", { deploymentId: first.id, operation: "inspect", input: {} });
  expect(tables.find(t => t.name === "notes")?.columns.map(c => c.name)).not.toContain("extra");
  expect((await publish("migration_model")).id).toBe(first.id);
});

test("release planning rejects an unrelated accepted model and preserves the prior deployment", async () => {
  const first = await publish("model_mismatch");
  const unrelated = model.replace('title: text("title").notNull()', 'title: text("title").notNull(), extra: text("extra")');
  await expect(publish("model_mismatch", true, undefined, files, { source: unrelated, acceptedSource: unrelated, revision: "claimed" })).rejects.toThrow("does not match the deployed schema");
  expect((await inspect("model_mismatch")).databaseModel).toEqual(databaseModel);
  expect((await inspect("model_mismatch")).id).toBe(first.id);
});

test("a CLI release without a model preserves a matching model and clears it when the schema changes", async () => {
  await publish("cli_model");
  expect((await publish("cli_model", false)).databaseModel).toEqual(databaseModel);
  const changedFiles = { ...files, "schema.sql": schema + " CREATE TABLE IF NOT EXISTS sqlitex(id INTEGER PRIMARY KEY);" };
  expect((await publish("cli_model", false, undefined, changedFiles)).databaseModel).toBeUndefined();
  expect((await inspect("cli_model")).databaseModel).toBeUndefined();
});

test("release apply rechecks the live schema before replacing the artifact", async () => {
  const first = await publish("apply_schema_drift");
  const before = await client.call<{ deployment: { artifactVersion: string } }>("project_inspect", { projectId: "apply_schema_drift" });
  const transfer = await client.upload(new TextEncoder().encode(JSON.stringify({ project: { id: "apply_schema_drift", name: "Apply", slug: "apply", files, manifest, databaseModel }, visibility: "private", artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<main>replacement</main>" } } })), "apply_schema_drift");
  try {
    const plan = await client.call<{ planId: string }>("release_plan", { transferId: transfer.transferId });
    await client.call("database_execute", { deploymentId: first.id, operation: "sql", input: { sql: "CREATE TABLE intervening(id INTEGER PRIMARY KEY)" } });
    await expect(client.call("release_apply", { planId: plan.planId })).rejects.toThrow("does not match the deployed schema");
    const after = await client.call<{ deployment: { artifactVersion: string; databaseModel: typeof databaseModel } }>("project_inspect", { projectId: "apply_schema_drift" });
    expect(after.deployment.artifactVersion).toBe(before.deployment.artifactVersion);
    expect(after.deployment.databaseModel).toEqual(databaseModel);
  } finally { await client.cancel(transfer.transferId); }
});

test("migration schema drift rolls back before recording a candidate model", async () => {
  const first = await publish("migration_schema_drift");
  const candidate = model.replace('title: text("title").notNull()', 'title: text("title").notNull(), extra: text("extra")');
  const plan = await client.call<Record<string, unknown>>("schema_plan", { acceptedSource: model, candidateSource: candidate });
  await client.call("database_execute", { deploymentId: first.id, operation: "sql", input: { sql: "CREATE TABLE intervening(id INTEGER PRIMARY KEY)" } });
  await expect(client.call("database_execute", { deploymentId: first.id, operation: "apply-migration", input: { ...plan, recoveryModelSource: model } })).rejects.toThrow("migration was rolled back");
  expect((await inspect("migration_schema_drift")).databaseModel).toEqual(databaseModel);
  const tables = await client.call<{ name: string; columns: { name: string }[] }[]>("database_execute", { deploymentId: first.id, operation: "inspect", input: {} });
  expect(tables.find(t => t.name === "notes")?.columns.map(c => c.name)).not.toContain("extra");
});

test("a deployed migration accepts a required defaulted column declared between existing columns", async () => {
  const candidate = model.replace('id: integer("id").primaryKey({ autoIncrement: true }),', 'id: integer("id").primaryKey({ autoIncrement: true }), priority: text("priority").notNull().default("normal"),');
  const first = await publish("inserted_column_model");
  await client.call("database_execute", { deploymentId: first.id, operation: "sql", input: { sql: "INSERT INTO notes(title) VALUES('keep this row')" } });
  const plan = await client.call<Record<string, unknown>>("schema_plan", { acceptedSource: model, candidateSource: candidate });
  await client.call("database_execute", { deploymentId: first.id, operation: "apply-migration", input: { ...plan, recoveryModelSource: model } });
  expect((await inspect("inserted_column_model")).databaseModel?.acceptedSource).toBe(candidate);
  const data = await client.call<{ rows: { title: string; priority: string }[] }>("database_execute", { deploymentId: first.id, operation: "sql", input: { sql: "SELECT title,priority FROM notes LIMIT 1" } });
  expect(data.rows).toEqual([{ title: "keep this row", priority: "normal" }]);
  expect((await publish("inserted_column_model", true, undefined, files, { source: candidate, acceptedSource: candidate, revision: String(plan.toHash) })).id).toBe(first.id);
});
