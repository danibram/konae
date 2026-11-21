import { lstat, readFile, realpath } from "node:fs/promises";
import path from "node:path";
import { BUILD_PROJECT_FILE, BUILD_SOURCE_PREFIX, buildProjectEntries, validateBuildPath } from "./build-project";
import { sourceFilesFrom } from "./source-files";
import type { FolderEntry } from "./app-folder";

export async function buildProjectEntriesFromDisk(appRoot: string): Promise<FolderEntry[]> {
  const manifestPath = path.join(appRoot, BUILD_PROJECT_FILE);
  const manifestInfo = await lstat(manifestPath).catch((error: NodeJS.ErrnoException) => error.code === "ENOENT" ? undefined : Promise.reject(error));
  if (!manifestInfo) return [];
  if (!manifestInfo.isFile() || manifestInfo.isSymbolicLink()) throw new Error(`app/${BUILD_PROJECT_FILE} must be a regular file, not a symbolic link.`);
  const raw = await readFile(manifestPath, "utf8");
  const parsed = JSON.parse(raw) as { version?: unknown; files?: unknown };
  if (!Array.isArray(parsed.files)) throw new Error(`app/${BUILD_PROJECT_FILE} must contain a files array.`);
  const names: string[] = parsed.files.map((file) => {
    if (typeof file !== "string") throw new Error(`app/${BUILD_PROJECT_FILE} file paths must be strings.`);
    validateBuildPath(file);
    return file;
  });
  const sourceEntries: [string, Uint8Array][] = [[BUILD_PROJECT_FILE, new TextEncoder().encode(raw)]];
  for (const file of names) {
    const relative = `${BUILD_SOURCE_PREFIX}${file}`;
    const absolute = path.resolve(appRoot, relative);
    if (!absolute.startsWith(`${path.resolve(appRoot)}${path.sep}`)) throw new Error(`Invalid build source path ${JSON.stringify(relative)}.`);
    await assertRegularPath(appRoot, relative);
    sourceEntries.push([relative, new Uint8Array(await readFile(absolute))]);
  }
  return buildProjectEntries(sourceFilesFrom(sourceEntries));
}

async function assertRegularPath(root: string, relative: string) {
  let current = path.resolve(root);
  for (const part of relative.split("/")) {
    current = path.join(current, part);
    const info = await lstat(current);
    if (info.isSymbolicLink()) throw new Error(`Build source app/${relative} traverses a symbolic link.`);
  }
  const info = await lstat(current);
  if (!info.isFile()) throw new Error(`Build source app/${relative} is not a regular file.`);
  const realRoot = await realpath(root);
  const realFile = await realpath(current);
  if (!realFile.startsWith(`${realRoot}${path.sep}`)) throw new Error(`Build source app/${relative} leaves the app folder.`);
}
