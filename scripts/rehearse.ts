import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import { checkPackageManifest } from "../src/lib/portable-package";
import { dataEntries, type PackageComponentName } from "../src/lib/app-folder";
import { readTar } from "../src/lib/tar";
import { sourceFilesFrom } from "../src/lib/source-files";
import { sourceDigest } from "../src/deployments/source-digest";
import { PULL_DIRECTORY, readPulled } from "./pull";

/** Evidence returned after the Kernel imports the immutable snapshot from pull. */
export type RehearsalEvidence = { snapshotId: string; sourceDigest: string; folderDigest: string; packageId: string };

/** Builds a scratch app from the exact folder and data exported by the latest pull, then runs its Kernel import. */
export async function rehearsePulledSnapshot(root: string, importSnapshot: (appFolder: string, snapshotFolder: string) => Promise<unknown>): Promise<RehearsalEvidence> {
  const pull = path.join(root, PULL_DIRECTORY);
  const pulled = await readPulled(root);
  if (!pulled) throw Error("Run konae pull before rehearse so the exact published snapshot is available.");
  const scratch = await mkdtemp(path.join(tmpdir(), "konae-rehearsal-"));
  try {
    const folderTar = new Uint8Array(await readFile(path.join(pull, "folder.tar")));
    const folderDigest = createHash("sha256").update(folderTar).digest("hex");
    const entries = readTar(folderTar);
    for (const entry of entries) {
      const target = path.resolve(scratch, entry.path);
      if (!target.startsWith(path.resolve(scratch) + path.sep)) throw Error(`Invalid pulled folder path ${entry.path}.`);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, entry.contents);
    }
    const manifest = checkPackageManifest(pulled.manifest);
    const snapshot = path.join(scratch, "snapshot");
    await mkdir(path.join(snapshot, "data"), { recursive: true });
    for (const component of manifest.contents) {
      const name = component.name as PackageComponentName;
      const relative = name === "database" ? "data/database.sqlite" : name === "accounts" ? "data/identity.sqlite" : name === "files" ? "data/files.json" : name === "usage" ? "data/usage.sqlite" : "";
      const source = path.join(pull, relative);
      const bytes = new Uint8Array(await readFile(source));
      const digest = createHash("sha256").update(bytes).digest("hex");
      if (bytes.length !== component.size || digest !== component.sha256) throw Error(`Pulled ${component.name} no longer matches snapshot ${manifest.packageId}. Run konae pull again.`);
      for (const entry of dataEntries(name, bytes)) {
        const target = path.resolve(snapshot, entry.path);
        if (!target.startsWith(snapshot + path.sep)) throw Error(`Invalid snapshot path ${entry.path}.`);
        await mkdir(path.dirname(target), { recursive: true });
        await writeFile(target, entry.contents);
      }
    }
    const sources = await sourceDigest(sourceFilesFrom(await listSourceFiles(path.join(scratch, "app"))));
    await importSnapshot(scratch, snapshot);
    return { snapshotId: manifest.packageId, packageId: manifest.packageId, sourceDigest: sources, folderDigest };
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

async function listSourceFiles(directory: string): Promise<[string, Uint8Array][]> {
  const files: [string, Uint8Array][] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await listSourceFiles(full).then((items) => items.map(([name, bytes]) => [`${entry.name}/${name}`, bytes] as [string, Uint8Array])));
    else if (entry.isFile()) files.push([entry.name, new Uint8Array(await readFile(full))]);
  }
  return files;
}
