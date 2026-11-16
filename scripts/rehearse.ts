import { cp, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import { checkPackageManifest } from "../src/lib/portable-package";
import { SETTINGS_FILE, type AppSettings } from "../src/lib/app-folder";
import { sourceFilesFrom } from "../src/lib/source-files";
import { sourceDigest } from "../src/deployments/source-digest";
import { extractVerifiedPulledData, readPulled } from "./pull";

/** Evidence returned after the Kernel imports the immutable snapshot from pull. */
export type RehearsalEvidence = { snapshotId: string; candidateSourceDigest: string; sourceBaselineDigest: string | null; sourceBaselineRevision: number | null; snapshotDigest: string; packageId: string };

/** Starts and stops the Kernel once so operator commands can address the scratch Deployment. */
export async function bootLocalKernel(kernel: { binary: string; env: Record<string, string | undefined> }) {
  const server = Bun.spawn([kernel.binary], { env: { ...kernel.env, KONAE_KERNEL_PORT: "0" }, stdout: "pipe", stderr: "pipe" });
  let output = "";
  const collect = async (stream: ReadableStream<Uint8Array>) => {
    const decoder = new TextDecoder();
    for await (const chunk of stream) output += decoder.decode(chunk);
  };
  const stdout = collect(server.stdout as ReadableStream<Uint8Array>);
  const stderr = collect(server.stderr as ReadableStream<Uint8Array>);
  try {
    for (let attempt = 0; attempt < 150 && !/listening on/.test(output); attempt++) {
      if (server.exitCode !== null) break;
      await Bun.sleep(100);
    }
    if (!/listening on/.test(output) || !/Serving .+ at /.test(output)) throw Error(`The scratch Kernel did not serve the candidate app.\n${output.trim()}`);
  } finally {
    server.kill();
    await server.exited;
    await Promise.all([stdout, stderr]);
  }
}

/** Creates the local Deployment record, then imports the snapshot through the Kernel operator. */
export async function importWithLocalKernel(kernel: { binary: string; env: Record<string, string | undefined>; run(args: string[]): Promise<string> }, snapshot: string, confirmation?: string) {
  await bootLocalKernel(kernel);
  return kernel.run(["import", snapshot, ...(confirmation ? ["--confirm-schema", confirmation] : [])]);
}

/** Copies current app sources and build into scratch, then imports the immutable pulled data. */
export async function rehearsePulledSnapshot(root: string, settings: AppSettings, importSnapshot: (appFolder: string, snapshotFolder: string) => Promise<unknown>): Promise<RehearsalEvidence> {
  const pulled = await readPulled(root);
  if (!pulled) throw Error("Run konae pull before rehearse so the exact published snapshot is available.");
  const scratch = await mkdtemp(path.join(tmpdir(), "konae-rehearsal-"));
  try {
    await cp(path.join(root, "app"), path.join(scratch, "app"), { recursive: true });
    await cp(path.join(root, "dist"), path.join(scratch, "dist"), { recursive: true });
    await writeFile(path.join(scratch, SETTINGS_FILE), `${JSON.stringify(settings, null, 2)}\n`);
    const manifest = checkPackageManifest(pulled.manifest);
    const snapshot = path.join(scratch, "snapshot");
    await extractVerifiedPulledData(root, snapshot, pulled);
    const snapshotComponents = manifest.contents.map(({ name, sha256, size }) => `${name}:${sha256}:${size}`);
    const candidateSourceDigest = await sourceDigest(sourceFilesFrom(await listSourceFiles(path.join(scratch, "app"))));
    const sourceBaseline = pulled.sourceBaseline;
    const sourceBaselineDigest = sourceBaseline?.sourceDigest ?? pulled.sourceDigest ?? null;
    const sourceBaselineRevision = sourceBaseline?.sourcesRevision ?? pulled.sourcesRevision ?? null;
    const snapshotDigest = createHash("sha256").update(`${manifest.packageId}\n${snapshotComponents.sort().join("\n")}`).digest("hex");
    await importSnapshot(scratch, snapshot);
    return { snapshotId: pulled.snapshotId ?? manifest.packageId, packageId: manifest.packageId, candidateSourceDigest, sourceBaselineDigest, sourceBaselineRevision, snapshotDigest };
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
