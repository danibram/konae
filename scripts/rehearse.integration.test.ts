import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { packageComponentFiles } from "../src/lib/portable-package";
import { bootLocalKernel, importWithLocalKernel, rehearsePulledSnapshot } from "./rehearse";
import { buildLocal, initLocal, localKernel, readSettings } from "./konae";

const fixtureRoots: string[] = [];
afterEach(async () => { for (const root of fixtureRoots.splice(0)) await rm(root, { recursive: true, force: true }); });

test.skipIf(!process.env.KONAE_REHEARSAL_KERNEL_BIN)("Kernel rehearsal checks the current candidate against pulled data and leaves both copies alone", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "konae-rehearsal-kernel-"));
  fixtureRoots.push(root);
  await initLocal(root, { name: "Rehearsal", template: "tasks" });
  const settings = await readSettings(root);
  await buildLocal(root, { development: true });
  const kernel = await localKernel(root, settings, { build: false });
  await bootLocalKernel(kernel);
  await kernel.run(["sql", "INSERT INTO tasks (title, created_at) VALUES ('published base', 1)"]);
  await kernel.run(["users"]);

  const pull = path.join(root, ".konae/pull");
  await mkdir(pull, { recursive: true });
  const raw: Record<string, Uint8Array> = {
    database: new Uint8Array(await readFile(path.join(root, "data/database.sqlite"))),
    identity: new Uint8Array(await readFile(path.join(root, "data/identity.sqlite"))),
  };
  const indexPath = path.join(root, "data/files/index.json");
  await mkdir(path.dirname(indexPath), { recursive: true });
  const indexBytes = new Uint8Array(await readFile(indexPath).catch(async () => {
    const empty = new Uint8Array(Buffer.from("[]"));
    await writeFile(indexPath, empty);
    return empty;
  }));
  const archivePath = path.join(root, "files.tar");
  const archive = Bun.spawn(["tar", "-cf", archivePath, "-C", path.dirname(indexPath), "index.json"], { stdout: "pipe", stderr: "pipe" });
  expect(await archive.exited).toBe(0);
  raw.files = new Uint8Array(await readFile(archivePath));
  await rm(archivePath, { force: true });

  const contents = Object.entries(raw).map(([name, bytes]) => {
    const file = packageComponentFiles[name as keyof typeof packageComponentFiles];
    return { name, file, bytes, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
  });
  for (const component of contents) await writeFile(path.join(pull, component.file), component.bytes);
  const manifest = {
    format: "konae-package", version: 1, packageId: "pkg_kernel_rehearsal", createdAt: Date.now(),
    deploymentId: settings.id, projectId: settings.id, visibility: settings.visibility, publishedRevision: 1,
    contents: contents.map(({ name, size, sha256 }) => ({ name, size, sha256 })), excluded: [],
  };
  await writeFile(path.join(pull, "pull.json"), JSON.stringify({ manifest, snapshotId: "snapshot-kernel-rehearsal", from: "http://published.test", at: Date.now(), sourceBaseline: { packageId: manifest.packageId, sourceDigest: "published-source", sourcesRevision: 1, capturedAt: Date.now() } }));
  const localDatabase = path.join(root, "data/database.sqlite");
  await kernel.run(["sql", "ALTER TABLE tasks ADD COLUMN required TEXT NOT NULL DEFAULT 'hand-fixed'"]);
  const editedLocalDb = new Uint8Array(await readFile(localDatabase));
  const pulledBase = new Uint8Array(await readFile(path.join(pull, packageComponentFiles.database)));
  const candidateSchema = path.join(root, "app/schema.sql");
  const baseSchema = await readFile(candidateSchema, "utf8");

  await writeFile(candidateSchema, baseSchema.replace("created_at INTEGER NOT NULL", "created_at INTEGER NOT NULL,\n  required TEXT NOT NULL"));
  await expect(rehearsePulledSnapshot(root, settings, async (scratch, snapshot) => {
    const cleanKernel = await localKernel(scratch, settings, { build: false });
    return importWithLocalKernel(cleanKernel, snapshot);
  })).rejects.toThrow(/NOT NULL|not null|default/i);
  expect(await readFile(localDatabase)).toEqual(editedLocalDb);
  expect(await readFile(path.join(pull, packageComponentFiles.database))).toEqual(pulledBase);

  await writeFile(candidateSchema, baseSchema.replace("created_at INTEGER NOT NULL", "created_at INTEGER NOT NULL,\n  note TEXT"));
  const evidence = await rehearsePulledSnapshot(root, settings, async (scratch, snapshot) => {
    const cleanKernel = await localKernel(scratch, settings, { build: false });
    return importWithLocalKernel(cleanKernel, snapshot);
  });
  expect(evidence.candidateSourceDigest).not.toBe(evidence.sourceBaselineDigest);
  expect(evidence.snapshotId).toBe("snapshot-kernel-rehearsal");
  expect(evidence.packageId).toBe(manifest.packageId);
  expect(await readFile(localDatabase)).toEqual(editedLocalDb);
  expect(await readFile(path.join(pull, packageComponentFiles.database))).toEqual(pulledBase);
});
