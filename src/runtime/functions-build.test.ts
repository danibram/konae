import { afterEach, expect, mock, test } from "bun:test";
import * as esbuild from "esbuild";
import { mkdtemp, mkdir, writeFile, readFile, rm, symlink } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { parseFunctions } from "../kernel/functions";
import { bundleFunctions, bundleModule } from "../lib/local-builder.js";
import { buildLocal, exportLocal } from "../../scripts/konae";
import { generatedFiles } from "../lib/app-folder";
import { createDefaultManifest } from "../deployments/manifest";
import type { MiniAppProject } from "../lib/types";

mock.module("esbuild-wasm", () => ({ initialize: async () => {}, build: esbuild.build }));
const { buildProjectInBrowser, compileProjectInBrowser } = await import("./browser-build");
const roots: string[] = [];
afterEach(async () => { await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true }))); });
const source = JSON.stringify({ version: 1, functions: { orders: { entry: "functions/orders.ts", mount: "/api/orders", methods: ["GET"], access: "anonymous" } } });
const serverCode = 'throw Error("SERVER_TOP_LEVEL_SENTINEL"); export const handle = () => new Response("SERVER_PRIVATE_BYTES");';
const files = { "index.html": "<!doctype html><html><body>Public</body></html>", "functions.json": source, "functions/orders.ts": serverCode, "package.json": '{"dependencies":{}}', "package-lock.json": '{"lockfileVersion":3}' };
const project: MiniAppProject = { id: "functions", name: "Functions", slug: "functions", manifest: { ...createDefaultManifest(), visibility: "public" }, files, directories: [], updatedAt: 0, modules: { functions: { on: true } } };

async function folder() {
  const root = await mkdtemp(path.join(tmpdir(), "konae-functions-")); roots.push(root);
  for (const [name, source] of Object.entries(files)) { await mkdir(path.dirname(path.join(root, "app", name)), { recursive: true }); await writeFile(path.join(root, "app", name), source); }
  await writeFile(path.join(root, "konae.json"), JSON.stringify({ id: project.id, name: project.name, slug: project.slug, kernel: "0.12.1", manifest: {}, visibility: "public", modules: project.modules }));
  return root;
}

test("editor builds separate ESM bytes and preview never imports server entry", async () => {
  const built = await buildProjectInBrowser(project);
  expect(built.serverArtifact?.bundles.orders).toContain("SERVER_TOP_LEVEL_SENTINEL");
  expect(built.serverArtifact?.bundles.orders).toContain("SERVER_PRIVATE_BYTES");
  expect(JSON.stringify(built.files)).not.toContain("SERVER_PRIVATE_BYTES");
  expect(Object.keys(built.files)).toEqual(["index.html"]);
  const preview = await compileProjectInBrowser({ ...project, files: { ...files, "app.js": 'document.title = "client";' } });
  expect(preview.script).not.toContain("SERVER_PRIVATE_BYTES");
});

test("local and generated folder builders do not execute top-level server code", async () => {
  const root = await folder();
  const local = await buildLocal(root);
  expect(local.serverArtifact?.bundles.orders).toContain("SERVER_TOP_LEVEL_SENTINEL");
  expect(await readFile(path.join(root, "server/orders.mjs"), "utf8")).toContain("SERVER_PRIVATE_BYTES");
  expect(JSON.stringify(local.artifact.files)).not.toContain("SERVER_PRIVATE_BYTES");
  const generated = generatedFiles({ settings: { id: project.id, name: project.name, slug: project.slug, visibility: "public", kernel: "0.12.1", manifest: {}, modules: project.modules }, actionsSource: '{"version":1,"actions":{}}', schemaSource: "", runtime: "", staticArtifact: await readFile("src/lib/static-artifact.js", "utf8"), localBuilder: await readFile("src/lib/local-builder.js", "utf8") });
  for (const entry of generated) { await mkdir(path.dirname(path.join(root, entry.path)), { recursive: true }); await writeFile(path.join(root, entry.path), entry.contents); }
  await symlink(path.resolve("node_modules"), path.join(root, "node_modules"));
  const process = Bun.spawn(["node", "scripts/build.mjs"], { cwd: root, stdout: "pipe", stderr: "pipe" });
  const stderr = await new Response(process.stderr).text();
  expect({ exitCode: await process.exited, stderr }).toEqual({ exitCode: 0, stderr: "" });
  expect(await readFile(path.join(root, "server/orders.mjs"), "utf8")).toContain("SERVER_PRIVATE_BYTES");
  expect(await readFile(path.join(root, "app/package-lock.json"), "utf8")).toBe(files["package-lock.json"]);
});

test("browser entries cannot import server sources", async () => {
  const frontend = { ...project, files: { ...files, "app.js": 'import { handle } from "./functions/orders.ts"; console.log(handle);' } };
  await expect(compileProjectInBrowser(frontend)).rejects.toThrow("Server function sources cannot be imported by a browser entry.");
  const root = await folder();
  await writeFile(path.join(root, "app/app.js"), frontend.files["app.js"]);
  await expect(bundleModule(path.join(root, "app"), "app.js")).rejects.toThrow("Server function sources cannot be imported by a browser entry.");
});

test("bundler refuses non-self-contained exports and host imports", async () => {
  const root = await folder();
  const entry = path.join(root, "app/functions/orders.ts");
  for (const code of ['export default () => new Response("bad");', 'import {readFile} from "node:fs"; export const handle = readFile;', 'export const handle = () => import(globalThis.location);', 'export const handle = () => require(globalThis.location);']) {
    await writeFile(entry, code);
    await expect(bundleFunctions(path.join(root, "app"), parseFunctions(source))).rejects.toThrow();
  }
});

const honoModules = path.resolve("spikes/functions-runtime/app/node_modules");
test.skipIf(!existsSync(path.join(honoModules, "hono/package.json")))("optional Hono is bundled into one ESM file without evaluation", async () => {
  const root = await folder();
  const hono = JSON.parse(await readFile(path.join(honoModules, "hono/package.json"), "utf8"));
  await writeFile(path.join(root, "app/package.json"), JSON.stringify({ dependencies: { hono: hono.version } }));
  await symlink(honoModules, path.join(root, "app/node_modules"));
  await writeFile(path.join(root, "app/functions/orders.ts"), 'import { Hono } from "hono"; throw Error("HONO_TOP_LEVEL_SENTINEL"); const app = new Hono(); app.get("/api/orders", c => c.text("ok")); export const handle = (request, context) => app.fetch(request, context);');
  const built = await bundleFunctions(path.join(root, "app"), parseFunctions(source));
  expect(built.bundles.orders).toContain("HONO_TOP_LEVEL_SENTINEL");
  expect(built.bundles.orders).not.toMatch(/(?:^|;)import[ (]/);
  expect(Object.keys(built.bundles)).toEqual(["orders"]);
});

test("CLI export keeps a published server bundle when the editable entry was removed", async () => {
  const root = await folder();
  const built = await buildLocal(root);
  const settingsPath = path.join(root, "konae.json");
  const settings = JSON.parse(await readFile(settingsPath, "utf8"));
  settings.released = { server: JSON.parse(await readFile(path.join(root, "server/manifest.json"), "utf8")) };
  await writeFile(settingsPath, JSON.stringify(settings));
  await rm(path.join(root, "app/functions/orders.ts"));
  await writeFile(path.join(root, "app/functions.json"), '{"version":1,"functions":{}}');
  const output = path.join(root, "export");
  await exportLocal(root, output);
  expect(await readFile(path.join(output, "server/orders.mjs"), "utf8")).toBe(built.serverArtifact.bundles.orders);
  expect(await readFile(path.join(output, "app/functions.json"), "utf8")).toBe('{"version":1,"functions":{}}');
  expect(JSON.parse(await readFile(path.join(output, "konae.json"), "utf8")).released).toEqual(settings.released);
});

test("a browser stylesheet cannot embed server source as a data URL", async () => {
  await expect(compileProjectInBrowser({ ...project, files: { ...files, "app.js": 'import "./client.css";', "client.css": 'body { background: url("./functions/orders.ts"); }' } })).rejects.toThrow("Server function sources cannot be imported by a browser entry.");
});
