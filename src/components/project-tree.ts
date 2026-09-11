import { directoriesForFiles, existingDirectory, existingFile, fileAncestor, invalidPathMessage, normalizeProjectPath, protectedProjectPaths, requireUnreservedPath, type ProjectTree } from "../lib/project-files";

export type TreeNode =
  | { kind: "directory"; path: string; name: string; children: TreeNode[] }
  | { kind: "file"; path: string; name: string };

/**
 * Turns flat project paths into a nested tree with folders first, then files,
 * each level sorted by name. Directories that only exist implicitly, as the parent
 * of a file, still get a node.
 */
export function buildTree(files: string[], directories: string[]): TreeNode[] {
  const root: TreeNode[] = [];
  const folders = new Map<string, TreeNode[]>([["", root]]);

  const folder = (path: string): TreeNode[] => {
    const existing = folders.get(path);
    if (existing) return existing;
    const parent = folder(parentOf(path));
    const children: TreeNode[] = [];
    parent.push({ kind: "directory", path, name: baseName(path), children });
    folders.set(path, children);
    return children;
  };

  for (const directory of directories) folder(directory);
  for (const file of new Set(files)) folder(parentOf(file)).push({ kind: "file", path: file, name: baseName(file) });

  const sort = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => a.kind !== b.kind ? (a.kind === "directory" ? -1 : 1) : a.name.localeCompare(b.name));
    for (const node of nodes) if (node.kind === "directory") sort(node.children);
    return nodes;
  };
  return sort(root);
}

export function baseName(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function parentOf(path: string): string {
  const index = path.lastIndexOf("/");
  return index < 0 ? "" : path.slice(0, index);
}

/** Every folder that contains `path`, outermost first. */
export function ancestorsOf(path: string): string[] {
  const parts = path.split("/").slice(0, -1);
  return parts.map((_, index) => parts.slice(0, index + 1).join("/"));
}

/** Files at `path` or below it, which is what deleting `path` removes. */
export function filesUnder(files: Record<string, string>, path: string): string[] {
  return Object.keys(files).filter((name) => name === path || name.startsWith(`${path}/`));
}

/**
 * Moves a file or a folder, with everything inside it, to a new path. Uses the
 * same path rules as creating one, and refuses to overwrite anything.
 */
export function renamePath(tree: ProjectTree, fromInput: string, toInput: string): ProjectTree {
  const from = normalizeProjectPath(fromInput);
  const to = normalizeProjectPath(toInput);
  if (!from) throw new Error("Choose a file or folder to rename.");
  if (!to) throw new Error(invalidPathMessage);
  if (from === to) return tree;
  // Only the real protected files are pinned. A user file that merely differs
  // in case, such as App.js from older data, must stay movable out of the way.
  const reservedSource = [...protectedProjectPaths].find((reserved) => from === reserved || from.startsWith(`${reserved}/`));
  if (reservedSource) throw new Error(`${reservedSource} is a reserved project file and cannot be renamed or moved.`);
  requireUnreservedPath(to);

  const isFile = typeof tree.files[from] === "string";
  const isDirectory = tree.directories.includes(from) || Object.keys(tree.files).some((name) => name.startsWith(`${from}/`));
  if (!isFile && !isDirectory) throw new Error(`Nothing exists at ${from}.`);
  if (isDirectory && to.startsWith(`${from}/`)) throw new Error("A folder cannot move inside itself.");

  const moved = (name: string) => name === from ? to : name.startsWith(`${from}/`) ? `${to}${name.slice(from.length)}` : name;
  const staying = Object.keys(tree.files).filter((name) => moved(name) === name);
  const remaining = Object.fromEntries(staying.map((name) => [name, tree.files[name]]));
  // Folders that stay, whether listed or only implied by the files inside them.
  const stayingDirectories = new Set([
    ...tree.directories.filter((name) => moved(name) === name),
    ...directoriesForFiles(remaining),
  ]);

  // Compared without case: a case-insensitive filesystem writes App.js and app.js to one file.
  const takenFile = existingFile(remaining, to);
  if (takenFile) throw new Error(`A file already exists at ${takenFile}.`);
  const takenFolder = existingDirectory(stayingDirectories, to);
  if (takenFolder) throw new Error(`A folder already exists at ${takenFolder}.`);
  const blocking = fileAncestor(remaining, to);
  if (blocking) throw new Error(`${blocking} is a file, so it cannot contain ${to}.`);

  // Every moved path must land somewhere free, or two entries would collapse into one.
  const movedPaths = [
    ...Object.keys(tree.files).filter((name) => moved(name) !== name).map(moved),
    ...tree.directories.filter((name) => moved(name) !== name).map(moved),
  ];
  for (const path of movedPaths) {
    requireUnreservedPath(path);
    const taken = existingFile(remaining, path);
    if (taken) throw new Error(`A file already exists at ${taken}.`);
    const ancestor = fileAncestor(remaining, path);
    if (ancestor) throw new Error(`${ancestor} is a file, so it cannot contain ${path}.`);
  }
  const movedFiles = Object.keys(tree.files).filter((name) => moved(name) !== name).map(moved);
  for (const path of movedFiles) {
    const folder = existingDirectory(stayingDirectories, path);
    if (folder) throw new Error(`A folder already exists at ${folder}.`);
  }

  const files = Object.fromEntries(Object.entries(tree.files).map(([name, content]) => [moved(name), content]));
  const directories = new Set([
    ...tree.directories.map(moved),
    ...directoriesForFiles(files),
    ...(isDirectory ? [to, ...ancestorsOf(to)] : ancestorsOf(to)),
  ]);
  return { files, directories: [...directories].sort() };
}
