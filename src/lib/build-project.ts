import { lstat, readFile, realpath } from "node:fs/promises";
import path from "node:path";
import { sourceContents, sourceFilesFrom, type SourceFiles } from "./source-files";
import type { FolderEntry } from "./app-folder";
import { sourcePathHeld } from "./source-rules";

export const BUILD_PROJECT_FILE = "build.json";
const BUILD_SOURCE_PREFIX = "build/";
const MAX_BUILD_FILES = 200;
const reservedRoots = new Set(["app", "data", "dist", "server", "releases", ".konae", ".git"]);
const sensitiveName = /(?:^|[._-])(credentials?|secrets?|tokens?|sessions?)(?:[._-]|$)/i;
const sensitiveExtension = /\.(?:pem|key|p12|pfx|keystore)$/i;

export type BuildProjectManifest = { version: 1; files: string[] };

export function parseBuildProject(value: string, sources: SourceFiles): BuildProjectManifest | undefined {
  if (sources.files[BUILD_PROJECT_FILE] === undefined) return undefined;
  let input: unknown;
  try { input = JSON.parse(value); } catch { throw new Error(`app/${BUILD_PROJECT_FILE} must contain valid JSON.`); }
  if (!input || typeof input !== "object" || Array.isArray(input)) throw new Error(`app/${BUILD_PROJECT_FILE} must be an object.`);
  const object = input as Record<string, unknown>;
  if (object.version !== 1 || !Array.isArray(object.files) || Object.keys(object).some((key) => key !== "version" && key !== "files")) {
    throw new Error(`app/${BUILD_PROJECT_FILE} must contain only version 1 and a files array.`);
  }
  if (object.files.length === 0 || object.files.length > MAX_BUILD_FILES) throw new Error(`app/${BUILD_PROJECT_FILE} must list 1 to ${MAX_BUILD_FILES} files.`);
  const seen = new Set<string>();
  for (const entry of object.files) {
    if (typeof entry !== "string") throw new Error(`app/${BUILD_PROJECT_FILE} file paths must be strings.`);
    validateBuildPath(entry);
    if (seen.has(entry)) throw new Error(`app/${BUILD_PROJECT_FILE} lists ${JSON.stringify(entry)} more than once.`);
    seen.add(entry);
    const source = `${BUILD_SOURCE_PREFIX}${entry}`;
    if (!sourcePathHeld(source) || sources.files[source] === undefined) throw new Error(`app/${BUILD_PROJECT_FILE} names missing or invalid source app/${source}.`);
  }
  const manifest = { version: 1 as const, files: object.files as string[] };
  const packageFile = "package.json";
  const lockFile = "package-lock.json";
  if (!seen.has(packageFile) || !seen.has(lockFile)) throw new Error(`app/${BUILD_PROJECT_FILE} must include package.json and package-lock.json.`);
  const packageText = sources.files[`${BUILD_SOURCE_PREFIX}${packageFile}`]!;
  if (sources.encodings?.[`${BUILD_SOURCE_PREFIX}${packageFile}`] === "base64") throw new Error("The build project package.json must be UTF-8 text.");
  let packageManifest: { scripts?: { build?: unknown } };
  try { packageManifest = JSON.parse(packageText); } catch { throw new Error("The build project package.json must contain valid JSON."); }
  if (typeof packageManifest.scripts?.build !== "string" || !packageManifest.scripts.build.trim()) throw new Error("The build project package.json must declare scripts.build.");
  const lockText = sources.files[`${BUILD_SOURCE_PREFIX}${lockFile}`]!;
  if (sources.encodings?.[`${BUILD_SOURCE_PREFIX}${lockFile}`] === "base64") throw new Error("The build project package-lock.json must be UTF-8 text.");
  try { JSON.parse(lockText); } catch { throw new Error("The build project package-lock.json must contain valid JSON."); }
  const nodeScript = /(?:^|[\s;&|])node(?:\s+--[^\s]+)*\s+([^\s;&|]+)/.exec(packageManifest.scripts.build)?.[1];
  if (nodeScript) requireRelative(packageFile, nodeScript.startsWith(".") ? nodeScript : `./${nodeScript}`, declaredPaths(manifest), appSourcesFrom(sources));
  validateStaticClosure(manifest, sources);
  return manifest;
}

export function buildProjectEntries(sources: SourceFiles): FolderEntry[] {
  const raw = sources.files[BUILD_PROJECT_FILE];
  if (raw === undefined) return [];
  if (sources.encodings?.[BUILD_PROJECT_FILE] === "base64") throw new Error(`app/${BUILD_PROJECT_FILE} must be UTF-8 text.`);
  const manifest = parseBuildProject(raw, sources)!;
  return manifest.files.map((file) => ({ path: file, contents: sourceContents(sources, `${BUILD_SOURCE_PREFIX}${file}`) }));
}

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

function validateBuildPath(value: string) {
  const parts = value.split("/");
  const base = parts.at(-1) ?? "";
  if (!sourcePathHeld(value) || value.startsWith("/") || /^[a-z]:/i.test(value) || value.includes(":") || path.posix.normalize(value) !== value
    || reservedRoots.has(parts[0]!) || value === "konae.json" || value === "build.json"
    || sensitiveName.test(base) || sensitiveExtension.test(base) || /\.env(?:\.|$)/i.test(base) || base.toLowerCase() === ".npmrc") {
    throw new Error(`app/${BUILD_PROJECT_FILE} has a forbidden or non-normalized path ${JSON.stringify(value)}.`);
  }
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

function validateStaticClosure(manifest: BuildProjectManifest, sources: SourceFiles) {
  const declared = new Set(manifest.files);
  const appSources = new Set(Object.keys(sources.files).filter((name) => !name.startsWith(BUILD_SOURCE_PREFIX) && name !== BUILD_PROJECT_FILE));
  for (const file of manifest.files) {
    if (file.endsWith(".json") && file === "tsconfig.json") {
      if (sources.encodings?.[`${BUILD_SOURCE_PREFIX}${file}`] === "base64") throw new Error("The build project tsconfig.json must be UTF-8 text.");
      let config: { extends?: unknown };
      try { config = JSON.parse(sources.files[`${BUILD_SOURCE_PREFIX}${file}`]!); } catch { throw new Error("The build project tsconfig.json must contain valid JSON."); }
      const bases = config.extends === undefined ? [] : Array.isArray(config.extends) ? config.extends : [config.extends];
      for (const base of bases) {
        if (typeof base === "string" && base.startsWith(".")) requireRelative(file, base, declared, appSources);
      }
    }
    if (!/\.(?:[cm]?js|tsx?)$/.test(file)) continue;
    const text = sources.files[`${BUILD_SOURCE_PREFIX}${file}`];
    if (text === undefined || sources.encodings?.[`${BUILD_SOURCE_PREFIX}${file}`] === "base64") continue;
    const specifiers = [...text.matchAll(/(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|\bimport\s*)["'](\.{1,2}\/[^"']+)["']/g)].map((match) => match[1]!);
    for (const specifier of specifiers) requireRelative(file, specifier, declared, appSources);
  }
}

function declaredPaths(manifest: BuildProjectManifest) { return new Set(manifest.files); }
function appSourcesFrom(sources: SourceFiles) { return new Set(Object.keys(sources.files).filter((name) => !name.startsWith(BUILD_SOURCE_PREFIX) && name !== BUILD_PROJECT_FILE)); }

function requireRelative(from: string, specifier: string, declared: Set<string>, appSources: Set<string>) {
  const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(from), specifier));
  const candidates = [resolved, ...[".mjs", ".cjs", ".js", ".mts", ".cts", ".ts", ".tsx", ".json"].map((extension) => `${resolved}${extension}`), ...["index.mjs", "index.cjs", "index.js", "index.ts", "index.tsx", "index.json"].map((index) => `${resolved}/${index}`)];
  const found = candidates.some((candidate) => declared.has(candidate) || appSources.has(candidate.replace(/^app\//, "")));
  if (!found) throw new Error(`Build project file ${JSON.stringify(from)} imports ${JSON.stringify(specifier)}, but the target is not in app sources or app/${BUILD_SOURCE_PREFIX}.`);
}
