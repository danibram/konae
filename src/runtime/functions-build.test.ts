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
const crons = { report: { cron: "0 8 * * 1-5", function: "orders" } };
const cronFiles = {
  "functions.json": JSON.stringify({ version: 1, functions: { orders: { entry: "functions/orders.ts", access: "scheduled" } } }),
  "crons.json": JSON.stringify({ version: 1, crons }),
};

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
  const scheduled = await buildProjectInBrowser({ ...project, modules: { functions: { on: false } }, files: { ...files, ...cronFiles } });
  expect(scheduled.serverArtifact?.manifest.crons).toEqual(crons);
  expect(Object.keys(scheduled.files)).toEqual(["index.html"]);
});

test("local and refreshed folder builds validate candidate server bundles", async () => {
  const root = await folder();
  for (const [file, contents] of Object.entries(cronFiles)) await writeFile(path.join(root, "app", file), contents);
  const settingsPath = path.join(root, "konae.json");
  const settings = JSON.parse(await readFile(settingsPath, "utf8"));
  settings.modules.functions.on = false;
  await writeFile(settingsPath, JSON.stringify(settings));
  const local = await buildLocal(root);
  expect(local.serverArtifact?.manifest.crons).toEqual(crons);
  expect(local.serverArtifact?.bundles.orders).toContain("SERVER_TOP_LEVEL_SENTINEL");
  expect(await readFile(path.join(root, "server/orders.mjs"), "utf8")).toContain("SERVER_PRIVATE_BYTES");
  expect(JSON.stringify(local.artifact.files)).not.toContain("SERVER_PRIVATE_BYTES");
  const generated = generatedFiles({ settings: { id: project.id, name: project.name, slug: project.slug, visibility: "public", kernel: "0.12.1", manifest: {}, modules: project.modules }, actionsSource: '{"version":1,"actions":{}}', schemaSource: "", runtime: "", staticArtifact: await readFile("src/lib/static-artifact.js", "utf8"), localBuilder: await readFile("src/lib/local-builder.js", "utf8"), parserSource: await readFile("node_modules/@babel/parser/lib/index.js", "utf8"), parserLicense: await readFile("node_modules/@babel/parser/LICENSE", "utf8") });
  const generatedBuilder = generated.find((entry) => entry.path === "scripts/local-builder.mjs")?.contents as string;
  for (const entry of generated) { await mkdir(path.dirname(path.join(root, entry.path)), { recursive: true }); await writeFile(path.join(root, entry.path), entry.contents); }
  expect(generated.find((entry) => entry.path === "scripts/build.mjs")?.contents).toContain("babel-parser.cjs");
  expect(generated.some((entry) => entry.path === "scripts/babel-parser.cjs")).toBe(true);
  expect(generated.some((entry) => entry.path === "scripts/BABEL-PARSER-LICENSE")).toBe(true);
  await mkdir(path.join(root, "node_modules"));
  await symlink(path.resolve("node_modules/esbuild"), path.join(root, "node_modules/esbuild"));
  const process = Bun.spawn(["node", "scripts/build.mjs"], { cwd: root, stdout: "pipe", stderr: "pipe" });
  const stderr = await new Response(process.stderr).text();
  expect({ exitCode: await process.exited, stderr }).toEqual({ exitCode: 0, stderr: "" });
  expect(await readFile(path.join(root, "server/orders.mjs"), "utf8")).toContain("SERVER_PRIVATE_BYTES");
  const published = JSON.parse(await readFile(path.join(root, "server/manifest.json"), "utf8"));
  expect(published.manifest.crons).toEqual(crons);
  expect(existsSync(path.join(root, "dist/crons.json"))).toBe(false);
  expect(await readFile(path.join(root, "app/package-lock.json"), "utf8")).toBe(files["package-lock.json"]);

  await writeFile(path.join(root, "scripts/local-builder.mjs"), `export async function bundleModule() { return { script: "", styles: "" }; }
export async function bundleFunctions(app, manifest) { return { version: 1, abi: "konae-functions/1", manifest, bundles: { orders: 'if (false) import("dep"); export const handle = () => null;' } }; }
`);
  const malformed = Bun.spawn(["node", "scripts/build.mjs"], { cwd: root, stdout: "pipe", stderr: "pipe" });
  const [malformedCode, malformedError] = await Promise.all([malformed.exited, new Response(malformed.stderr).text()]);
  expect(malformedCode).not.toBe(0);
  expect(malformedError).toContain("Function orders bundle must be a self-contained ESM module exporting handle.");
  expect(await readFile(path.join(root, "server/orders.mjs"), "utf8")).toContain("SERVER_PRIVATE_BYTES");
  await writeFile(path.join(root, "scripts/local-builder.mjs"), generatedBuilder);

  await writeFile(path.join(root, "app/functions/orders.ts"), "export default () => null;");
  const invalid = Bun.spawn(["node", "scripts/build.mjs"], { cwd: root, stdout: "pipe", stderr: "pipe" });
  const [code, error] = await Promise.all([invalid.exited, new Response(invalid.stderr).text()]);
  expect(code).not.toBe(0);
  expect(error).toContain("Function orders bundle must be a self-contained ESM module exporting handle.");
  expect(await readFile(path.join(root, "server/orders.mjs"), "utf8")).toContain("SERVER_PRIVATE_BYTES");
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

test("both builders use the artifact contract refusal without handle", async () => {
  const root = await folder();
  const noHandle = "export default () => null;";
  await writeFile(path.join(root, "app/functions/orders.ts"), noHandle);
  const refusal = { code: "function.bundle-contract", message: "Function orders bundle must be a self-contained ESM module exporting handle." };
  await expect(bundleFunctions(path.join(root, "app"), parseFunctions(source))).rejects.toMatchObject(refusal);
  await expect(buildProjectInBrowser({ ...project, files: { ...files, "functions/orders.ts": noHandle } })).rejects.toMatchObject(refusal);
});

test("both builders preserve aliased handle, shutdown, and extra exports", async () => {
  const root = await folder();
  const source = "const run = () => new Response('ok'); const stop = () => {}; export { run as handle, stop as shutdown }; export const extra = 1;";
  await writeFile(path.join(root, "app/functions/orders.ts"), source);
  const local = await buildLocal(root);
  expect(local.serverArtifact?.bundles.orders).toMatch(/export\s*\{[^}]*handle/);
  const browser = await buildProjectInBrowser({ ...project, files: { ...files, "functions/orders.ts": source } });
  expect(browser.serverArtifact?.bundles.orders).toMatch(/export\s*\{[^}]*handle/);
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
