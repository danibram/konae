export type ProjectTree = {
  files: Record<string, string>;
  directories: string[];
};

export function normalizeProjectPath(input: string): string | null {
  if (typeof input !== "string") return null;
  const trimmed = input.trim().replaceAll("\\", "/").replace(/^\/+/, "").replace(/\/+$/, "");
  if (!trimmed || trimmed.length > 240 || /[\0-\x1f]/.test(trimmed)) return null;
  const parts = trimmed.split("/");
  if (parts.some((part) => !part || part === "." || part === "..")) return null;
  return parts.join("/");
}

export function createDirectory(tree: ProjectTree, input: string): ProjectTree {
  const path = requirePath(input);
  if (tree.files[path] !== undefined) throw new Error(`A file already exists at ${path}.`);
  return { files: { ...tree.files }, directories: mergeDirectories(tree.directories, path) };
}

export function createFile(tree: ProjectTree, input: string, content = ""): ProjectTree {
  const path = requirePath(input);
  if (tree.files[path] !== undefined) throw new Error(`A file already exists at ${path}.`);
  if (tree.directories.includes(path)) throw new Error(`A directory already exists at ${path}.`);
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

function requirePath(input: string): string {
  const path = normalizeProjectPath(input);
  if (!path) throw new Error("Use a relative project path without . or ...");
  return path;
}
