import { build } from "esbuild";
import path from "node:path";
import { readFile, realpath } from "node:fs/promises";

export async function bundleModule(app, entry, development = false) {
  app = await realpath(app);
  const result = await build({
    entryPoints: [path.join(app, entry)], absWorkingDir: path.resolve(app), bundle: true, write: false,
    platform: "browser", format: "esm", target: "es2022", outdir: "out", jsx: "automatic",
    plugins: [{ name: "server-source-boundary", setup(build) {
      build.onLoad({ filter: /.*/ }, (args) => {
        if (path.relative(path.resolve(app), args.path).startsWith(`functions${path.sep}`)) return { errors: [{ text: "Server function sources cannot be imported by a browser entry." }] };
      });
    } }],
    minify: !development, define: { "process.env.NODE_ENV": JSON.stringify(development ? "development" : "production") },
  });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (script === undefined) throw Error("The local builder produced no JavaScript.");
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}

export async function bundleFunctions(app, manifest, development = false) {
  app = await realpath(app);
  const bundles = {};
  const packages = JSON.parse(await readFile(path.join(app, "package.json"), "utf8").catch((error) => {
    if (error.code === "ENOENT") return "{}";
    throw error;
  })).dependencies ?? {};
  for (const [name, version] of Object.entries(packages)) {
    if (typeof version !== "string" || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
      throw Error(`package.json asks for ${name} ${JSON.stringify(version)}. Name an exact version, such as "1.2.3", so every build gets the same code.`);
    }
  }
  for (const [name, declaration] of Object.entries(manifest.functions)) {
    const result = await build({
      entryPoints: [path.join(app, declaration.entry)], absWorkingDir: path.resolve(app), bundle: true, write: false,
      platform: "browser", format: "esm", target: "es2022", outdir: "out", metafile: true,
      minify: !development, logOverride: { "unsupported-dynamic-import": "error", "unsupported-require-call": "error" },
      plugins: [{ name: "declared-server-packages", setup(build) {
        build.onResolve({ filter: /.*/ }, (args) => {
          if (args.kind === "entry-point") return;
          if (!args.importer.includes(`${path.sep}node_modules${path.sep}`) && (args.path.startsWith(".") || path.isAbsolute(args.path))) {
            const resolved = path.resolve(path.dirname(args.importer), args.path);
            if (!resolved.startsWith(path.resolve(app) + path.sep)) return { errors: [{ text: "Server functions can import source files only from app/." }] };
          }
          if (args.path.startsWith(".") || path.isAbsolute(args.path)) return;
          if (args.path.startsWith("node:") || /^[a-z]+:/.test(args.path)) return { errors: [{ text: "Server functions cannot import host modules or URLs." }] };
          const parts = args.path.split("/");
          const dependency = parts.slice(0, args.path.startsWith("@") ? 2 : 1).join("/");
          if (!args.importer.includes(`${path.sep}node_modules${path.sep}`) && !Object.hasOwn(packages, dependency)) {
            return { errors: [{ text: `${args.path} is not in app/package.json dependencies.` }] };
          }
        });
      } }],
    });
    const output = Object.entries(result.metafile.outputs).find(([file]) => file.endsWith(".js"));
    if (!output || output[1].imports.length || !output[1].exports.includes("handle") || result.outputFiles.length !== 1) {
      throw Object.assign(new Error(`Function ${name} bundle must be a self-contained ESM module exporting handle.`), { code: "function.bundle-contract" });
    }
    bundles[name] = result.outputFiles[0].text;
  }
  return { version: 1, abi: "konae-functions/1", manifest, bundles };
}
