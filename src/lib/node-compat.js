import { providers } from "./node-compat-providers.js";

const builtins = new Set([
  ...providers.keys(), "assert", "async_hooks", "child_process", "cluster", "console", "constants",
  "crypto", "dgram", "dns", "domain", "fs", "http", "http2", "https", "module", "net",
  "inspector", "os", "path", "perf_hooks", "punycode", "querystring", "readline", "repl", "sea", "sqlite", "stream",
  "string_decoder", "sys", "timers", "tls", "trace_events", "tty", "url", "util", "v8",
  "test", "vm", "wasi", "worker_threads", "zlib",
]);

const globals = "konae:node-globals";
const globalsSource = 'export { Buffer } from "node:buffer"; export { default as process } from "node:process";';

function builtinName(specifier, importer) {
  if (specifier.startsWith("node:")) return specifier.slice(5);
  if (importer.startsWith("https://esm.sh/")) {
    try {
      const url = new URL(specifier, importer);
      const match = url.origin === "https://esm.sh" && url.pathname.match(/^\/node\/(.+)\.mjs$/);
      if (match) return match[1];
    } catch { return undefined; }
  }
  if (specifier.endsWith("/")) return undefined;
  const root = specifier.split("/")[0];
  return builtins.has(root) ? specifier : undefined;
}

export function nodeCompatPlugin(loadProvider = async (name) => providers.get(name)) {
  return {
    name: "konae-node-compat",
    setup(build) {
      build.onResolve({ filter: /.*/ }, ({ path, importer, kind }) => {
        if (path === globals) return { path, namespace: "konae-node-compat" };
        if (kind === "entry-point") return;
        const name = builtinName(path, importer);
        if (name === undefined) return;
        if (providers.has(name)) {
          return { path: name, namespace: kind === "require-call" ? "konae-node-compat-require" : "konae-node-compat", sideEffects: false };
        }
        const message = `Server functions cannot import the Node.js module node:${name}.`;
        return { errors: [{ text: message, detail: { code: "function.node-module-unsupported", message } }] };
      });
      build.onLoad({ filter: /.*/, namespace: "konae-node-compat-require" }, ({ path }) => ({
        contents: `import mod from "node:${path}"; module.exports = mod;`, loader: "js",
      }));
      build.onLoad({ filter: /.*/, namespace: "konae-node-compat" }, async ({ path }) => ({
        contents: path === globals ? globalsSource : await loadProvider(path), loader: "js",
      }));
    },
  };
}

export function nodeCompatGlobals() { return globals; }

export function nodeCompatBuildError(error) {
  const item = error?.errors?.find((entry) => entry.detail?.code === "function.node-module-unsupported");
  const details = item?.detail;
  if (details) return Object.assign(new Error(details.message), {
    code: details.code,
    detail: item.location ? { importer: item.location.file, line: item.location.line, column: item.location.column } : undefined,
  });
  return error;
}
