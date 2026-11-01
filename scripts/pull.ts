// `konae pull`: a copy of a Deployment's live package kept under
// .konae/pull, which `konae dev` installs on a Development Kernel
// that holds no data yet. The copy travels one way. Nothing here writes to
// the Deployment it came from.
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import type { McpClient, Transfer } from "../src/mcp/client";
import { checkPackageManifest, packageComponentFiles, type PackageManifest } from "../src/lib/portable-package";
import { downloadToFile, uploadFromFile } from "./package-files";

/** Where the copy sits in a project folder. .konae is already kept out of Git. */
export const PULL_DIRECTORY = ".konae/pull";
const RECORD = "pull.json";

/** What pull.json holds: the package's manifest, the installation it came from and when. */
export type Pulled = { manifest: PackageManifest; from: string; at: number };

export const NEEDS_EXPORT_DATA = "Downloading live data needs a credential with the export_data permission. Run login with --export-data, or create a token with export_data for this project.";

/**
 * Captures the Deployment with package_export and writes each component into
 * `directory` under its kit file name, checked against the manifest's size
 * and SHA-256. Every download transfer is released, whether it was read or not.
 */
export async function downloadPackage(client: McpClient, deploymentId: string, directory: string) {
  let exported: { manifest: unknown; downloads: Record<string, Transfer> };
  try {
    exported = await client.call("package_export", { deploymentId });
  } catch (error) {
    if (error instanceof Error && error.message.includes("Unknown tool")) throw Error(NEEDS_EXPORT_DATA);
    throw error;
  }
  try {
    const manifest = checkPackageManifest(exported.manifest);
    const files: { name: string; file: string; size: number }[] = [];
    for (const content of manifest.contents) {
      const transfer = exported.downloads[content.name];
      if (!transfer || transfer.size !== content.size || transfer.sha256 !== content.sha256) throw Error(`The ${content.name} download does not match the package manifest.`);
      const file = path.join(directory, packageComponentFiles[content.name]);
      await downloadToFile(client, transfer, file);
      files.push({ name: content.name, file, size: content.size });
    }
    return { manifest, files };
  } finally {
    for (const transfer of Object.values(exported.downloads)) await client.cancel(transfer.transferId).catch(() => {});
  }
}

/**
 * Downloads the project's live package into .konae/pull. The download
 * lands in a folder beside it first, so a pull that fails keeps the copy an
 * earlier pull left.
 */
export async function pullLocal(root: string, client: McpClient, from: string, projectId: string): Promise<Pulled> {
  const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId });
  if (!remote.deployment) throw Error("This project has no Deployment to pull from. Publish it first.");
  await mkdir(path.join(root, ".konae"), { recursive: true });
  const scratch = await mkdtemp(path.join(root, ".konae/pull-"));
  try {
    const { manifest } = await downloadPackage(client, remote.deployment.id, scratch);
    const pulled: Pulled = { manifest, from, at: Date.now() };
    await writeFile(path.join(scratch, RECORD), JSON.stringify(pulled, null, 2), { mode: 0o600 });
    const target = path.join(root, PULL_DIRECTORY);
    await rm(target, { recursive: true, force: true });
    await rename(scratch, target);
    return pulled;
  } catch (error) {
    await rm(scratch, { recursive: true, force: true });
    throw error;
  }
}

/** The copy a pull left in the folder, or undefined when there is none. */
export async function readPulled(root: string): Promise<Pulled | undefined> {
  let text: string;
  try { text = await readFile(path.join(root, PULL_DIRECTORY, RECORD), "utf8"); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined; throw error; }
  const pulled = JSON.parse(text) as Pulled;
  return { ...pulled, manifest: checkPackageManifest(pulled.manifest) };
}

/** Uploads each component of the pulled copy and returns the transfer IDs a release names in `initialPackage.components`. */
export async function uploadPulled(client: McpClient, root: string, pulled: Pulled, projectId: string) {
  const components: Record<string, string> = {};
  try {
    for (const content of pulled.manifest.contents) {
      components[content.name] = await uploadFromFile(client, path.join(root, PULL_DIRECTORY, packageComponentFiles[content.name]), content, projectId);
    }
    return components;
  } catch (error) {
    for (const id of Object.values(components)) await client.cancel(id).catch(() => {});
    throw error;
  }
}

/** One phrase for the copy: where it came from, when, and each component with its size. */
export function describePulled(pulled: Pulled) {
  const size = (bytes: number) => bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KiB` : `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  const parts = pulled.manifest.contents.map((content) => `${content.name} ${size(content.size)}`).join(", ");
  return `the copy of ${new URL(pulled.from).host} pulled ${new Date(pulled.at).toISOString().replace(/\.\d+Z$/, "Z")} (${parts})`;
}
