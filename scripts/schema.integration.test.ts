import { afterEach, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { buildLocal, fingerprintLocal, initLocal, localKernel, readSettings } from "./konae";
import { bootLocalKernel } from "./rehearse";

const roots: string[] = [];
afterEach(async () => { for (const root of roots.splice(0)) await rm(root, { recursive: true, force: true }); });

test.skipIf(!process.env.KONAE_SCHEMA_KERNEL_BIN)("schema fingerprints read source or data without creating an app runtime", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-schema-cli-"));
  roots.push(root);
  await mkdir(path.join(root, "app"));
  await mkdir(path.join(root, "data"));
  const schema = "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY, title TEXT);";
  await writeFile(path.join(root, "app/schema.sql"), schema);
  const database = path.join(root, "data/database.sqlite");
  const db = new Database(database);
  db.exec(schema);
  db.exec("INSERT INTO tasks(title) VALUES ('kept')");
  db.close();
  const before = await readFile(database);
  const previous = process.env.KONAE_KERNEL_BIN;
  process.env.KONAE_KERNEL_BIN = process.env.KONAE_SCHEMA_KERNEL_BIN;
  try {
    const source = await fingerprintLocal(root);
    expect(source).toMatch(/^sha256:[a-f0-9]{64}$/);
    expect(await fingerprintLocal(root, true)).toBe(source);
    await writeFile(path.join(root, "app/schema.sql"), "CREATE TABLE IF NOT EXISTS tasks(id INTEGER PRIMARY KEY, title TEXT, done INTEGER);");
    expect(await fingerprintLocal(root)).not.toBe(source);
    expect(await fingerprintLocal(root, true)).toBe(source);
    expect(await readFile(database)).toEqual(before);
    await expect(stat(path.join(root, ".konae"))).rejects.toThrow();
    await rm(database);
    await expect(fingerprintLocal(root, true)).rejects.toThrow();
    await expect(stat(database)).rejects.toThrow();
  } finally {
    if (previous === undefined) delete process.env.KONAE_KERNEL_BIN;
    else process.env.KONAE_KERNEL_BIN = previous;
  }
});


test.skipIf(!process.env.KONAE_SCHEMA_KERNEL_BIN)("test-mode restart applies a schema-only edit with unchanged build files", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-schema-restart-"));
  roots.push(root);
  await initLocal(root, { name: "Schema restart", template: "tasks" });
  const settings = await readSettings(root);
  const previous = process.env.KONAE_KERNEL_BIN;
  process.env.KONAE_KERNEL_BIN = process.env.KONAE_SCHEMA_KERNEL_BIN;
  try {
    await buildLocal(root, { development: true });
    const beforeBuild = await readFile(path.join(root, "dist/index.html"));
    const kernel = await localKernel(root, settings, { build: false });
    await bootLocalKernel(kernel);
    await kernel.run(["sql", "INSERT INTO tasks(title, created_at) VALUES ('keep this row', 1)"]);
    const schemaPath = path.join(root, "app/schema.sql");
    const schema = await readFile(schemaPath, "utf8");
    await writeFile(schemaPath, schema.replace("created_at INTEGER NOT NULL", "created_at INTEGER NOT NULL, note TEXT"));
    await buildLocal(root, { development: true });
    expect(await readFile(path.join(root, "dist/index.html"))).toEqual(beforeBuild);
    await bootLocalKernel(kernel);
    const result = JSON.parse(await kernel.run(["sql", "SELECT title, note FROM tasks LIMIT 10"]));
    expect(result.rows).toEqual([{ title: "keep this row", note: null }]);
  } finally {
    if (previous === undefined) delete process.env.KONAE_KERNEL_BIN;
    else process.env.KONAE_KERNEL_BIN = previous;
  }
});

test.skipIf(!process.env.KONAE_SCHEMA_KERNEL_BIN)("local startup confirms a data transformation and applies it once across restarts", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-transform-restart-"));
  roots.push(root);
  await initLocal(root, { name: "Transform restart", template: "tasks" });
  const settings = await readSettings(root);
  const previous = process.env.KONAE_KERNEL_BIN;
  process.env.KONAE_KERNEL_BIN = process.env.KONAE_SCHEMA_KERNEL_BIN;
  try {
    await buildLocal(root, { development: true });
    const kernel = await localKernel(root, settings, { build: false });
    await bootLocalKernel(kernel);
    await kernel.run(["sql", "INSERT INTO tasks(title, created_at) VALUES ('original', 1)"]);
    const before = await fingerprintLocal(root, true);
    const schemaPath = path.join(root, "app/schema.sql");
    const schema = await readFile(schemaPath, "utf8");
    await writeFile(schemaPath, schema.replace("created_at INTEGER NOT NULL", "created_at INTEGER NOT NULL, note TEXT"));
    const after = await fingerprintLocal(root);
    await writeFile(path.join(root, "app/schema-transforms.json"), JSON.stringify({ version: 1, transitions: [{
      id: "add-note", predecessor: null, before, after, maxRows: 10,
      statements: ["ALTER TABLE tasks ADD COLUMN note TEXT", "UPDATE tasks SET title=title || '!', note='migrated'"],
    }] }));
    await buildLocal(root, { development: true });
    let refusal = "";
    try { await bootLocalKernel(kernel); }
    catch (error) { refusal = error instanceof Error ? error.message : String(error); }
    const confirmation = /KONAE_CONFIRM_SCHEMA=([a-f0-9]{64})/.exec(refusal)?.[1];
    expect(confirmation).toBeDefined();
    expect(await fingerprintLocal(root, true)).toBe(before);
    expect(JSON.parse(await kernel.run(["sql", "SELECT title FROM tasks LIMIT 10"])).rows).toEqual([{ title: "original" }]);
    const confirmed = await localKernel(root, settings, { build: false, confirmSchema: confirmation });
    await bootLocalKernel(confirmed);
    await bootLocalKernel(confirmed);
    expect(await fingerprintLocal(root, true)).toBe(after);
    expect(JSON.parse(await confirmed.run(["sql", "SELECT title, note FROM tasks LIMIT 10"])).rows).toEqual([{ title: "original!", note: "migrated" }]);
    await expect(stat(path.join(root, "dist/schema-transforms.json"))).rejects.toThrow();
  } finally {
    if (previous === undefined) delete process.env.KONAE_KERNEL_BIN;
    else process.env.KONAE_KERNEL_BIN = previous;
  }
});
