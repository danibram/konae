export type ProjectTree = {
  files: Record<string, string>;
  directories: string[];
};

/**
 * Files every project has, which the editor never creates, renames, moves or
 * deletes as ordinary tree entries. schema.ts is listed even when the project
 * does not hold it as a file: the export writes the database model there, and
 * a user file at that path would replace the real model.
 */
export const protectedProjectPaths: ReadonlySet<string> = new Set(["index.html", "styles.css", "app.js", "schema.sql", "actions.json", "schema.ts"]);

/**
 * The key two project paths share when a case-insensitive filesystem (the
 * default on macOS and Windows) would write them to the same place. An export
 * of `App.js` next to `app.js` keeps only one of them.
 */
export function foldPath(path: string): string {
  return path.normalize("NFC").toLowerCase();
}

/**
 * The protected path that `path` is or would sit inside, or null. The match
 * ignores case: `App.js` or `SCHEMA.TS` overwrites the real file once the
 * project is exported to a case-insensitive filesystem.
 */
export function reservedPathConflict(path: string): string | null {
  const folded = foldPath(path);
  for (const reserved of protectedProjectPaths) {
    if (folded === reserved || folded.startsWith(`${reserved}/`)) return reserved;
  }
  return null;
}

/** The existing file at `path`, compared without case, or null. */
export function existingFile(files: Record<string, string>, path: string): string | null {
  const folded = foldPath(path);
  for (const name of Object.keys(files)) {
    if (typeof files[name] === "string" && foldPath(name) === folded) return name;
  }
  return null;
}

/** The existing directory at `path`, compared without case, or null. */
export function existingDirectory(directories: Iterable<string>, path: string): string | null {
  const folded = foldPath(path);
  for (const name of directories) {
    if (foldPath(name) === folded) return name;
  }
  return null;
}

export function requireUnreservedPath(path: string) {
  const reserved = reservedPathConflict(path);
  if (reserved) throw new Error(`${reserved} is a reserved project file. Choose another name.`);
}

export function normalizeProjectPath(input: string): string | null {
  if (typeof input !== "string") return null;
  const trimmed = input.trim().replaceAll("\\", "/").replace(/^\/+/, "").replace(/\/+$/, "");
  if (!trimmed || trimmed.length > 240 || /[\0-\x1f]/.test(trimmed)) return null;
  const parts = trimmed.split("/");
  if (parts.some((part) => !part || part === "." || part === "..")) return null;
  return parts.join("/");
}

/**
 * The existing file that sits where one of `path`'s parent directories would
 * have to be, or null. A tree holding both `app.js` and `app.js/x.js` cannot be
 * written to disk, so the export and every builder would trip over it. Case is
 * ignored, for the reason `reservedPathConflict` gives.
 */
export function fileAncestor(files: Record<string, string>, path: string): string | null {
  const parts = path.split("/");
  for (let index = 1; index < parts.length; index += 1) {
    const parent = existingFile(files, parts.slice(0, index).join("/"));
    if (parent) return parent;
  }
  return null;
}

export function createDirectory(tree: ProjectTree, input: string): ProjectTree {
  const path = requirePath(input);
  requireUnreservedPath(path);
  requireNoFile(tree.files, path);
  requireNoFileAncestor(tree.files, path);
  return { files: { ...tree.files }, directories: mergeDirectories(tree.directories, path) };
}

export function createFile(tree: ProjectTree, input: string, content = ""): ProjectTree {
  const path = requirePath(input);
  requireUnreservedPath(path);
  requireNoFile(tree.files, path);
  const directory = existingDirectory([...tree.directories, ...directoriesForFiles(tree.files)], path);
  if (directory) throw new Error(`A directory already exists at ${directory}.`);
  requireNoFileAncestor(tree.files, path);
  const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  return {
    files: { ...tree.files, [path]: content },
    directories: parent ? mergeDirectories(tree.directories, parent) : [...tree.directories].sort(),
  };
}

export function removePath(tree: ProjectTree, input: string): ProjectTree {
  const path = requirePath(input);
  const prefix = `${path}/`;
  return {
    files: Object.fromEntries(Object.entries(tree.files).filter(([name]) => name !== path && !name.startsWith(prefix))),
    directories: tree.directories.filter((name) => name !== path && !name.startsWith(prefix)).sort(),
  };
}

export function directoriesForFiles(files: Record<string, string>): string[] {
  let directories: string[] = [];
  for (const path of Object.keys(files)) {
    const parts = path.split("/").slice(0, -1);
    for (let index = 1; index <= parts.length; index += 1) {
      directories = mergeDirectories(directories, parts.slice(0, index).join("/"));
    }
  }
  return directories;
}

function mergeDirectories(current: string[], path: string): string[] {
  const parts = path.split("/");
  const next = new Set(current);
  for (let index = 1; index <= parts.length; index += 1) next.add(parts.slice(0, index).join("/"));
  return [...next].sort();
}

function requireNoFile(files: Record<string, string>, path: string) {
  const existing = existingFile(files, path);
  if (existing) throw new Error(`A file already exists at ${existing}.`);
}

function requireNoFileAncestor(files: Record<string, string>, path: string) {
  const blocking = fileAncestor(files, path);
  if (blocking) throw new Error(`${blocking} is a file, so it cannot contain ${path}.`);
}

export const invalidPathMessage = "Use a relative project path, without \".\" or \"..\" segments.";

function requirePath(input: string): string {
  const path = normalizeProjectPath(input);
  if (!path) throw new Error(invalidPathMessage);
  return path;
}
