import { chmod, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { version } from "../package.json";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const os = process.platform === "darwin" ? "darwin" : process.platform === "linux" ? "linux" : undefined;
const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : undefined;
if (!os || !arch) throw Error(`No konae binary release target for ${process.platform}/${process.arch}.`);

const esbuildPlatform = `${process.platform === "darwin" ? "darwin" : "linux"}-${arch}`;
const generated = path.join(root, "scripts/.konae-compile.generated.ts");
const output = path.resolve(process.env.KONAE_BINARY_OUTFILE ?? path.join(root, `out/konae-${version}-${os}-${arch}`));
const typeScriptLibDirectory = path.join(root, "node_modules/typescript/lib");
const typeScriptLibs = Object.fromEntries(await Promise.all((await readdir(typeScriptLibDirectory))
  .filter((name) => /^lib\..+\.d\.ts$/.test(name))
  .map(async (name) => [name, await readFile(path.join(typeScriptLibDirectory, name), "utf8")] as const)));
const embeddedResources = {
  templates: await readFile(path.join(root, "kernel-rs/konae/src/project-templates.json"), "utf8"),
  runtime: await readFile(path.join(root, "src/mcp/runtime.js"), "utf8"),
  staticArtifact: await readFile(path.join(root, "src/lib/static-artifact.js"), "utf8"),
  localBuilder: await readFile(path.join(root, "src/lib/local-builder.js"), "utf8"),
  parserSource: await readFile(path.join(root, "node_modules/@babel/parser/lib/index.js"), "utf8"),
  parserLicense: await readFile(path.join(root, "node_modules/@babel/parser/LICENSE"), "utf8"),
  typeScriptLibs,
};
const source = `
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import nativeEsbuild from "@esbuild/${esbuildPlatform}/bin/esbuild" with { type: "file" };

const directory = await mkdtemp(path.join(tmpdir(), "konae-esbuild-"));
const executable = path.join(directory, "esbuild");
await writeFile(executable, await readFile(nativeEsbuild));
await chmod(executable, 0o755);
process.env.ESBUILD_BINARY_PATH = executable;
process.on("exit", () => { try { rmSync(directory, { recursive: true, force: true }); } catch {} });
(globalThis as typeof globalThis & { __KONAE_RESOURCES__: unknown }).__KONAE_RESOURCES__ = ${JSON.stringify(embeddedResources)};
const cli = await import("./konae");
await cli.main().catch((error) => { console.error(error instanceof Error ? error.message : error); process.exitCode = 1; });
`;

await writeFile(generated, source);
try {
  await Bun.spawn([process.execPath, "build", "--compile", "--packages=bundle", generated, "--outfile", output], { cwd: root, stdout: "inherit", stderr: "inherit" }).exited.then((code) => {
    if (code !== 0) throw Error(`Bun failed to compile ${output}.`);
  });
} finally {
  await rm(generated, { force: true });
}
await chmod(output, 0o755);
console.log(`Built ${output}`);
