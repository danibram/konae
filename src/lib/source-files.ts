// The files of `app/` as a source document carries them: a file whose bytes
// are UTF-8 is its text, and any other file, such as an image, is base64 that
// `encodings` names, as in a Build Artifact. The CLI, the editor's import and
// the Kernel's `sources::read` all decide by the same rule.
import { decodeBase64, encodeBase64 } from "../mcp/client";
import type { MiniAppProject, ProjectFile } from "./types";

export type SourceFiles = Pick<MiniAppProject, "files" | "encodings">;

// ignoreBOM keeps a leading byte order mark in the text, so the text is the file's bytes.
const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** The text of bytes that are UTF-8, or null for any other bytes. */
export function utf8Text(bytes: Uint8Array): string | null {
  try { return utf8.decode(bytes); } catch { return null; }
}

/** Files and encodings from each file's bytes. `encodings` is absent when every file is text. */
export function sourceFilesFrom(entries: Iterable<readonly [ProjectFile, Uint8Array]>): SourceFiles {
  const files: Record<ProjectFile, string> = {};
  const encodings: Record<ProjectFile, "base64"> = {};
  for (const [path, bytes] of entries) {
    const text = utf8Text(bytes);
    if (text === null) { files[path] = encodeBase64(bytes); encodings[path] = "base64"; } else files[path] = text;
  }
  return Object.keys(encodings).length ? { files, encodings } : { files };
}

/** True when the file is not text, so the code pane and the build agent leave it as it is. */
export function isEncoded(source: SourceFiles, path: ProjectFile): boolean {
  return source.encodings?.[path] === "base64" && path in source.files;
}

/** One file as its bytes. */
export function sourceBytes(source: SourceFiles, path: ProjectFile): Uint8Array {
  const contents = source.files[path] ?? "";
  return isEncoded(source, path) ? decodeBase64(contents) : new TextEncoder().encode(contents);
}

/** One file as the static publishing contract reads it: text, or bytes for a file that is not text. */
export function sourceContents(source: SourceFiles, path: ProjectFile): string | Uint8Array {
  return isEncoded(source, path) ? decodeBase64(source.files[path]!) : source.files[path]!;
}

/** The text files only: what the code pane edits, the build agent reads and esbuild bundles. */
export function textFiles(source: SourceFiles): Record<ProjectFile, string> {
  if (!source.encodings) return source.files;
  return Object.fromEntries(Object.entries(source.files).filter(([path]) => !isEncoded(source, path)));
}

/**
 * The encodings that still name a file of `files`, after `rename` moves each
 * path and `replaced` lists paths that now hold text. Undefined when none is left.
 */
export function keptEncodings(
  encodings: SourceFiles["encodings"],
  files: Record<ProjectFile, string>,
  { rename = (path: ProjectFile) => path, replaced = [] }: { rename?: (path: ProjectFile) => ProjectFile; replaced?: readonly ProjectFile[] } = {},
): SourceFiles["encodings"] {
  if (!encodings) return undefined;
  const kept = Object.fromEntries(Object.keys(encodings).map(rename).filter((path) => path in files && !replaced.includes(path)).map((path) => [path, "base64" as const]));
  return Object.keys(kept).length ? kept : undefined;
}
