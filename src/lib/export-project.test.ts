import { expect, test } from "bun:test";
import { SETTINGS_FILE } from "./app-folder";
import { createProjectExport } from "./export-project";
import { createTar, readTar } from "./tar";
import type { Transfer } from "../mcp/client";

const settings = { id: "app_1", name: "Notes", slug: "notes", visibility: "private", signupPolicy: { mode: "closed", allowlist: [] }, signInMethods: ["password"], kernel: "0.7.0", manifest: {} };
const folder = createTar([{ path: SETTINGS_FILE, contents: JSON.stringify(settings) }, { path: "app/actions.json", contents: JSON.stringify({ version: 1, actions: {} }) }]);
const mcp = {
  call: async <T>() => ({ manifest: { contents: [] }, downloads: {}, folder: { transferId: "folder" }, sourcesAhead: false }) as T,
  download: async (_transfer: Transfer) => folder,
  cancel: async () => ({}),
} as never;
const draft = { id: "app_1", name: "Notes", slug: "notes", files: { "index.html": "<!doctype html><img src=missing.png>" }, directories: [], manifest: { visibility: "private" }, updatedAt: 0 } as never;

test("a published app exports its folder even when the editor's draft does not build", async () => {
  let builds = 0;
  const build = async () => { builds += 1; throw Error("index.html references missing.png, but the Capsule holds no missing.png."); };
  const result = await createProjectExport(draft, "app_1", { mcp, build });
  expect(builds).toBe(0);
  const paths = readTar(new Uint8Array(await result.blob.arrayBuffer())).map((entry) => entry.path);
  expect(paths).toContain(`notes/${SETTINGS_FILE}`);
});

test("a draft is built for its export, and a draft that does not build says why", async () => {
  const build = async () => { throw Error("index.html references missing.png, but the Capsule holds no missing.png."); };
  await expect(createProjectExport(draft, undefined, { mcp, build })).rejects.toThrow("missing.png");
});
