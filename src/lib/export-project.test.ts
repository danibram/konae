import { expect, test } from "bun:test";
import { SETTINGS_FILE } from "./app-folder";
import { createProjectExport } from "./export-project";
import { createTar, readTar } from "./tar";
import { encodeBase64, type Transfer } from "../mcp/client";

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

test("a draft's export writes a file that is not text as its bytes", async () => {
  const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0, 0xff]);
  const withLogo = { id: "app_1", name: "Notes", slug: "notes", files: { "index.html": "<main></main>", "actions.json": JSON.stringify({ version: 1, actions: {} }), "public/logo.png": encodeBase64(png) }, encodings: { "public/logo.png": "base64" }, directories: ["public"], manifest: { visibility: "private" }, updatedAt: 0 } as never;
  const build = async () => ({ version: 1, entrypoint: "index.html", files: { "index.html": "<main></main>", "logo.png": encodeBase64(png) }, encodings: { "logo.png": "base64" } }) as never;
  const result = await createProjectExport(withLogo, undefined, { mcp, build });
  const entries = new Map(readTar(new Uint8Array(await result.blob.arrayBuffer())).map((entry) => [entry.path, entry.contents]));
  expect(entries.get("notes/app/public/logo.png")).toEqual(png);
  expect(entries.get("notes/dist/logo.png")).toEqual(png);
});

test("a draft export writes the full module state into konae.json", async () => {
  const withActions = { ...draft, modules: { seo: { on: true, canonical: "path", image: "social.png" } }, files: { ...draft.files, "actions.json": JSON.stringify({ version: 1, actions: {} }) } } as never;
  const result = await createProjectExport(withActions, undefined, {
    mcp,
    build: async () => ({ version: 1, entrypoint: "index.html", files: { "index.html": "<main></main>" } }) as never,
  });
  const entry = readTar(new Uint8Array(await result.blob.arrayBuffer())).find((item) => item.path === `notes/${SETTINGS_FILE}`)!;
  const exported = JSON.parse(new TextDecoder().decode(entry.contents));
  expect(Object.keys(exported.modules).sort()).toEqual(["seo"]);
  expect(exported.modules.seo).toEqual({ on: true, canonical: "path", image: "social.png" });
});

test("a draft export carries schema transform definitions in app/ and omits them from dist/", async () => {
  const source = '{"version":1,"transitions":[]}';
  const withTransforms = { ...draft, files: { ...draft.files, "actions.json": JSON.stringify({ version: 1, actions: {} }), "schema-transforms.json": source } } as never;
  const result = await createProjectExport(withTransforms, undefined, {
    mcp,
    build: async () => ({ version: 1, entrypoint: "index.html", files: { "index.html": "<main></main>" } }) as never,
  });
  const paths = readTar(new Uint8Array(await result.blob.arrayBuffer())).map((entry) => entry.path);
  expect(paths).toContain("notes/app/schema-transforms.json");
  expect(paths).not.toContain("notes/dist/schema-transforms.json");
});
