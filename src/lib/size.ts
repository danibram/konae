
export const ARTIFACT_BYTES = 16 * 1024 * 1024;
export const RELEASE_BYTES = 26_000_000;
export const UPLOAD_BYTES = 32 * 1024 * 1024;

const ceilDiv = (a: number, b: number) => Math.floor((a + b - 1) / b);

export function sizeText(bytes: number): string {
  if (bytes === 1) return "1 byte";
  if (bytes < 1024) return `${bytes} bytes`;
  const kib = ceilDiv(bytes * 10, 1024);
  if (kib < 10 * 1024) return `${Math.floor(kib / 10)}.${kib % 10} KiB`;
  const mib = ceilDiv(bytes * 10, 1024 * 1024);
  return `${Math.floor(mib / 10)}.${mib % 10} MiB`;
}

export function byteOrder(a: string, b: string): number {
  const left = [...a];
  const right = [...b];
  for (let index = 0; index < Math.min(left.length, right.length); index++) {
    const difference = left[index].codePointAt(0)! - right[index].codePointAt(0)!;
    if (difference !== 0) return difference;
  }
  return left.length - right.length;
}

export function largestFiles(files: Iterable<readonly [string, number]>): string {
  const named = [...files].sort((a, b) => b[1] - a[1] || byteOrder(a[0], b[0])).slice(0, 3).map(([path, bytes]) => `${path} (${sizeText(bytes)})`);
  if (named.length === 0) return "";
  if (named.length === 1) return `The largest file is ${named[0]}.`;
  if (named.length === 2) return `The largest files are ${named[0]} and ${named[1]}.`;
  return `The largest files are ${named[0]}, ${named[1]} and ${named[2]}.`;
}

export function artifactRefused(files: readonly (readonly [string, number])[]): string {
  const total = files.reduce((sum, [, bytes]) => sum + bytes, 0);
  return `dist/ holds ${sizeText(total)}. A Build Artifact holds at most 16 MiB. ${largestFiles(files)}`;
}

export function releaseRefused(bytes: number): string {
  const tenths = ceilDiv(bytes * 10, 1_000_000);
  return `The release is ${Math.floor(tenths / 10)}.${tenths % 10} MB of JSON. A release holds at most 26 MB.`;
}

export function uploadRefused(bytes: number): string {
  return `The upload is ${sizeText(bytes)}. An upload holds 1 byte to 32 MiB.`;
}

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

export function carriedSizes(document: { files: Record<string, string>; encodings?: Record<string, string> }): [string, number][] {
  return Object.entries(document.files).filter(([, contents]) => typeof contents === "string").map(([path, contents]) => [path, carriedBytes(contents, document.encodings?.[path] === "base64")]);
}

export function checkArtifact(artifact: { files: Record<string, string>; encodings?: Record<string, string> }): void {
  const sizes = carriedSizes(artifact);
  if (sizes.reduce((sum, [, bytes]) => sum + bytes, 0) > ARTIFACT_BYTES) throw new Error(artifactRefused(sizes));
}

export function checkRelease(bytes: Uint8Array): void {
  if (bytes.length > RELEASE_BYTES) throw new Error(releaseRefused(bytes.length));
}

export function checkSourceUpload(bytes: number): void {
  if (bytes > UPLOAD_BYTES) throw new Error(uploadRefused(bytes));
}
