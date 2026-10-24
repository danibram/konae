import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import type { Transfer } from "../src/mcp/client";
import type { PackageManifest } from "../src/lib/portable-package";
import { downloadToFile } from "../scripts/package-files";
import { buildKernel, downloadMigration, importMigration, release, startKernel, type Begun, type Kernel } from "./migration-kernel";

// schema.sql may add CHECK constraints a Database Model cannot state. A
// migration that recreates a table carries them over from the live table, a
// package and a migration to another machine keep them, and a change that
// removes a column one of them names is refused (kernel-rs/src/check-carry-cases.json).
let root: string;
let kernel: Kernel;

const model = 'import { sqliteTable, integer, text } from "drizzle-orm/sqlite-core"; export const habits = sqliteTable("habits", { id: integer("id").primaryKey({ autoIncrement: true }), name: text("name").notNull(), state: text("state").notNull().default("on"), note: text("note") });';
const schema = "CREATE TABLE IF NOT EXISTS habits(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 60), state TEXT NOT NULL DEFAULT 'on' CHECK (state IN ('on', 'off')), note TEXT, CONSTRAINT short_note CHECK (note IS NULL OR length(note) < 20));";
const actions = { version: 1, actions: {
  "habits.add": { kind: "mutation", access: "authenticated", statement: "INSERT INTO habits(name, state) VALUES(?1, ?2)", bindings: [{ source: "input", key: "name", type: "string", maxLength: 200 }, { source: "input", key: "state", type: "string", maxLength: 20 }] },
  "habits.list": { kind: "query", access: "authenticated", statement: "SELECT name, state FROM habits ORDER BY id LIMIT 50", bindings: [] },
} };

function payload(id: string, source = model) {
  return {
    project: { id, name: id, slug: id.replaceAll("_", "-"), manifest: { version: 1, visibility: "public" }, files: { "schema.ts": source, "schema.sql": schema, "actions.json": JSON.stringify(actions) }, databaseModel: { source, acceptedSource: source, revision: "baseline" } },
    visibility: "public",
    artifact: { version: 1, entrypoint: "index.html", files: { "index.html": "<!doctype html><main>habits</main>" } },
  };
}

async function publish(id: string) {
  await kernel.owner.call("project_create", { projectId: id, name: id, sourceAuthority: "managed" });
  return (await release(kernel.owner, payload(id), id)).receipt;
}

const sql = (deploymentId: string, statement: string) => kernel.owner.call<{ rows: Record<string, unknown>[] }>("database_execute", { deploymentId, operation: "sql", input: { sql: statement } });
const tableSql = async (deploymentId: string) => String((await sql(deploymentId, "SELECT sql FROM sqlite_master WHERE name = 'habits' LIMIT 1")).rows[0].sql);
const backups = async (deploymentId: string) => (await kernel.owner.call<{ label: string }[]>("database_execute", { deploymentId, operation: "list-backups", input: {} })).filter((backup) => backup.label === "Before migration");

async function migrate(deploymentId: string, candidate: string) {
  const plan = await kernel.owner.call<Record<string, unknown>>("schema_plan", { acceptedSource: model, candidateSource: candidate });
  return kernel.owner.call("database_execute", { deploymentId, operation: "apply-migration", input: { ...plan, recoveryModelSource: model } });
}

/** The CHECKs as SQLite enforces them: each of these rows breaks one. */
function expectEnforced(db: { run(statement: string): unknown }) {
  expect(() => db.run("INSERT INTO habits(name, state) VALUES('walk', 'maybe')")).toThrow("CHECK constraint failed");
  expect(() => db.run("INSERT INTO habits(name) VALUES('   ')")).toThrow("CHECK constraint failed");
  expect(() => db.run("INSERT INTO habits(name, note) VALUES('walk', 'a note that is far too long')")).toThrow("CHECK constraint failed");
}

beforeAll(async () => {
  await buildKernel();
  root = await mkdtemp(path.join(tmpdir(), "littlebox-check-carry-"));
  const port = Number(process.env.LITTLEBOX_MCP_TEST_PORT) || 19500 + Math.floor(Math.random() * 90);
  kernel = await startKernel({ port, data: path.join(root, "data"), setupKey: "check-carry-setup-secret", ownerEmail: "owner@checks.test" });
}, 300_000);

afterAll(async () => { await kernel?.stop(); if (root) await rm(root, { recursive: true, force: true }); });

describe("CHECK constraints from schema.sql", () => {
  test("survive a migration that recreates their table", async () => {
    const app = await publish("checks_recreate");
    await sql(app.id, "INSERT INTO habits(name, state) VALUES('read', 'off')");
    // A new default makes Drizzle rebuild the table as __new_habits.
    const candidate = model.replace('default("on")', 'default("off")');
    await migrate(app.id, candidate);
    expect((await kernel.owner.call<{ deployment: { databaseModel?: { acceptedSource: string } } }>("project_inspect", { projectId: "checks_recreate" })).deployment.databaseModel?.acceptedSource).toBe(candidate);
    const rebuilt = await tableSql(app.id);
    expect(rebuilt).toContain("DEFAULT 'off'");
    expect(rebuilt).toContain("CHECK (state IN ('on', 'off'))");
    expect(rebuilt).toContain("CONSTRAINT short_note CHECK (note IS NULL OR length(note) < 20)");
    expect((await sql(app.id, "SELECT name, state FROM habits LIMIT 5")).rows).toEqual([{ name: "read", state: "off" }]);
    await expect(sql(app.id, "INSERT INTO habits(name, state) VALUES('walk', 'maybe')")).rejects.toThrow("CHECK constraint failed");
    await sql(app.id, "INSERT INTO habits(name, state) VALUES('walk', 'on')");
    expect((await sql(app.id, "SELECT COUNT(*) n FROM habits LIMIT 1")).rows).toEqual([{ n: 2 }]);
    const copy = new Database(":memory:");
    try {
      copy.run(rebuilt);
      expectEnforced(copy);
    } finally { copy.close(); }
  }, 60_000);

  test("refuse a migration that removes a column a CHECK of another column names", async () => {
    const app = await publish("checks_refused");
    await sql(app.id, "INSERT INTO habits(name) VALUES('read')");
    const before = await tableSql(app.id);
    const candidate = model.replace(', note: text("note")', "");
    await expect(migrate(app.id, candidate)).rejects.toThrow("Table habits has a CHECK that names column note, which this migration removes. Rebuild habits without that CHECK in the SQL console, or keep note, then apply the migration again.");
    expect(await tableSql(app.id)).toBe(before);
    expect(await backups(app.id)).toEqual([]);
    expect((await kernel.owner.call<{ deployment: { databaseModel?: { acceptedSource: string } } }>("project_inspect", { projectId: "checks_refused" })).deployment.databaseModel?.acceptedSource).toBe(model);
  }, 60_000);

  test("travel in a package to another app", async () => {
    const app = await publish("checks_package");
    await sql(app.id, "INSERT INTO habits(name) VALUES('read')");
    const exported = await kernel.owner.call<{ manifest: PackageManifest; downloads: Record<string, Transfer> }>("package_export", { deploymentId: app.id });
    const directory = await mkdtemp(path.join(root, "package-"));
    const files: Record<string, string> = {};
    for (const content of exported.manifest.contents) {
      files[content.name] = path.join(directory, content.name);
      await downloadToFile(kernel.owner, exported.downloads[content.name], files[content.name]);
    }
    const imported = await importMigration(kernel.owner, { manifest: exported.manifest, files: { ...files, release: await writeRelease(directory, "checks_package_copy") } }, "checks_package_copy");
    const destination = imported.receipt;
    expect(await tableSql(destination.id)).toBe(await tableSql(app.id));
    expect((await sql(destination.id, "SELECT name FROM habits LIMIT 5")).rows).toEqual([{ name: "read" }]);
    for (const row of ["('walk', 'maybe')", "('   ', 'on')"]) {
      await expect(sql(destination.id, `INSERT INTO habits(name, state) VALUES${row}`)).rejects.toThrow("CHECK constraint failed");
    }
  }, 60_000);

  test("travel in a migration to another machine", async () => {
    const app = await publish("checks_move");
    await sql(app.id, "INSERT INTO habits(name) VALUES('read')");
    const begun = await kernel.owner.call<Begun>("migration_begin", { deploymentId: app.id, waitSeconds: 10 });
    expect(begun.phase).toBe("captured");
    const captured = await downloadMigration(kernel.owner, begun, await mkdtemp(path.join(root, "move-")));
    const carried = new Database(captured.files.database, { readonly: true });
    try {
      expect(carried.query("SELECT sql FROM sqlite_master WHERE name = 'habits'").get()).toEqual({ sql: expect.stringContaining("CHECK (state IN ('on', 'off'))") });
    } finally { carried.close(); }
    const copy = (await importMigration(kernel.owner, captured, "checks_move_copy")).receipt;
    // The copy starts paused, so its database is read from disk.
    const moved = new Database(path.join(kernel.data, "deployments", copy.id, "database.sqlite"), { readonly: true });
    let movedSql: string;
    try {
      movedSql = (moved.query("SELECT sql FROM sqlite_master WHERE name = 'habits'").get() as { sql: string }).sql;
    } finally { moved.close(); }
    // The live file is in WAL mode, and bun's SQLite opens such a file read-only
    // only while its -shm exists, which the Kernel removes with its last connection.
    const origin = new Database(path.join(kernel.data, "deployments", app.id, "database.sqlite"));
    try {
      expect(movedSql).toBe((origin.query("SELECT sql FROM sqlite_master WHERE name = 'habits'").get() as { sql: string }).sql);
    } finally { origin.close(); }
    const scratch = new Database(":memory:");
    try { scratch.run(movedSql); expectEnforced(scratch); } finally { scratch.close(); }
    await kernel.owner.call("migration_cancel", { operationId: begun.operationId });
  }, 60_000);
});

async function writeRelease(directory: string, projectId: string) {
  const file = path.join(directory, "release.json");
  await Bun.write(file, JSON.stringify(payload(projectId)));
  return file;
}

