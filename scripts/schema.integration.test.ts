import { afterEach, expect, test } from "bun:test";
import { Database } from "bun:sqlite";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fingerprintLocal } from "./konae";

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
