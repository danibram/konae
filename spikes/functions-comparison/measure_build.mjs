import { readFile, stat } from "node:fs/promises";
import { resolve, join } from "node:path";
import { bundleFunctions } from "../../src/lib/local-builder.js";

const app = resolve(process.argv[2] ?? "");
if (!app) throw new Error("Pass the app directory that contains functions.json.");
const manifest = JSON.parse(await readFile(join(app, "functions.json"), "utf8"));
const results = [];
for (const [name, definition] of Object.entries(manifest.functions)) {
  const start = process.hrtime.bigint();
  const artifact = await bundleFunctions(app, { version: 1, functions: { [name]: definition } }, undefined, false);
  const elapsedMs = Number(process.hrtime.bigint() - start) / 1e6;
  const bytes = Buffer.byteLength(artifact.bundles[name]);
  results.push({ name, buildMs: elapsedMs, bundleBytes: bytes });
}
console.log(JSON.stringify({ app, functionCount: results.length, buildMsSum: results.reduce((sum, row) => sum + row.buildMs, 0), results }, null, 2));
