import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import { providers } from "./node-compat-provider-definitions.mjs";

const directory = fileURLToPath(new URL(".", import.meta.url));

export async function bundleProviders() {
  const result = await Promise.all(Object.entries(providers).map(async ([name, contents]) => {
    const { outputFiles } = await build({
      stdin: { contents, resolveDir: directory },
      bundle: true, format: "esm", platform: "browser", target: "es2022",
      external: ["node:*"], write: false, minify: true, legalComments: "inline",
      plugins: [{
        name: "bundle-buffer-require",
        setup(build) {
          build.onResolve({ filter: /^buffer$/ }, () => ({ path: "buffer", namespace: "buffer" }));
          build.onLoad({ filter: /^buffer$/, namespace: "buffer" }, () => ({
            contents: 'import mod from "node:buffer"; module.exports = mod;',
            loader: "js",
          }));
        },
      }],
    });
    return [name, outputFiles[0].text];
  }));
  return new Map(result);
}
