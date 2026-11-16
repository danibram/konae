import { build } from "esbuild";
import path from "node:path";

export async function bundleModule(app, entry, development = false) {
  const result = await build({
    entryPoints: [path.join(app, entry)], absWorkingDir: path.resolve(app), bundle: true, write: false,
    platform: "browser", format: "esm", target: "es2022", outdir: "out", jsx: "automatic",
    minify: !development, define: { "process.env.NODE_ENV": JSON.stringify(development ? "development" : "production") },
  });
  const script = result.outputFiles.find((file) => file.path.endsWith(".js"))?.text;
  if (script === undefined) throw Error("The local builder produced no JavaScript.");
  return { script, styles: result.outputFiles.find((file) => file.path.endsWith(".css"))?.text ?? "" };
}
