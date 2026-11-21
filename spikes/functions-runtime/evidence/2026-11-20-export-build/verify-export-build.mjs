import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { createTar } from "../../../../src/lib/tar.ts";
import { parseFunctions, serverFolderEntries } from "../../../../src/kernel/functions.ts";
import { readAppFolder, sourcesPayload } from "../../../../src/lib/import-folder.ts";
import { sourceBytes } from "../../../../src/lib/source-files.ts";

const repo = path.resolve(import.meta.dir, "../../../..");
const { exportLocal } = await import(pathToFileURL(path.join(repo, "scripts/konae.ts")).href);
const evidence = import.meta.dir;
const temp = await mkdtemp(path.join(os.tmpdir(), "konae-export-build-"));
const source = path.join(temp, "source");
const first = path.join(temp, "first-export");
const imported = path.join(temp, "imported-without-git");
const output = path.join(temp, "reexported");
const npmConfig = path.join(temp, "empty.npmrc");
const startedAt = new Date().toISOString();
const commands = [];
let success = false;

function run(command, args, cwd, env = process.env) {
  const result = spawnSync(command, args, { cwd, env, encoding: "utf8", maxBuffer: 4 * 1024 * 1024 });
  commands.push({ command: [command, ...args].join(" "), cwd, status: result.status, stdout: result.stdout, stderr: result.stderr });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed (${result.status}).\n${result.stdout}\n${result.stderr}`);
  return result.stdout;
}

async function put(root, relative, bytes) {
  const target = path.join(root, relative);
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, bytes);
}

async function filesUnder(root, prefix = "") {
  const entries = await readdir(path.join(root, prefix), { withFileTypes: true });
  const out = [];
  for (const entry of entries) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) out.push(...await filesUnder(root, relative)); else out.push(relative);
  }
  return out.sort();
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}

try {
  await mkdir(path.join(source, "app"), { recursive: true });
  await writeFile(npmConfig, "");
  const npmEnv = { ...process.env, NPM_CONFIG_USERCONFIG: npmConfig, NPM_CONFIG_CACHE: path.join(temp, "npm-cache"), npm_config_registry: "https://registry.npmjs.org/" };
  const buildRoot = path.join(source, "app/build");
  const rootPackage = {
    name: "portable-konae-fixture", private: true, type: "module",
    scripts: { build: "node scripts/app-build.mjs" },
    dependencies: { esbuild: "0.25.12", react: "18.3.1", "react-dom": "18.3.1" },
    devDependencies: { "@types/react": "18.3.12", "@types/react-dom": "18.3.1" },
  };
  const appPackage = { name: "portable-konae-functions", private: true, type: "module", dependencies: { hono: "4.7.2" } };
  await put(source, "app/build/package.json", JSON.stringify(rootPackage, null, 2));
  await put(source, "app/package.json", JSON.stringify(appPackage, null, 2));
  run("npm", ["install", "--package-lock-only", "--ignore-scripts", "--no-audit", "--no-fund"], buildRoot, npmEnv);
  run("npm", ["install", "--package-lock-only", "--ignore-scripts", "--no-audit", "--no-fund"], path.join(source, "app"), npmEnv);

  const buildFiles = [
    "package.json", "package-lock.json", "tsconfig.json", "tsconfig.base.json",
    "scripts/app-build.mjs", "scripts/local-builder.mjs",
  ];
  await put(source, "app/build.json", JSON.stringify({ version: 1, files: buildFiles }, null, 2));
  await put(source, "app/build/tsconfig.json", JSON.stringify({ extends: "./tsconfig.base.json", include: ["app/src", "app/assets.d.ts", "app/functions"] }, null, 2));
  await put(source, "app/build/tsconfig.base.json", JSON.stringify({
    compilerOptions: {
      target: "ES2022", module: "ESNext", moduleResolution: "Bundler", jsx: "react-jsx", strict: true,
      baseUrl: ".", paths: {
        "@repo/*": ["app/vendor/*"], "@/*": ["app/src/*"], "node:buffer": ["app/vendor/shims/node-buffer.ts"],
      },
    },
  }, null, 2));
  await put(source, "app/build/scripts/app-build.mjs", 'import { buildApp } from "./local-builder.mjs"; await buildApp();\n');
  await put(source, "app/build/scripts/local-builder.mjs", `import { build } from "esbuild";
import { mkdir, writeFile } from "node:fs/promises";
export async function buildApp() {
  await mkdir("dist", { recursive: true });
  await build({ entryPoints: ["app/src/main.tsx"], bundle: true, outfile: "dist/app.js", platform: "browser", format: "esm", target: "es2022", jsx: "automatic", tsconfig: "tsconfig.json", loader: { ".png": "dataurl" }, minify: true });
  await writeFile("dist/index.html", '<!doctype html><html><body><div id="root"></div><script type="module" src="/app.js"></script></body></html>');
}
`);
  await put(source, "app/src/main.tsx", `import { createRoot } from "react-dom/client";
import { banner } from "@repo/ui";
import { title } from "@/title";
import { Buffer } from "node:buffer";
import logo from "../assets/logo.png";
const root = document.getElementById("root");
if (!root) throw new Error("root missing");
createRoot(root).render(<main><h1>{title}</h1><p>{banner}</p><img src={logo} /><code>{Buffer.from("node-shim").toString()}</code></main>);
`);
  await put(source, "app/src/title.ts", 'export const title = "draft-v2";\n');
  await put(source, "app/vendor/ui.ts", 'export const banner = "alias-vendor";\n');
  await put(source, "app/vendor/shims/node-buffer.ts", 'export const Buffer = { from(value: string) { return { toString: () => value }; } };\n');
  await put(source, "app/assets.d.ts", 'declare module "*.png" { const url: string; export default url; }\n');
  await put(source, "app/assets/logo.png", Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+cB9sAAAAASUVORK5CYII=", "base64"));
  await put(source, "app/functions.json", JSON.stringify({ version: 1, functions: { health: { entry: "functions/health.ts", mount: "/api/health", methods: ["GET"], access: "anonymous" } } }, null, 2));
  await put(source, "app/functions/health.ts", `import { Hono } from "hono";
const app = new Hono();
app.get("/", (context) => context.text("function-draft-v2"));
export const handle = (request: Request) => app.fetch(request);
`);
  await put(source, "konae.json", JSON.stringify({
    id: "portable_fixture", name: "Portable fixture", slug: "portable-fixture", visibility: "public",
    signupPolicy: { mode: "open", allowlist: [] }, signInMethods: ["password"], kernel: "0.12.1",
    modules: { functions: { on: true } }, manifest: { version: 1, routing: "spa" },
    released: { artifactVersion: "published-v1", server: { version: "published-v1" } },
  }, null, 2));
  await put(source, "dist/index.html", "published-v1");
  const oldManifest = parseFunctions(JSON.stringify({ version: 1, functions: { health: { entry: "functions/health.ts", mount: "/api/health", methods: ["GET"], access: "anonymous" } } }));
  const oldServer = { version: 1, abi: "konae-functions/1", manifest: oldManifest, bundles: { health: 'export const handle = () => new Response("function-published-v1");' } };
  for (const entry of await serverFolderEntries(oldServer)) await put(source, entry.path, entry.contents);
  await put(source, ".env", "SYNTHETIC_SECRET=not-for-export");
  await put(source, ".npmrc", "//registry.npmjs.org/:_authToken=synthetic-token");
  await put(source, ".git/config", "synthetic git metadata");
  await put(source, ".konae/credentials.json", "synthetic credential");
  await put(source, "data/database.sqlite", "synthetic data");
  await put(source, "node_modules/.package-lock.json", "synthetic cache");

  await exportLocal(source, first);
  check(await readFile(path.join(first, "package.json"), "utf8") === await readFile(path.join(buildRoot, "package.json"), "utf8"), "export did not project the authoritative build package");
  check(await readFile(path.join(first, "dist/index.html"), "utf8") === "published-v1", "export rebuilt or replaced the published static snapshot");
  check((await readFile(path.join(first, "server/health.mjs"), "utf8")).includes("function-published-v1"), "export rebuilt or replaced the published Functions snapshot");
  const firstFiles = await filesUnder(first);
  check(!firstFiles.some((file) => [".env", ".npmrc", ".git/config", ".konae/credentials.json", "data/database.sqlite", "node_modules/.package-lock.json"].includes(file)), "an excluded credential, data, git, or cache file reached the export");

  const firstArchive = createTar(await Promise.all(firstFiles.map(async (file) => ({ path: file, contents: new Uint8Array(await readFile(path.join(first, file))) }))));
  const importedFolder = readAppFolder(firstArchive);
  const payload = sourcesPayload(importedFolder, { id: "portable_fixture", name: "Portable fixture", slug: "portable-fixture" }, "public");
  check(payload.serverArtifact?.bundles.health.includes("function-published-v1"), "import changed the published server artifact");
  check(payload.project.files["build.json"] !== undefined, "import lost the build project manifest");
  await put(imported, "konae.json", JSON.stringify(importedFolder.settings, null, 2));
  for (const file of Object.keys(importedFolder.sources.files)) await put(imported, `app/${file}`, sourceBytes(importedFolder.sources, file));
  for (const [file, bytes] of Object.entries(importedFolder.dist)) await put(imported, `dist/${file}`, bytes);
  for (const [file, bytes] of Object.entries(importedFolder.server ?? {})) await put(imported, `server/${file}`, bytes);
  check(!(await filesUnder(imported)).some((file) => file.startsWith(".git/") || file.startsWith("node_modules/")), "the import fixture unexpectedly has Git or node_modules");
  await exportLocal(imported, output);
  check(await readFile(path.join(output, "package-lock.json"), "utf8") === await readFile(path.join(buildRoot, "package-lock.json"), "utf8"), "re-export did not recover the exact package lock");
  check(await readFile(path.join(output, "dist/index.html"), "utf8") === "published-v1", "re-export lost the published static snapshot");
  check((await readFile(path.join(output, "server/health.mjs"), "utf8")).includes("function-published-v1"), "re-export lost the published Functions snapshot");

  const cleanEnv = { ...npmEnv, NPM_CONFIG_CACHE: path.join(temp, "npm-cache-clean-install") };
  run("npm", ["ci", "--no-audit", "--no-fund"], output, cleanEnv);
  run("npm", ["ci", "--no-audit", "--no-fund"], path.join(output, "app"), cleanEnv);
  const realOutput = await realpath(output);
  const rootEsbuild = await realpath(path.join(output, "node_modules/esbuild/package.json"));
  const appHono = await realpath(path.join(output, "app/node_modules/hono/package.json"));
  check(rootEsbuild.startsWith(`${realOutput}${path.sep}node_modules${path.sep}`), "esbuild resolved outside the exported root install");
  check(appHono.startsWith(`${realOutput}${path.sep}app${path.sep}node_modules${path.sep}`), "Hono resolved outside the exported app install");
  run("npm", ["run", "build"], output, cleanEnv);
  check((await readFile(path.join(output, "dist/app.js"), "utf8")).includes("draft-v2"), "npm run build did not compile the editable TypeScript SPA");
  check((await readFile(path.join(output, "dist/app.js"), "utf8")).includes("node-shim"), "the Node shim did not bundle");
  check((await readFile(path.join(output, "dist/app.js"), "utf8")).includes("data:image/png;base64"), "the PNG loader did not bundle the synthetic asset");
  await rm(path.join(output, "dist"), { recursive: true, force: true });
  await rm(path.join(output, "server"), { recursive: true, force: true });
  run("bun", ["run", path.join(repo, "scripts/konae.ts"), "build", "--dir", output], repo, cleanEnv);
  check((await readFile(path.join(output, "dist/app.js"), "utf8")).includes("draft-v2"), "konae build did not rebuild the editable SPA");
  check((await readFile(path.join(output, "server/health.mjs"), "utf8")).includes("function-draft-v2"), "konae build did not rebuild the Hono function bundle");
  check(await readFile(path.join(output, "dist/index.html"), "utf8") !== "published-v1", "the explicit build did not replace the published artifact with the draft build");

  const result = {
    startedAt, finishedAt: new Date().toISOString(),
    passed: true,
    checks: {
      buildProjectReexportWithoutGit: true,
      publishedDistAndServerPreservedAlongsideDraft: true,
      excludedCredentialsDataGitAndNodeModules: true,
      npmCiUsesOnlyExportedLockfiles: true,
      aliasesTsconfigExtendsPngNodeShimAndReactBundle: true,
      honoFunctionsRebuiltAfterDeletingServerAndDist: true,
    },
    packages: { esbuild: "0.25.12", react: "18.3.1", reactDom: "18.3.1", hono: "4.7.2" },
    commands,
  };
  await writeFile(path.join(evidence, "results.json"), `${JSON.stringify(result, null, 2)}\n`);
  success = true;
  console.log(JSON.stringify({ passed: true, checks: result.checks, results: path.join(evidence, "results.json") }, null, 2));
} catch (error) {
  const result = { startedAt, finishedAt: new Date().toISOString(), passed: false, error: error instanceof Error ? error.stack : String(error), commands };
  await writeFile(path.join(evidence, "results.json"), `${JSON.stringify(result, null, 2)}\n`);
  throw error;
} finally {
  if (success) await rm(temp, { recursive: true, force: true });
  else console.error(`Fixture retained for diagnosis at ${temp}`);
}
