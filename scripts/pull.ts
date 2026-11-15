// `konae pull` exports the published app's data into .konae/pull, an app
// folder that holds only data/, and `konae dev --reset` imports it with the
// kernel binary's import command. Nothing here writes to the app it came
// from; push.ts sends data/ back.
import { mkdir, mkdtemp, readFile, readdir, rename as fsRename, rm, stat, writeFile } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import type { McpClient, Transfer } from "../src/mcp/client";
import { checkPackageManifest, packageComponentFiles, type PackageComponentName, type PackageManifest } from "../src/lib/portable-package";
import { dataEntries, SETTINGS_FILE } from "../src/lib/app-folder";
import { downloadToFile } from "./package-files";
import { readTar } from "../src/lib/tar";
import { sourceDigest } from "../src/deployments/source-digest";
import { sourceFilesFrom } from "../src/lib/source-files";
import { settingsDigest } from "./settings-digest";
import { matchesSourceRevisionScope, sourceRevisionScope } from "./source-revision";

/** Where the exported data sits in the app folder. .konae is already kept out of Git. */
export const PULL_DIRECTORY = ".konae/pull";
const RECORD = "pull.json";

/** What pull.json holds: the export's manifest, the installation it came from and when. */
export type Pulled = { manifest: PackageManifest; from: string; at: number; sourcesRevision?: number; sourceDigest?: string; sourcesInstalled?: boolean };

export const NEEDS_EXPORT_DATA = "Downloading live data needs a credential with the export_data permission. Run login with --export-data, or create a token with export_data for this project.";

/**
 * Exports the app with project_export and writes each data component into
 * `directory` under its file name, checked against the manifest's size and
 * SHA-256. Every download transfer is released, the folder download too,
 * whether it was read or not.
 */
export async function downloadPackage(client: McpClient, deploymentId: string, directory: string, includeFolder = true) {
  let exported: { manifest: unknown; downloads: Record<string, Transfer>; folder?: Transfer };
  try {
    exported = await client.call("project_export", { deploymentId });
  } catch (error) {
    if (error instanceof Error && error.message.includes("Unknown tool")) throw Error(NEEDS_EXPORT_DATA);
    throw error;
  }
  try {
    const manifest = checkPackageManifest(exported.manifest);
    const files: { name: string; file: string; size: number }[] = [];
    for (const content of manifest.contents) {
      const transfer = exported.downloads[content.name];
      if (!transfer || transfer.size !== content.size || transfer.sha256 !== content.sha256) throw Error(`The ${content.name} download does not match the export's manifest.`);
      const file = path.join(directory, packageComponentFiles[content.name]);
      await downloadToFile(client, transfer, file);
      files.push({ name: content.name, file, size: content.size });
    }
    let folderFile: string | undefined;
    if (includeFolder && exported.folder) {
      folderFile = path.join(directory, "folder.tar");
      if (!Number.isSafeInteger(exported.folder.size) || exported.folder.size < 0) throw new McpError("Invalid app folder download size.");
      await downloadToFile(client, exported.folder, folderFile);
    }
    return { manifest, files, folderFile };
  } finally {
    for (const transfer of [...Object.values(exported.downloads), ...(exported.folder ? [exported.folder] : [])]) await client.cancel(transfer.transferId).catch(() => {});
  }
}

/**
 * Exports the published app's data into the data/ of the app folder
 * `folder`, laid out as an export lays it out. Answers the export's manifest.
 */
export async function downloadData(client: McpClient, deploymentId: string, folder: string) {
  const scratch = await mkdtemp(path.join(tmpdir(), "konae-data-"));
  try {
    const { manifest, files } = await downloadPackage(client, deploymentId, scratch, false);
    for (const { name, file } of files) {
      for (const entry of dataEntries(name as PackageComponentName, new Uint8Array(await readFile(file)))) {
        const target = path.resolve(folder, entry.path);
        if (!target.startsWith(path.resolve(folder) + path.sep)) throw Error(`Invalid folder path ${entry.path}.`);
        await mkdir(path.dirname(target), { recursive: true });
        await writeFile(target, entry.contents);
      }
    }
    return manifest;
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

/**
 * Exports the published app's data into .konae/pull/data. The download
 * lands in a folder beside it first, so a pull that fails keeps the data an
 * earlier pull left.
 */
export async function pullLocal(root: string, client: McpClient, from: string, projectId: string, operations: { rename: typeof fsRename } = { rename: fsRename }): Promise<Pulled> {
  const remote = await client.call<{ deployment?: { id: string } | null }>("project_inspect", { projectId });
  if (!remote.deployment) throw Error("This project has no Deployment to pull from. Publish it first.");
  await mkdir(path.join(root, ".konae"), { recursive: true });
  const scratch = await mkdtemp(path.join(root, ".konae/pull-"));
  try {
    const downloaded = await downloadPackage(client, remote.deployment.id, scratch);
    const manifest = downloaded.manifest;
    if (!downloaded.folderFile) throw new Error("The export has no app folder download.");
    const sourceMetadata = await client.call<{ revision: number; project?: { id: string; name: string; slug: string; manifest: Record<string, unknown> } | null }>("project_sources_read", { projectId, metadataOnly: true });
    if (manifest.sourcesRevision != null && sourceMetadata.revision !== manifest.sourcesRevision) throw new Error("Sources changed during pull. Run konae pull again.");
    if (sourceMetadata.project && sourceMetadata.project.id !== projectId) throw new Error("The exported sources do not match this project.");
    const folderEntries = readTar(new Uint8Array(await readFile(downloaded.folderFile)));
    const settingsEntries = folderEntries.filter((entry) => entry.path === SETTINGS_FILE);
    if (settingsEntries.length !== 1) throw new Error("The export must contain one " + SETTINGS_FILE + ".");
    let remoteSettings = JSON.parse(new TextDecoder().decode(settingsEntries[0]!.contents)) as Record<string, unknown>;
    if (sourceMetadata.project) {
      const sourceManifest = { ...sourceMetadata.project.manifest };
      const visibility = sourceManifest.visibility;
      delete sourceManifest.visibility;
      delete sourceManifest.signupPolicy;
      remoteSettings.name = sourceMetadata.project.name;
      remoteSettings.slug = sourceMetadata.project.slug;
      if (visibility === "private" || visibility === "public") remoteSettings.visibility = visibility;
      remoteSettings.manifest = sourceManifest;
    }
    const remoteSettingsBytes = new TextEncoder().encode(JSON.stringify(remoteSettings, null, 2) + "\n");
    const remoteSettingsDigest = settingsDigest(remoteSettings);
    await writeFile(path.join(scratch, SETTINGS_FILE), remoteSettingsBytes);
    const sourceEntries = folderEntries
      .filter((entry) => entry.path.startsWith("app/"))
      .map((entry) => ({ path: entry.path.slice(4), contents: entry.contents }));
    const artifactEntries = folderEntries
      .filter((entry) => entry.path.startsWith("dist/"))
      .map((entry) => ({ path: entry.path.slice(5), contents: entry.contents }));
    if (!sourceEntries.some((entry) => entry.path === "actions.json")) throw new Error("The export's app/ has no actions.json.");
    const stagedApp = path.join(scratch, "app");
    await mkdir(stagedApp, { recursive: true });
    for (const entry of sourceEntries) {
      const target = path.resolve(stagedApp, entry.path);
      if (!target.startsWith(path.resolve(stagedApp) + path.sep)) throw new Error(`Invalid app folder path ${entry.path}.`);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, entry.contents);
    }
    const stagedDist = path.join(scratch, "dist");
    await mkdir(stagedDist, { recursive: true });
    for (const entry of artifactEntries) {
      const target = path.resolve(stagedDist, entry.path);
      if (!target.startsWith(path.resolve(stagedDist) + path.sep)) throw new Error(`Invalid dist folder path ${entry.path}.`);
      await mkdir(path.dirname(target), { recursive: true });
      await writeFile(target, entry.contents);
    }
    for (const { name, file } of downloaded.files) {
      for (const entry of dataEntries(name as PackageComponentName, new Uint8Array(await readFile(file)))) {
        const target = path.resolve(scratch, entry.path);
        if (!target.startsWith(path.resolve(scratch) + path.sep)) throw Error(`Invalid folder path ${entry.path}.`);
        await mkdir(path.dirname(target), { recursive: true });
        await writeFile(target, entry.contents);
      }
    }
    const source = sourceFilesFrom(await listFiles(stagedApp));
    const digest = await sourceDigest(source);
    const pulled: Pulled = { manifest, from, at: Date.now(), sourcesRevision: manifest.sourcesRevision ?? 0, sourceDigest: digest };
    const target = path.join(root, PULL_DIRECTORY);
    const currentApp = path.join(root, "app");
    const markerPath = path.join(root, ".konae/source-revision.json");
    let marker: { sourceDigest?: string; settingsDigest?: string; installation?: string; projectId?: string } | undefined;
    try { marker = JSON.parse(await readFile(markerPath, "utf8")) as typeof marker; } catch { marker = undefined; }
    if (!matchesSourceRevisionScope(marker, from, projectId)) marker = undefined;
    let localAppDigest: string | undefined;
    try { localAppDigest = await sourceDigest(sourceFilesFrom(await listFiles(currentApp))); } catch { localAppDigest = undefined; }
    let localSettingsDigest: string | undefined;
    try { localSettingsDigest = settingsDigest(JSON.parse(await readFile(path.join(root, SETTINGS_FILE), "utf8"))); } catch { localSettingsDigest = undefined; }
    const incomingDigest = pulled.sourceDigest;
    const cleanApp = localAppDigest !== undefined && (localAppDigest === marker?.sourceDigest || localAppDigest === incomingDigest);
    const cleanSettings = localSettingsDigest !== undefined && (localSettingsDigest === marker?.settingsDigest || localSettingsDigest === remoteSettingsDigest);
    const cleanToReplace = cleanApp && cleanSettings;
    pulled.sourcesInstalled = cleanToReplace;
    await writeFile(path.join(scratch, RECORD), JSON.stringify(pulled, null, 2), { mode: 0o600 });
    const nonce = randomUUID();
    const retiredApp = path.join(root, `.app-pull-retired-${nonce}`);
    const retiredSettings = path.join(root, `.settings-pull-retired-${nonce}`);
    const retiredPull = path.join(root, `.konae-pull-retired-${nonce}`);
    const retiredMarker = path.join(root, `.source-revision-retired-${nonce}`);
    const stagedMarker = path.join(root, `.source-revision-staged-${nonce}`);
    const stagedSettings = path.join(root, `.settings-pull-staged-${nonce}`);
    let movedApp = false; let installedApp = false; let movedSettings = false; let installedSettings = false; let movedPull = false; let installedPull = false; let movedMarker = false; let installedMarker = false;
    if (cleanToReplace) {
      await writeFile(stagedMarker, JSON.stringify({ ...sourceRevisionScope(from, projectId), sourcesRevision: pulled.sourcesRevision, sourceDigest: pulled.sourceDigest, settingsDigest: remoteSettingsDigest }, null, 2), { mode: 0o600 });
      await writeFile(stagedSettings, remoteSettingsBytes, { mode: 0o600 });
    }
    try {
      if (cleanToReplace) {
        if (await exists(currentApp)) { await operations.rename(currentApp, retiredApp); movedApp = true; }
        await operations.rename(stagedApp, currentApp); installedApp = true;
        if (await exists(path.join(root, SETTINGS_FILE))) { await operations.rename(path.join(root, SETTINGS_FILE), retiredSettings); movedSettings = true; }
        await operations.rename(stagedSettings, path.join(root, SETTINGS_FILE)); installedSettings = true;
      }
      if (await exists(target)) { await operations.rename(target, retiredPull); movedPull = true; }
      await operations.rename(scratch, target);
      installedPull = true;
      if (cleanToReplace) {
        if (await exists(markerPath)) { await operations.rename(markerPath, retiredMarker); movedMarker = true; }
        await operations.rename(stagedMarker, markerPath);
        installedMarker = true;
      }
    } catch (error) {
      if (installedMarker) await rm(markerPath, { force: true });
      if (movedMarker && await exists(retiredMarker)) await operations.rename(retiredMarker, markerPath);
      if (installedPull && await exists(target)) await rm(target, { recursive: true, force: true });
      if (movedPull && await exists(retiredPull)) await operations.rename(retiredPull, target);
      if (installedSettings) await rm(path.join(root, SETTINGS_FILE), { force: true });
      if (movedSettings && await exists(retiredSettings)) await operations.rename(retiredSettings, path.join(root, SETTINGS_FILE));
      if (installedApp) await rm(currentApp, { recursive: true, force: true });
      if (movedApp && await exists(retiredApp)) await operations.rename(retiredApp, currentApp);
      throw error;
    } finally {
      await rm(stagedMarker, { force: true });
      await rm(stagedSettings, { force: true });
    }
    await rm(retiredPull, { recursive: true, force: true });
    await rm(retiredApp, { recursive: true, force: true });
    await rm(retiredSettings, { force: true });
    await rm(retiredMarker, { force: true });
    return pulled;
  } catch (error) {
    await rm(scratch, { recursive: true, force: true });
    throw error;
  }
}

async function exists(file: string) { try { await stat(file); return true; } catch { return false; } }

async function listFiles(directory: string): Promise<[string, Uint8Array][]> {
  const files: [string, Uint8Array][] = [];
  const pending = [directory];
  while (pending.length) {
    const current = pending.pop()!;
    for (const entry of await readdir(current, { withFileTypes: true })) {
      if (entry.name.startsWith(".") || entry.name === "node_modules") continue;
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) pending.push(full);
      else if (entry.isFile()) files.push([path.relative(directory, full).split(path.sep).join("/"), new Uint8Array(await readFile(full))]);
    }
  }
  return files;
}

/** The data a pull exported into the folder, or undefined when there is none. */
export async function readPulled(root: string): Promise<Pulled | undefined> {
  let text: string;
  try { text = await readFile(path.join(root, PULL_DIRECTORY, RECORD), "utf8"); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined; throw error; }
  const pulled = JSON.parse(text) as Pulled;
  return { ...pulled, manifest: checkPackageManifest(pulled.manifest) };
}

/** One phrase for the exported data: where it came from, when, and each component with its size. */
export function describePulled(pulled: Pulled) {
  const size = (bytes: number) => bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KiB` : `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  const parts = pulled.manifest.contents.map((content) => `${content.name} ${size(content.size)}`).join(", ");
  return `that ${new URL(pulled.from).host} held at ${new Date(pulled.at).toISOString().replace(/\.\d+Z$/, "Z")} (${parts})`;
}
