import { build } from "esbuild";
import { writeFile } from "node:fs/promises";

const result = await build({
  entryPoints: ["browser/entry.ts"],
  outdir: "browser-dist",
  bundle: true,
  splitting: true,
  format: "esm",
  platform: "browser",
  target: ["es2022"],
  metafile: true,
});
await writeFile("browser-dist/meta.json", JSON.stringify(result.metafile, null, 2));
const outputs = Object.entries(result.metafile.outputs);
const externalImports = outputs.flatMap(([output, value]) => (value.imports ?? []).filter((entry) => entry.external).map((entry) => ({ output, path: entry.path, kind: entry.kind })));
const unexpected = externalImports.filter((entry) => !(entry.path === "node:fs" && entry.kind === "require-call"));
if (unexpected.length) throw new Error(`Unexpected external imports: ${JSON.stringify(unexpected)}`);
const entry = outputs.find(([name]) => name.endsWith("/entry.js"))?.[1];
if (!entry || entry.bytes > 4096 || !entry.imports.some((item) => item.kind === "dynamic-import")) throw new Error("Agent code is not behind the user-triggered lazy import");
const totalBytes = outputs.reduce((sum, [, output]) => sum + output.bytes, 0);
console.log(JSON.stringify({ totalBytes, entryBytes: entry.bytes, outputs: Object.fromEntries(outputs.map(([name, output]) => [name, output.bytes])), externalImports }, null, 2));
