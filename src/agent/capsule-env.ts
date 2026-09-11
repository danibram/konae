import { ExecutionError, FileError, err, ok } from "@earendil-works/pi-agent-core";
import type { FileInfo, FileKind, ShellExecOptions } from "@earendil-works/pi-agent-core";
import type { ProjectFile } from "../lib/types";
import type {
  CapsuleFiles,
  CapsuleMutation,
  CapsuleSnapshot,
  CapsuleWorkspace,
  ExecutionEnv,
  Result,
} from "./contracts";

/** The Agent Workspace is mounted here; a Capsule has no other directory. */
export const capsuleRoot = "/workspace";

export const capsuleFileNames: readonly ProjectFile[] = [
  "index.html",
  "styles.css",
  "app.js",
  "schema.sql",
  "actions.json",
];

export const capsuleFilePaths: readonly string[] = capsuleFileNames.map((name) => `${capsuleRoot}/${name}`);

export function capsulePathFor(file: ProjectFile): string {
  return `${capsuleRoot}/${file}`;
}

/** Resolve an agent-supplied path to a safe file inside the Capsule, or null. */
export function capsuleFileFor(path: string): ProjectFile | null {
  const normalized = normalizePath(path);
  if (!normalized || !normalized.startsWith(`${capsuleRoot}/`)) return null;
  const name = normalized.slice(capsuleRoot.length + 1);
  return name && !name.endsWith("/") ? name : null;
}

export function createCapsuleWorkspace(seed: CapsuleFiles): CapsuleWorkspace {
  return new CapsuleEnvWorkspace(seed);
}

type Located = { path: string; file: ProjectFile | null; kind: FileKind | null };

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * An in-memory `ExecutionEnv` over a disposable copy of a Project's files.
 * Every write is a proposal: it stays in this workspace until the Validation
 * Gate clears it. `exec` is deliberately unavailable.
 */
class CapsuleEnvWorkspace implements CapsuleWorkspace {
  readonly env: ExecutionEnv;
  private readonly seed: CapsuleFiles;
  private readonly writeOrder: ProjectFile[] = [];
  private readonly listeners = new Set<(mutation: CapsuleMutation) => void>();
  private readonly mtimes = new Map<ProjectFile, number>();
  private current: CapsuleFiles;

  constructor(seed: CapsuleFiles) {
    this.seed = { ...seed };
    this.current = { ...seed };
    const created = Date.now();
    for (const name of Object.keys(seed)) this.mtimes.set(name, created);
    this.env = this.createEnv();
  }

  get files(): CapsuleFiles {
    return this.current;
  }

  onMutation(callback: (mutation: CapsuleMutation) => void): () => void {
    this.listeners.add(callback);
    return () => {
      this.listeners.delete(callback);
    };
  }

  changedFiles(): ProjectFile[] {
    return this.writeOrder.filter((file) => this.current[file] !== this.seed[file]);
  }

  serialize(): CapsuleSnapshot {
    return { files: { ...this.current }, changedFiles: this.changedFiles() };
  }

  private write(file: ProjectFile, content: string) {
    if (!this.writeOrder.includes(file)) this.writeOrder.push(file);
    this.current = { ...this.current, [file]: content };
    this.mtimes.set(file, Date.now());
    const mutation: CapsuleMutation = { file, files: this.current, changedFiles: this.changedFiles() };
    for (const listener of this.listeners) listener(mutation);
  }

  private locate(input: string): Located | null {
    const normalized = normalizePath(input);
    if (!normalized) return null;
    if (normalized === "/" || normalized === capsuleRoot) return { path: normalized, file: null, kind: "directory" };
    const file = capsuleFileFor(normalized);
    if (!file) return { path: normalized, file: null, kind: null };
    const kind = typeof this.current[file] === "string"
      ? "file"
      : Object.keys(this.current).some((candidate) => candidate.startsWith(`${file}/`)) ? "directory" : null;
    return { path: normalized, file: kind === "directory" ? null : file, kind };
  }

  private info(file: ProjectFile): FileInfo {
    return {
      name: file.split("/").at(-1) ?? file,
      path: capsulePathFor(file),
      kind: "file",
      size: encoder.encode(this.current[file] ?? "").byteLength,
      mtimeMs: this.mtimes.get(file) ?? 0,
    };
  }

  private createEnv(): ExecutionEnv {
    const workspace = this;

    const invalid = <T>(path: string) =>
      err<T, FileError>(new FileError("invalid", "A path is required.", path));
    const missing = <T>(path: string) =>
      err<T, FileError>(new FileError("not_found", `No such file in the Agent Workspace: ${path}`, path));
    const stopped = <T>(signal?: AbortSignal) =>
      signal?.aborted ? err<T, FileError>(new FileError("aborted", "The workspace operation was aborted.")) : null;
    const unsupported = <T>(message: string, path?: string) =>
      err<T, FileError>(new FileError("not_supported", message, path));

    /** Locate a path that must already exist. The error branch is assignable to any `Result`. */
    const locateFile = (path: string, signal?: AbortSignal): Result<ProjectFile, FileError> => {
      const stop = stopped<ProjectFile>(signal);
      if (stop) return stop;
      const located = workspace.locate(path);
      if (!located) return invalid<ProjectFile>(path);
      if (located.kind === "directory") {
        return err<ProjectFile, FileError>(
          new FileError("is_directory", `${located.path} is a directory.`, located.path),
        );
      }
      if (!located.file) return missing<ProjectFile>(located.path);
      // A Capsule record hand-edited or persisted before a file existed has no such key.
      if (typeof workspace.current[located.file] !== "string") return missing<ProjectFile>(located.path);
      return ok<ProjectFile, FileError>(located.file);
    };

    return {
      cwd: capsuleRoot,

      async absolutePath(path, abortSignal) {
        const stop = stopped<string>(abortSignal);
        if (stop) return stop;
        const normalized = normalizePath(path);
        return normalized ? ok<string, FileError>(normalized) : invalid<string>(path);
      },

      async joinPath(parts, abortSignal) {
        const stop = stopped<string>(abortSignal);
        if (stop) return stop;
        const usable = parts.filter((part) => part.length > 0);
        if (usable.length === 0) return invalid<string>(parts.join("/"));
        const joined = usable.join("/");
        const normalized = normalizePath(joined);
        return normalized ? ok<string, FileError>(normalized) : invalid<string>(joined);
      },

      async readTextFile(path, abortSignal) {
        const located = locateFile(path, abortSignal);
        if (!located.ok) return located;
        return ok<string, FileError>(workspace.current[located.value]);
      },

      async readTextLines(path, options) {
        const located = locateFile(path, options?.abortSignal);
        if (!located.ok) return located;
        const lines = workspace.current[located.value].split("\n");
        const maxLines = options?.maxLines;
        return ok<string[], FileError>(maxLines === undefined ? lines : lines.slice(0, Math.max(maxLines, 0)));
      },

      async readBinaryFile(path, abortSignal) {
        const located = locateFile(path, abortSignal);
        if (!located.ok) return located;
        return ok<Uint8Array, FileError>(encoder.encode(workspace.current[located.value]));
      },

      async writeFile(path, content, abortSignal) {
        const stop = stopped<void>(abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<void>(path);
        if (!located.file) return unsupported<void>(`${located.path} is a directory.`, located.path);
        workspace.write(located.file, asText(content));
        return ok<void, FileError>(undefined);
      },

      async appendFile(path, content, abortSignal) {
        const stop = stopped<void>(abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<void>(path);
        if (!located.file) return unsupported<void>(`${located.path} is a directory.`, located.path);
        workspace.write(located.file, (workspace.current[located.file] ?? "") + asText(content));
        return ok<void, FileError>(undefined);
      },

      async renameFile(sourcePath, _destinationPath, abortSignal) {
        const stop = stopped<void>(abortSignal);
        if (stop) return stop;
        return unsupported<void>("The agent tools do not rename files. Create the destination with write instead.", sourcePath);
      },

      async fileInfo(path, abortSignal) {
        const stop = stopped<FileInfo>(abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<FileInfo>(path);
        if (located.kind === "file" && located.file) return ok<FileInfo, FileError>(workspace.info(located.file));
        if (located.kind === "directory") {
          return ok<FileInfo, FileError>({
            name: located.path === "/" ? "/" : located.path.slice(1),
            path: located.path,
            kind: "directory",
            size: 0,
            mtimeMs: 0,
          });
        }
        return missing<FileInfo>(located.path);
      },

      async listDir(path, abortSignal) {
        const stop = stopped<FileInfo[]>(abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<FileInfo[]>(path);
        if (located.file) {
          return err<FileInfo[], FileError>(
            new FileError("not_directory", `${located.path} is a file.`, located.path),
          );
        }
        if (located.path === "/") {
          return ok<FileInfo[], FileError>([
            { name: capsuleRoot.slice(1), path: capsuleRoot, kind: "directory", size: 0, mtimeMs: 0 },
          ]);
        }
        if (located.path === capsuleRoot) {
          return ok<FileInfo[], FileError>(workspace.listEntries(""));
        }
        const directory = located.path.slice(capsuleRoot.length + 1);
        if (located.kind === "directory") return ok<FileInfo[], FileError>(workspace.listEntries(directory));
        return missing<FileInfo[]>(located.path);
      },

      async canonicalPath(path, abortSignal) {
        const stop = stopped<string>(abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<string>(path);
        if (located.kind === "file" || located.kind === "directory") return ok<string, FileError>(located.path);
        return missing<string>(located.path);
      },

      async exists(path, abortSignal) {
        const stop = stopped<boolean>(abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<boolean>(path);
        return ok<boolean, FileError>((Boolean(located.file) && typeof workspace.current[located.file!] === "string") || located.kind === "directory");
      },

      async createDir(path, options) {
        const stop = stopped<void>(options?.abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<void>(path);
        if (located.kind === "directory") return ok<void, FileError>(undefined);
        return located.path.startsWith(`${capsuleRoot}/`)
          ? ok<void, FileError>(undefined)
          : unsupported<void>("Directories must stay inside /workspace.", located.path);
      },

      async remove(path, options) {
        const stop = stopped<void>(options?.abortSignal);
        if (stop) return stop;
        const located = workspace.locate(path);
        if (!located) return invalid<void>(path);
        if (located.kind === "file" || located.kind === "directory") {
          return unsupported<void>("The agent tools do not remove files.", located.path);
        }
        if (options?.force) return ok<void, FileError>(undefined);
        return missing<void>(located.path);
      },

      async createTempDir(_prefix, abortSignal) {
        const stop = stopped<string>(abortSignal);
        if (stop) return stop;
        return unsupported<string>("The Agent Workspace has no temporary storage.");
      },

      async createTempFile(options) {
        const stop = stopped<string>(options?.abortSignal);
        if (stop) return stop;
        return unsupported<string>("The Agent Workspace has no temporary storage.");
      },

      async exec(_command: string, options?: ShellExecOptions) {
        if (options?.abortSignal?.aborted) {
          return err<{ stdout: string; stderr: string; exitCode: number }, ExecutionError>(
            new ExecutionError("aborted", "The command was aborted."),
          );
        }
        return err<{ stdout: string; stderr: string; exitCode: number }, ExecutionError>(
          new ExecutionError(
            "shell_unavailable",
            "There is no shell in Littlebox. Edit the Capsule files and run validate_capsule instead.",
          ),
        );
      },

      async cleanup() {},
    };
  }

  private listEntries(directory: string): FileInfo[] {
    const prefix = directory ? `${directory}/` : "";
    const children = new Map<string, FileInfo>();
    for (const file of Object.keys(this.current)) {
      if (!file.startsWith(prefix)) continue;
      const remainder = file.slice(prefix.length);
      const [child, ...rest] = remainder.split("/");
      if (!child) continue;
      const childPath = `${prefix}${child}`;
      if (rest.length === 0) children.set(child, this.info(childPath));
      else if (!children.has(child)) children.set(child, {
        name: child,
        path: capsulePathFor(childPath),
        kind: "directory",
        size: 0,
        mtimeMs: 0,
      });
    }
    return [...children.values()].sort((left, right) => left.name.localeCompare(right.name));
  }
}

function asText(content: string | Uint8Array): string {
  return typeof content === "string" ? content : decoder.decode(content);
}

/** Normalize a Capsule path. Relative paths resolve against the workspace root; returns null when unusable. */
function normalizePath(input: string): string | null {
  if (typeof input !== "string" || !input.trim()) return null;
  const absolute = input.startsWith("/") ? input : `${capsuleRoot}/${input}`;
  const segments: string[] = [];
  for (const segment of absolute.split("/")) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      segments.pop();
      continue;
    }
    segments.push(segment);
  }
  return `/${segments.join("/")}`;
}
