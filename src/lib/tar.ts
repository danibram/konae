export type TarEntry = {
  path: string;
  contents: string | Uint8Array;
  mode?: number;
};

const blockSize = 512;
const encoder = new TextEncoder();

export function createTar(entries: readonly TarEntry[]): Uint8Array {
  const chunks: Uint8Array[] = [];
  for (const entry of entries) {
    const path = normalizeArchivePath(entry.path);
    const contents = typeof entry.contents === "string" ? encoder.encode(entry.contents) : entry.contents;
    const header = tarHeader(path, contents.length, entry.mode ?? 0o644);
    chunks.push(header, contents, new Uint8Array(tarPadding(contents.length)));
  }
  chunks.push(new Uint8Array(blockSize * 2));
  return concatenate(chunks);
}

export function normalizeArchivePath(value: string): string {
  const path = value.replaceAll("\\", "/");
  const parts = path.split("/");
  if (!path || /[\0-\x1f]/.test(path) || path.startsWith("/") || /^[A-Za-z]:/.test(path) || parts.some((part) => !part || part === "." || part === "..")) {
    throw new Error(`Invalid archive path: ${value}`);
  }
  return path;
}

/** One ustar header block. Writers that stream file contents from disk use it with `tarPadding`. */
export function tarHeader(path: string, size: number, mode: number): Uint8Array {
  const header = new Uint8Array(blockSize);
  const split = splitUstarPath(path);
  writeString(header, 0, 100, split.name);
  writeOctal(header, 100, 8, mode);
  writeOctal(header, 108, 8, 0);
  writeOctal(header, 116, 8, 0);
  writeOctal(header, 124, 12, size);
  writeOctal(header, 136, 12, 0);
  header.fill(32, 148, 156);
  header[156] = 48;
  writeString(header, 257, 6, "ustar\0");
  writeString(header, 263, 2, "00");
  if (split.prefix) writeString(header, 345, 155, split.prefix);
  const checksum = header.reduce((sum, byte) => sum + byte, 0);
  writeOctal(header, 148, 8, checksum);
  return header;
}

function splitUstarPath(path: string): { name: string; prefix?: string } {
  if (encoder.encode(path).length <= 100) return { name: path };
  const segments = path.split("/");
  for (let index = segments.length - 1; index > 0; index -= 1) {
    const name = segments.slice(index).join("/");
    const prefix = segments.slice(0, index).join("/");
    if (encoder.encode(name).length <= 100 && encoder.encode(prefix).length <= 155) return { name, prefix };
  }
  throw new Error(`Archive path is too long: ${path}`);
}

function writeString(target: Uint8Array, offset: number, width: number, value: string) {
  const encoded = encoder.encode(value);
  if (encoded.length > width) throw new Error("Tar field is too long.");
  target.set(encoded, offset);
}

function writeOctal(target: Uint8Array, offset: number, width: number, value: number) {
  const digits = Math.trunc(value).toString(8).padStart(width - 1, "0");
  if (digits.length >= width) throw new Error("Tar numeric field is too large.");
  writeString(target, offset, width, `${digits}\0`);
}

export function tarPadding(size: number) {
  return (blockSize - (size % blockSize)) % blockSize;
}

function concatenate(chunks: readonly Uint8Array[]): Uint8Array {
  const result = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}
