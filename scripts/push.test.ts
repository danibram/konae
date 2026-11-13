import { Database } from "bun:sqlite";
import { afterEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { readTar } from "../src/lib/tar";
import { checkFiles, stageData } from "./push";

const limits = { quotaBytes: 20 * 1024 * 1024, maxFileBytes: 2 * 1024 * 1024 };
const id = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
const folders: string[] = [];
async function folder() { const made = await mkdtemp(path.join(tmpdir(), "konae-push-test-")); folders.push(made); return made; }
afterEach(async () => { for (const made of folders.splice(0)) await rm(made, { recursive: true, force: true }); });

async function appFolder(files: { id: string; name: string; size: number; ownerId?: string }[]) {
  const root = await folder();
  await mkdir(path.join(root, "data/files"), { recursive: true });
  const db = new Database(path.join(root, "data/database.sqlite"));
  db.run("PRAGMA journal_mode=WAL");
  db.run("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT)");
  db.run("INSERT INTO notes(body) VALUES ('a'), ('b')");
  await writeFile(path.join(root, "data/files/index.json"), JSON.stringify(files.map((file) => ({ ...file, type: "text/plain", visibility: "public" }))));
  for (const file of files) await writeFile(path.join(root, `data/files/${file.id}.blob`), new Uint8Array(file.size));
  // What macOS leaves beside a copied file, and what the Kernel refuses inside the archive.
  await writeFile(path.join(root, "data/files/._index.json"), "apple double");
  await writeFile(path.join(root, "data/secrets.sqlite"), "never sent");
  return { root, db };
}

describe("konae push", () => {
  test("stages index.json first and each blob in index order, with rows still in the WAL", async () => {
    const { root, db } = await appFolder([{ id: id(2), name: "b.txt", size: 3 }, { id: id(1), name: "a.txt", size: 600 }]);
    const scratch = await folder();
    const staged = await stageData(root, scratch, limits);
    db.close();
    expect(staged.components.map((component) => component.name)).toEqual(["database", "files"]);
    expect(staged.tables).toEqual([{ name: "notes", rows: 2 }]);
    expect(staged.files).toEqual({ count: 2, bytes: 603 });
    const entries = readTar(new Uint8Array(await readFile(path.join(scratch, "files.tar"))));
    expect(entries.map((entry) => entry.path)).toEqual(["index.json", `${id(2)}.blob`, `${id(1)}.blob`]);
  });

  test("refuses a file over the per-file limit, naming the largest and the totals", async () => {
    const { root } = await appFolder([{ id: id(1), name: "small.png", size: 10 }, { id: id(2), name: "big.mov", size: 3 * 1024 * 1024 }, { id: id(3), name: "bigger.mov", size: 5 * 1024 * 1024 }]);
    const refusal = await checkFiles(path.join(root, "data/files"), limits, undefined).catch((error: Error) => error.message);
    expect(refusal).toContain("2 files are over this installation's limit of 2 MiB per file");
    expect(refusal).toContain(`The largest is bigger.mov (${id(3)}) at 5 MiB.`);
    expect(refusal).toContain("3 files in data/files/index.json come to 8.0 MiB");
    expect(refusal).toContain("Nothing was sent.");
  });

  test("refuses files over the quota and an owner the accounts lack", async () => {
    const { root } = await appFolder([{ id: id(1), name: "a", size: 1500 }, { id: id(2), name: "b", size: 1500 }]);
    const quota = await checkFiles(path.join(root, "data/files"), { quotaBytes: 2048, maxFileBytes: 2048 }, undefined).catch((error: Error) => error.message);
    expect(quota).toContain("2 files in data/files/index.json come to 2.9 KiB, over this installation's quota of 2 KiB per app");
    await writeFile(path.join(root, "data/files/index.json"), JSON.stringify([{ id: id(1), name: "a", size: 1500, ownerId: "user-1" }]));
    const owner = await checkFiles(path.join(root, "data/files"), limits, new Set(["user-2"])).catch((error: Error) => error.message);
    expect(owner).toContain("belongs to account user-1");
  });

  test("refuses a blob whose size the index does not give", async () => {
    const { root } = await appFolder([{ id: id(1), name: "a", size: 4 }]);
    await writeFile(path.join(root, `data/files/${id(1)}.blob`), "longer than four");
    const refusal = await checkFiles(path.join(root, "data/files"), limits, undefined).catch((error: Error) => error.message);
    expect(refusal).toContain("is 16 bytes, but data/files/index.json says 4");
  });
});
