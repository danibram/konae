// Reads an app folder, as the editor's Export writes it in a tar, back into
// what `project_import` takes: a release payload for the sources and the
// export's manifest and parts for the data. It is the inverse of export-project.ts and
// `dataEntries` in app-folder.ts.
import { checksum, encodeBase64 } from "../mcp/client";
import { DATA_FOLDER, DIST_FOLDER, SETTINGS_FILE, SOURCES_FOLDER, dataPaths, type AppSettings } from "./app-folder";
import { packageFormat, packageVersion, type PackageComponentName, type PackageContent, type PackageManifest } from "./portable-package";
import { createTar, readTar } from "./tar";

export type ImportedFolder = {
  settings: AppSettings;
  /** The text files of `app/`, by path inside it. */
  sources: Record<string, string>;
  /** The files of `dist/`, by path inside it. */
  dist: Record<string, Uint8Array>;
  /** Each data component as the bytes a package upload carries. */
  data: Partial<Record<PackageComponentName, Uint8Array>>;
};

const text = new TextDecoder("utf-8", { fatal: true });

/** Reads the tar. The folder may sit at the archive's root or in one top folder, as Export names it. */
export function readAppFolder(archive: Uint8Array): ImportedFolder {
  const entries = readTar(archive);
  const top = entries.find((entry) => entry.path === SETTINGS_FILE || entry.path.endsWith(`/${SETTINGS_FILE}`) && entry.path.split("/").length === 2);
  if (!top) throw new Error(`This archive has no ${SETTINGS_FILE}, so it is not an exported app folder.`);
  const prefix = top.path.slice(0, -SETTINGS_FILE.length);
  const inside = new Map(entries.filter((entry) => entry.path.startsWith(prefix)).map((entry) => [entry.path.slice(prefix.length), entry.contents]));
  const settings = JSON.parse(text.decode(inside.get(SETTINGS_FILE)!)) as AppSettings;
  const under = (folder: string) => [...inside].filter(([path]) => path.startsWith(`${folder}/`)).map(([path, contents]) => [path.slice(folder.length + 1), contents] as const);
  const sources: Record<string, string> = {};
  for (const [path, contents] of under(SOURCES_FOLDER)) {
    try { sources[path] = text.decode(contents); } catch { throw new Error(`${SOURCES_FOLDER}/${path} is not text. The sources of an app are text files.`); }
  }
  const dist = Object.fromEntries(under(DIST_FOLDER));
  const data: ImportedFolder["data"] = {};
  for (const name of ["database", "identity", "usage"] as const) {
    const bytes = inside.get(dataPaths[name]);
    if (bytes) data[name] = bytes;
  }
  const files = new Map(under(`${DATA_FOLDER}/files`));
  const index = files.get("index.json");
  if (index) {
    // The files component is index.json, then each file's bytes in index order.
    const listed = JSON.parse(text.decode(index)) as { id: string }[];
    data.files = createTar([{ path: "index.json", contents: index }, ...listed.map(({ id }) => {
      const blob = files.get(`${id}.blob`);
      if (!blob) throw new Error(`${dataPaths.files}/index.json lists file ${id}, but the folder has no ${id}.blob.`);
      return { path: `${id}.blob`, contents: blob };
    })]);
  }
  return { settings, sources, dist, data };
}

/** The release payload for the folder's sources, published under the app's own ID and visibility. */
export function sourcesPayload(folder: ImportedFolder, project: { id: string; name: string; slug: string }, visibility: "public" | "private") {
  if (!folder.dist["index.html"]) throw new Error(`The folder has no ${DIST_FOLDER}/index.html. Build it before importing its sources.`);
  const files: Record<string, string> = {};
  const encodings: Record<string, "base64"> = {};
  for (const [path, bytes] of Object.entries(folder.dist)) {
    try { files[path] = text.decode(bytes); } catch { files[path] = encodeBase64(bytes); encodings[path] = "base64"; }
  }
  return {
    project: { ...project, files: folder.sources, manifest: { ...folder.settings.manifest, visibility } },
    visibility,
    artifact: { version: 1, entrypoint: "index.html", files, ...(Object.keys(encodings).length ? { encodings } : {}) },
  };
}

/**
 * The manifest of the folder's data, with each component's size and SHA-256.
 * An import into an existing app keeps that app's Signup Policy and sign-in
 * methods, so the manifest names neither.
 */
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
