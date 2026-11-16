import { refusal } from "../kernel/refusal";
import { byteOrder, carriedSizes, largestFiles, sizeText } from "./size";

export const MAX_SOURCE_FILES = 1_000;
export const MAX_SOURCE_BYTES = 20 * 1024 * 1024;
export const MAX_SOURCE_PATH_BYTES = 240;

const edgeSpace = /^\p{White_Space}|\p{White_Space}$/u;
const control = /[\u0000-\u001f\u007f-\u009f]/;

export function sourcePathHeld(path: string): boolean {
  return new TextEncoder().encode(path).length <= MAX_SOURCE_PATH_BYTES
    && !edgeSpace.test(path)
    && !control.test(path)
    && !path.includes("\\")
    && path.split("/").every((part) => part !== "" && !part.startsWith(".") && part !== "node_modules");
}

export function skippedName(name: string): boolean {
  return name.startsWith(".") || name === "node_modules";
}

export function checkSources(files: Iterable<readonly [string, number]>): void {
  const sorted = [...files].sort((a, b) => byteOrder(a[0], b[0]));
  if (sorted.length > MAX_SOURCE_FILES) throw refusal("sources.file-count", `app/ holds ${sorted.length} files. A project's sources hold at most ${MAX_SOURCE_FILES} files.`);
  const bad = sorted.find(([path]) => !sourcePathHeld(path));
  if (bad) throw refusal("sources.path-invalid", `The sources cannot hold the path ${JSON.stringify(bad[0])}. A path inside app/ is at most ${MAX_SOURCE_PATH_BYTES} bytes, with no control character, no space at either end, no backslash, no empty part, and no part that starts with a dot or is node_modules.`);
  const total = sorted.reduce((sum, [, bytes]) => sum + bytes, 0);
  if (total > MAX_SOURCE_BYTES) throw refusal("sources.size", `app/ holds ${sizeText(total)}. A project's sources hold at most 20 MiB. ${largestFiles(sorted)}`);
}

export function checkSourceFiles(source: { files: Record<string, string>; encodings?: Record<string, string> }): void {
  checkSources(carriedSizes(source));
}
