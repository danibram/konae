import { checksum, encodeBase64 } from "../mcp/client";
import { sourceFilesFrom, type SourceFiles } from "./source-files";
import { skippedName } from "./source-rules";
import { DATA_FOLDER, DIST_FOLDER, SETTINGS_FILE, SOURCES_FOLDER, dataPaths, type AppSettings } from "./app-folder";
import { packageFormat, packageVersion, type PackageComponentName, type PackageContent, type PackageManifest } from "./portable-package";
import { createTar, readTar } from "./tar";

export type ImportedFolder = {
  settings: AppSettings;
  sources: SourceFiles;
  dist: Record<string, Uint8Array>;
  data: Partial<Record<PackageComponentName, Uint8Array>>;
};

const text = new TextDecoder("utf-8", { fatal: true });

export function readAppFolder(archive: Uint8Array): ImportedFolder {
  const entries = readTar(archive);
  const top = entries.find((entry) => entry.path === SETTINGS_FILE || entry.path.endsWith(`/${SETTINGS_FILE}`) && entry.path.split("/").length === 2);
  if (!top) throw new Error(`This archive has no ${SETTINGS_FILE}, so it is not an exported app folder.`);
  const prefix = top.path.slice(0, -SETTINGS_FILE.length);
  const inside = new Map(entries.filter((entry) => entry.path.startsWith(prefix)).map((entry) => [entry.path.slice(prefix.length), entry.contents]));
  const settings = JSON.parse(text.decode(inside.get(SETTINGS_FILE)!)) as AppSettings;
  const under = (folder: string) => [...inside].filter(([path]) => path.startsWith(`${folder}/`)).map(([path, contents]) => [path.slice(folder.length + 1), contents] as const);
  const sources = sourceFilesFrom(under(SOURCES_FOLDER).filter(([path]) => !path.split("/").some(skippedName)));
  const dist = Object.fromEntries(under(DIST_FOLDER));
  const data: ImportedFolder["data"] = {};
  for (const name of ["database", "identity", "usage"] as const) {
    const bytes = inside.get(dataPaths[name]);
    if (bytes) data[name] = bytes;
  }
  const files = new Map(under(`${DATA_FOLDER}/files`));
  const index = files.get("index.json");
  if (index) {
    const listed = JSON.parse(text.decode(index)) as { id: string }[];
    data.files = createTar([{ path: "index.json", contents: index }, ...listed.map(({ id }) => {
      const blob = files.get(`${id}.blob`);
      if (!blob) throw new Error(`${dataPaths.files}/index.json lists file ${id}, but the folder has no ${id}.blob.`);
      return { path: `${id}.blob`, contents: blob };
    })]);
  }
  return { settings, sources, dist, data };
}

export function sourcesPayload(folder: ImportedFolder, project: { id: string; name: string; slug: string }, visibility: "public" | "private") {
  if (!folder.dist["index.html"]) throw new Error(`The folder has no ${DIST_FOLDER}/index.html. Build it before importing its sources.`);
  const files: Record<string, string> = {};
  const encodings: Record<string, "base64"> = {};
  for (const [path, bytes] of Object.entries(folder.dist)) {
    try { files[path] = text.decode(bytes); } catch { files[path] = encodeBase64(bytes); encodings[path] = "base64"; }
  }
  return {
    project: { ...project, ...folder.sources, manifest: { ...folder.settings.manifest, visibility } },
    visibility,
    artifact: { version: 1, entrypoint: "index.html", files, ...(Object.keys(encodings).length ? { encodings } : {}) },
  };
}

export async function dataManifest(folder: ImportedFolder, projectId: string): Promise<PackageManifest> {
  if (!folder.data.database) throw new Error(`The folder has no ${dataPaths.database}, so it carries no data to import.`);
  const contents: PackageContent[] = [];
  for (const name of ["database", "identity", "files", "usage"] as const) {
    const bytes = folder.data[name];
    if (bytes) contents.push({ name, size: bytes.length, sha256: await checksum(bytes) });
  }
  return {
    format: packageFormat, version: packageVersion, packageId: crypto.randomUUID(), createdAt: Date.now(),
    deploymentId: folder.settings.id, projectId, visibility: folder.settings.visibility,
    publishedRevision: 0, contents, excluded: [],
  };
}
