import { decodeBase64, encodeBase64 } from "../mcp/client";
import type { MiniAppProject, ProjectFile } from "./types";

export type SourceFiles = Pick<MiniAppProject, "files" | "encodings">;

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

export function utf8Text(bytes: Uint8Array): string | null {
  try { return utf8.decode(bytes); } catch { return null; }
}

export function sourceFilesFrom(entries: Iterable<readonly [ProjectFile, Uint8Array]>): SourceFiles {
  const files: Record<ProjectFile, string> = {};
  const encodings: Record<ProjectFile, "base64"> = {};
  for (const [path, bytes] of entries) {
    const text = utf8Text(bytes);
    if (text === null) { files[path] = encodeBase64(bytes); encodings[path] = "base64"; } else files[path] = text;
  }
  return Object.keys(encodings).length ? { files, encodings } : { files };
}

export function isEncoded(source: SourceFiles, path: ProjectFile): boolean {
  return source.encodings?.[path] === "base64" && path in source.files;
}

export function sourceBytes(source: SourceFiles, path: ProjectFile): Uint8Array {
  const contents = source.files[path] ?? "";
  return isEncoded(source, path) ? decodeBase64(contents) : new TextEncoder().encode(contents);
}

export function sourceContents(source: SourceFiles, path: ProjectFile): string | Uint8Array {
  return isEncoded(source, path) ? decodeBase64(source.files[path]!) : source.files[path]!;
}

export function textFiles(source: SourceFiles): Record<ProjectFile, string> {
  if (!source.encodings) return source.files;
  return Object.fromEntries(Object.entries(source.files).filter(([path]) => !isEncoded(source, path)));
}

export function keptEncodings(
  encodings: SourceFiles["encodings"],
  files: Record<ProjectFile, string>,
  { rename = (path: ProjectFile) => path, replaced = [] }: { rename?: (path: ProjectFile) => ProjectFile; replaced?: readonly ProjectFile[] } = {},
): SourceFiles["encodings"] {
  if (!encodings) return undefined;
  const kept = Object.fromEntries(Object.keys(encodings).map(rename).filter((path) => path in files && !replaced.includes(path)).map((path) => [path, "base64" as const]));
  return Object.keys(kept).length ? kept : undefined;
}
