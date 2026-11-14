import { useEffect, useState } from "react";

/**
 * The digest of an app's sources, the same one the Kernel stores on the
 * Deployment as `sourceDigest` (kernel-rs/kernel/src/sources.rs `digest`).
 * SHA-256 in lowercase hex over the files sorted by the UTF-8 bytes of their
 * path. Each file adds its path, a NUL, its length in UTF-8 bytes as decimal
 * digits, a NUL and its bytes. kernel-rs/kernel/src/source-digest-cases.json
 * holds the cases both sides run.
 */
export async function sourceDigest(files: Record<string, string>): Promise<string> {
  const encoder = new TextEncoder();
  const entries = Object.entries(files).map(([path, text]) => [encoder.encode(path), encoder.encode(text)] as const);
  entries.sort(([a], [b]) => compareBytes(a, b));
  const parts: Uint8Array[] = [];
  for (const [path, bytes] of entries) parts.push(path, nul, encoder.encode(String(bytes.length)), nul, bytes);
  const all = new Uint8Array(parts.reduce((size, part) => size + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    all.set(part, offset);
    offset += part.length;
  }
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", all));
  return Array.from(hash, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

const nul = new Uint8Array([0]);

function compareBytes(a: Uint8Array, b: Uint8Array) {
  const length = Math.min(a.length, b.length);
  for (let index = 0; index < length; index++) if (a[index] !== b[index]) return a[index]! - b[index]!;
  return a.length - b.length;
}

// One digest per files object. The editor replaces the object on every edit, so a stale entry is never read.
const digests = new WeakMap<Record<string, string>, Promise<string>>();

export function cachedSourceDigest(files: Record<string, string>): Promise<string> {
  let digest = digests.get(files);
  if (!digest) {
    digest = sourceDigest(files);
    digests.set(files, digest);
  }
  return digest;
}

/** The digest of `files`, or null until it is known. */
export function useSourceDigest(files: Record<string, string> | null | undefined): string | null {
  const [known, setKnown] = useState<{ files: Record<string, string>; digest: string } | null>(null);
  useEffect(() => {
    if (!files) return;
    let current = true;
    cachedSourceDigest(files).then((digest) => { if (current) setKnown({ files, digest }); }, () => {});
    return () => { current = false; };
  }, [files]);
  return files && known?.files === files ? known.digest : null;
}

/** The digest of each project's files by project ID, filled in as each one is known. */
export function useSourceDigests(projects: { id: string; files: Record<string, string> }[]): ReadonlyMap<string, string> {
  const [known, setKnown] = useState<ReadonlyMap<string, { files: Record<string, string>; digest: string }>>(new Map());
  useEffect(() => {
    let current = true;
    void Promise.all(projects.map(async (project) => [project.id, { files: project.files, digest: await cachedSourceDigest(project.files) }] as const))
      .then((entries) => { if (current) setKnown(new Map(entries)); }, () => {});
    return () => { current = false; };
  }, [projects]);
  const result = new Map<string, string>();
  for (const project of projects) {
    const entry = known.get(project.id);
    if (entry?.files === project.files) result.set(project.id, entry.digest);
  }
  return result;
}
