// The rules the sources of a project meet, from each file's path and decoded
// size. app/ is the Capsule, so these are Capsule refusals with codes, and
// kernel-rs/kernel/src/sources.rs `check` refuses with the same code and words
// on save, on release and on import. The CLI, the editor's save and its
// import run them before a byte travels. The `cases` of
// kernel-rs/kernel/src/sources-cases.json hold what both answer.
import { refusal } from "../kernel/refusal";
import { byteOrder, carriedSizes, largestFiles, sizeText } from "./size";

/** The most files the sources of a project hold. */
export const MAX_SOURCE_FILES = 1_000;
/** The most bytes every file of `app/` holds together, decoded. As base64 in the JSON of a save or a release, they fit one 32 MiB upload. */
export const MAX_SOURCE_BYTES = 20 * 1024 * 1024;
/** The longest path inside `app/`, in UTF-8 bytes. */
export const MAX_SOURCE_PATH_BYTES = 240;

const edgeSpace = /^\p{White_Space}|\p{White_Space}$/u;
const control = /[\u0000-\u001f\u007f-\u009f]/;

/** Whether `path` names a file the sources may hold. The folder readers skip a part that starts with a dot or is `node_modules`, so the rule refuses it. */
export function sourcePathHeld(path: string): boolean {
  return new TextEncoder().encode(path).length <= MAX_SOURCE_PATH_BYTES
    && !edgeSpace.test(path)
    && !control.test(path)
    && !path.includes("\\")
    && path.split("/").every((part) => part !== "" && !part.startsWith(".") && part !== "node_modules");
}

/** Whether a folder reader skips an entry of `app/` by its name: a dot entry or `node_modules`, at any depth. */
export function skippedName(name: string): boolean {
  return name.startsWith(".") || name === "node_modules";
}

/** Refuses sources that break a rule: the file count, then each path in byte order, then the total. */
export function checkSources(files: Iterable<readonly [string, number]>): void {
  const sorted = [...files].sort((a, b) => byteOrder(a[0], b[0]));
  if (sorted.length > MAX_SOURCE_FILES) throw refusal("sources.file-count", `app/ holds ${sorted.length} files. A project's sources hold at most ${MAX_SOURCE_FILES} files.`);
  const bad = sorted.find(([path]) => !sourcePathHeld(path));
  if (bad) throw refusal("sources.path-invalid", `The sources cannot hold the path ${JSON.stringify(bad[0])}. A path inside app/ is at most ${MAX_SOURCE_PATH_BYTES} bytes, with no control character, no space at either end, no backslash, no empty part, and no part that starts with a dot or is node_modules.`);
  const total = sorted.reduce((sum, [, bytes]) => sum + bytes, 0);
  if (total > MAX_SOURCE_BYTES) throw refusal("sources.size", `app/ holds ${sizeText(total)}. A project's sources hold at most 20 MiB. ${largestFiles(sorted)}`);
}

/** {@link checkSources} on a source document's `files` and `encodings`. */
export function checkSourceFiles(source: { files: Record<string, string>; encodings?: Record<string, string> }): void {
  checkSources(carriedSizes(source));
}
