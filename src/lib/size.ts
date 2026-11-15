// The size limits a person meets when an app travels to the installation,
// and the sentences that name them. Each sentence names the size sent and the
// limit. kernel-rs/kernel/src/size.rs is the Kernel's twin, and the `sizes` and
// `sentences` of kernel-rs/kernel/src/sources-cases.json are the cases both run.

/** The most bytes a Build Artifact holds once base64 is decoded. */
export const ARTIFACT_BYTES = 16 * 1024 * 1024;
/** The most bytes of JSON one release payload holds. */
export const RELEASE_BYTES = 26_000_000;
/** The most bytes one owner upload holds when it names no component of an export. */
export const UPLOAD_BYTES = 32 * 1024 * 1024;

const ceilDiv = (a: number, b: number) => Math.floor((a + b - 1) / b);

/** `bytes` for a person: whole bytes under 1 KiB, else KiB or MiB with one decimal, rounded up so a size over a limit never reads as the limit. */
export function sizeText(bytes: number): string {
  if (bytes === 1) return "1 byte";
  if (bytes < 1024) return `${bytes} bytes`;
  const kib = ceilDiv(bytes * 10, 1024);
  if (kib < 10 * 1024) return `${Math.floor(kib / 10)}.${kib % 10} KiB`;
  const mib = ceilDiv(bytes * 10, 1024 * 1024);
  return `${Math.floor(mib / 10)}.${mib % 10} MiB`;
}

/** Orders two paths by their UTF-8 bytes, as Rust orders strings. */
export function byteOrder(a: string, b: string): number {
  const left = [...a];
  const right = [...b];
  for (let index = 0; index < Math.min(left.length, right.length); index++) {
    const difference = left[index].codePointAt(0)! - right[index].codePointAt(0)!;
    if (difference !== 0) return difference;
  }
  return left.length - right.length;
}

/** "The largest files are a (x), b (y) and c (z).": the three largest by size, a tie in the byte order of their paths. */
export function largestFiles(files: Iterable<readonly [string, number]>): string {
  const named = [...files].sort((a, b) => b[1] - a[1] || byteOrder(a[0], b[0])).slice(0, 3).map(([path, bytes]) => `${path} (${sizeText(bytes)})`);
  if (named.length === 0) return "";
  if (named.length === 1) return `The largest file is ${named[0]}.`;
  if (named.length === 2) return `The largest files are ${named[0]} and ${named[1]}.`;
  return `The largest files are ${named[0]}, ${named[1]} and ${named[2]}.`;
}

/** The refusal of a Build Artifact over {@link ARTIFACT_BYTES}, from the decoded size of each file. */
export function artifactRefused(files: readonly (readonly [string, number])[]): string {
  const total = files.reduce((sum, [, bytes]) => sum + bytes, 0);
  return `dist/ holds ${sizeText(total)}. A Build Artifact holds at most 16 MiB. ${largestFiles(files)}`;
}

/** The refusal of a release payload over {@link RELEASE_BYTES}. */
export function releaseRefused(bytes: number): string {
  const tenths = ceilDiv(bytes * 10, 1_000_000);
  return `The release is ${Math.floor(tenths / 10)}.${tenths % 10} MB of JSON. A release holds at most 26 MB.`;
}

/** The refusal of an owner upload that names no component, outside 1 byte to {@link UPLOAD_BYTES}. */
export function uploadRefused(bytes: number): string {
  return `The upload is ${sizeText(bytes)}. An upload holds 1 byte to 32 MiB.`;
}

/** The decoded size of a file a document carries: base64 when `encoded`, else UTF-8 text. */
export function carriedBytes(contents: string, encoded: boolean): number {
  if (encoded) return Math.floor(contents.length / 4) * 3 - (contents.endsWith("==") ? 2 : contents.endsWith("=") ? 1 : 0);
  let bytes = 0;
  for (let index = 0; index < contents.length; index++) {
    const code = contents.charCodeAt(index);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code < 0xdc00 && (contents.charCodeAt(index + 1) & 0xfc00) === 0xdc00) { bytes += 4; index++; }
    else bytes += 3;
  }
  return bytes;
}

/** The decoded size of each file of a document that names its base64 files in `encodings`, as `files` of a source document or a Build Artifact. */
export function carriedSizes(document: { files: Record<string, string>; encodings?: Record<string, string> }): [string, number][] {
  // A file the editor removed may stay as undefined until JSON leaves it out.
  return Object.entries(document.files).filter(([, contents]) => typeof contents === "string").map(([path, contents]) => [path, carriedBytes(contents, document.encodings?.[path] === "base64")]);
}

/** Refuses a Build Artifact over {@link ARTIFACT_BYTES} with the Kernel's sentence. */
export function checkArtifact(artifact: { files: Record<string, string>; encodings?: Record<string, string> }): void {
  const sizes = carriedSizes(artifact);
  if (sizes.reduce((sum, [, bytes]) => sum + bytes, 0) > ARTIFACT_BYTES) throw new Error(artifactRefused(sizes));
}

/** Refuses a release payload over {@link RELEASE_BYTES} with the Kernel's sentence. */
export function checkRelease(bytes: Uint8Array): void {
  if (bytes.length > RELEASE_BYTES) throw new Error(releaseRefused(bytes.length));
}
