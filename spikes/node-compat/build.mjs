import assert from "node:assert/strict";
import { mkdir, readFile, symlink, writeFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { createServer } from "vite";
import { patchEditor } from "./patch.mjs";
import { providers } from "./installed-providers.mjs";
import { cases, filesFor, settings } from "./cases.mjs";
import { refusal } from "./policy.mjs";
import { hash } from "node:crypto";

const { bundleFunctions } = await import("../../src/lib/local-builder.js");
const { parseFunctions, validateServerArtifact } = await import("../../src/kernel/functions.ts");
const here = fileURLToPath(new URL(".", import.meta.url));
const root = fileURLToPath(new URL("../../", import.meta.url));
const output = resolve(process.argv[2] ?? "/tmp/konae-functions-node-compat-design-spike");
const packageText = await readFile(join(here, "package.json"), "utf8");
const lock = await readFile(join(here, "package-lock.json"));
const names = Object.keys(cases).filter((name) => !cases[name].refused);
const refused = Object.keys(cases).filter((name) => cases[name].refused);

async function fixture(directory, fixtureNames) {
  const app = join(directory, "app");
  await mkdir(join(app, "functions"), { recursive: true });
  await mkdir(join(directory, "data"), { recursive: true });
  for (const [file, data] of Object.entries(filesFor(fixtureNames, packageText))) {
    await writeFile(join(app, file), data);
  }
  await writeFile(join(app, "package-lock.json"), lock);
  await writeFile(join(directory, "konae.json"), JSON.stringify(settings));
  await symlink(join(here, "node_modules"), join(app, "node_modules")).catch((error) => {
    if (error.code !== "EEXIST") throw error;
  });
  return app;
}

const localApp = await fixture(join(output, "cli"), names);
const editorApp = await fixture(join(output, "editor"), names);
const denied = await fixture(join(output, "refused"), refused);
const server = await createServer({
  root,
  configFile: false,
  plugins: [{
    name: "node-compat-spike",
    enforce: "pre",
    configureServer(server) {
      server.middlewares.use("/__node_compat", (request, response) => {
        const source = providers.get(request.url.slice(1));
        response.statusCode = source === undefined ? 404 : 200;
        response.end(source);
      });
    },
    transform(source, id) {
      if (id === resolve(root, "src/runtime/browser-build.ts")) return patchEditor(source);
    },
  }],
  optimizeDeps: { include: ["esbuild-wasm", "@babel/parser"] },
  server: { host: "127.0.0.1", port: 0 },
});
await server.listen();
const browser = await chromium.launch({ channel: process.env.KONAE_SPIKE_CHROME ?? "chrome" });
try {
  const page = await browser.newPage();
  page.on("console", (msg) => { if (msg.type() === "error") console.error(msg.text()); });
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/spikes/node-compat/package.json`);
  await page.evaluate(() => import("/spikes/node-compat/browser.mjs"));
  const editor = await page.evaluate((packageText) => globalThis.konaeSpike(packageText), packageText);
  const manifest = parseFunctions(await readFile(join(localApp, "functions.json"), "utf8"));
  const rejected = parseFunctions(await readFile(join(denied, "functions.json"), "utf8"));
  const local = {};
  for (const [name, probe] of Object.entries(cases)) {
    const app = probe.refused ? denied : localApp;
    const declaration = (probe.refused ? rejected : manifest).functions[name];
    try {
      const artifact = await bundleFunctions(app, { version: 1, functions: { [name]: declaration } });
      local[name] = { bundle: artifact.bundles[name] };
    } catch (error) {
      const { message, code } = error.errors?.[0]?.detail ?? error;
      local[name] = { message, code };
    }
  }
  let failures = 0;
  for (const [name, probe] of Object.entries(cases)) {
    try {
      if (probe.refused) {
        assert.deepEqual(local[name], refusal(probe.refused));
        assert.deepEqual(editor[name], local[name]);
        console.log(`REFUSED ${name} ${local[name].code} ${local[name].message}`);
        continue;
      }
      for (const [door, result] of [["cli", local[name]], ["editor", editor[name]]]) {
        assert.ok(result.bundle, `${door}: ${result.message}`);
        const request = { name, bundle: result.bundle };
        const response = await page.evaluate(({ name, bundle }) => globalThis.runKonaeBundle(name, bundle), request);
        assert.deepEqual(response, { status: 200, body: probe.expected }, door);
      }
      console.log(`PASS ${name} ${JSON.stringify(probe.expected)} (${Buffer.byteLength(local[name].bundle)}/${Buffer.byteLength(editor[name].bundle)} UTF-8 bytes)`);
    } catch (error) {
      failures++;
      console.error(`FAIL ${name} ${error.message}`);
      console.error(`  cli: ${local[name].message ?? "built"}; editor: ${editor[name].message ?? "built"}`);
    }
  }
  if (failures) process.exitCode = 1;
  const artifact = { version: 1, abi: "konae-functions/1", manifest, bundles: {} };
  for (const [door, app, results] of [["cli", localApp, local], ["editor", editorApp, editor]]) {
    artifact.bundles = Object.fromEntries(names.map((name) => [name, results[name].bundle]));
    if (Object.values(artifact.bundles).some((bundle) => bundle === undefined)) continue;
    validateServerArtifact(manifest, artifact);
    const folder = dirname(app);
    const directory = join(folder, "server");
    await mkdir(directory, { recursive: true });
    await mkdir(join(folder, "dist"), { recursive: true });
    await writeFile(join(folder, "dist/index.html"), await readFile(join(app, "index.html")));
    const hashes = {};
    for (const [name, bundle] of Object.entries(artifact.bundles)) {
      const file = join(directory, name + ".mjs");
      await writeFile(file, bundle);
      hashes[name] = hash("sha256", bundle);
    }
    await writeFile(join(directory, "manifest.json"), JSON.stringify({ version: 1, abi: artifact.abi, manifest, hashes }));
    console.log(`WROTE ${door} ${basename(folder)}/server`);
  }
} finally {
  await browser.close();
  await server.close();
}
