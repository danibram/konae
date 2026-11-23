const unenv = (name) => `export * from "unenv/node/${name}"; export { default } from "unenv/node/${name}";`;
const stub = (name) => `throw Object.assign(new Error("Konae does not support ${name}."), { code: "ERR_KONAE_UNSUPPORTED_NODE_API" });`;

export const providers = {
  buffer: `import { Buffer, SlowBuffer } from "unenv/node/internal/buffer/buffer";
    export { Buffer, SlowBuffer };
    export default { Buffer, SlowBuffer };
  `,
  events: `import events from "events/";
    export const { EventEmitter, once } = events;
    export default events;
  `,
  util: unenv("util"),
  "util/types": unenv("util/types"),
  path: `import path from "unenv/node/path";
    export { join, normalize, dirname, basename, extname, relative, isAbsolute, parse, format, sep, delimiter, posix } from "unenv/node/path";
    export default { ...path, win32: undefined };
  `,
  "path/posix": 'export * from "node:path"; export { default } from "node:path";',
  url: unenv("url"),
  querystring: unenv("querystring"),
  assert: unenv("assert"),
  "assert/strict": unenv("assert/strict"),
  string_decoder: `export class StringDecoder {
      constructor(encoding = "utf8") {
        if (!["utf8", "utf-8"].includes(encoding.toLowerCase())) { ${stub("StringDecoder encoding")} }
        this.encoding = "utf8";
        this.decoder = new TextDecoder("utf-8", { ignoreBOM: true });
      }
      write(bytes) { return this.decoder.decode(bytes, { stream: true }); }
      end(bytes) { return (bytes === undefined ? "" : this.write(bytes)) + this.decoder.decode(); }
    }
    export default { StringDecoder };
  `,
  stream: `import stream from "readable-stream";
    export default stream;
    export { Readable, Writable, Duplex, Transform, PassThrough, pipeline, finished, Stream } from "readable-stream";
  `,
  "stream/promises": `import promises from "readable-stream/lib/stream/promises.js";
    export const { pipeline, finished } = promises;
    export default promises;
  `,
  "stream/web": `export const { ReadableStream, WritableStream, TransformStream } = globalThis;
    export default { ReadableStream, WritableStream, TransformStream };
  `,
  crypto: 'export * from "./crypto.mjs"; export { default } from "./crypto.mjs";',
  process: `export const env = Object.freeze(Object.create(null));
    export const nextTick = (callback, ...args) => queueMicrotask(() => callback(...args));
    export default { env, nextTick };
  `,
  timers: `export const setTimeout = (...args) => globalThis.setTimeout(...args);
    export const clearTimeout = (...args) => globalThis.clearTimeout(...args);
    export const setInterval = (...args) => globalThis.setInterval(...args);
    export const clearInterval = (...args) => globalThis.clearInterval(...args);
    export default { setTimeout, clearTimeout, setInterval, clearInterval };
  `,
  "timers/promises": `export const setTimeout = (ms, value) => new Promise(resolve => globalThis.setTimeout(resolve, ms, value));
    export default { setTimeout };
  `,
  async_hooks: `export class AsyncLocalStorage { constructor() { ${stub("AsyncLocalStorage")} } }
    export class AsyncResource { constructor() { ${stub("AsyncResource")} } }
    export default { AsyncLocalStorage, AsyncResource };
  `,
};

const bare = new Set([
  ...Object.keys(providers), "assert", "async_hooks", "child_process", "cluster",
  "console", "constants", "crypto", "dgram", "dns", "domain", "fs", "http", "http2",
  "https", "module", "net", "os", "path", "perf_hooks", "punycode", "readline", "repl",
  "sys", "tls", "trace_events", "tty", "v8", "vm", "wasi", "worker_threads", "zlib",
]);

export const refusal = (name) => ({
  code: "function.node-module-unsupported",
  message: `Server functions cannot import the Node.js module node:${name}.`,
});

export const globals = "konae:node-globals";
const globalsSource = 'export { Buffer } from "node:buffer"; export { default as process } from "node:process";';

function builtin(specifier, importer) {
  if (specifier.startsWith("node:")) return specifier.slice(5);
  if (importer.startsWith("https://esm.sh/")) {
    const url = new URL(specifier, importer);
    const cdn = url.origin === "https://esm.sh" && url.pathname.match(/^\/node\/(.+)\.mjs$/);
    if (cdn) return cdn[1];
  }
  if (specifier.endsWith("/")) return;
  return bare.has(specifier.split("/")[0]) ? specifier : undefined;
}

export function nodeCompat(load) {
  return {
    name: "konae-node-compat-spike",
    setup(build) {
      build.onResolve({ filter: /.*/ }, ({ path, importer, kind }) => {
        if (path === globals) return { path, namespace: "node-compat" };
        if (kind === "entry-point") return;
        const name = builtin(path, importer);
        if (name === undefined) return;
        if (Object.hasOwn(providers, name)) {
          return { path: name, namespace: kind === "require-call" ? "node-compat-require" : "node-compat", sideEffects: false };
        }
        const { code, message } = refusal(name);
        return { errors: [{ text: message, detail: { code, message } }] };
      });
      build.onLoad({ filter: /.*/, namespace: "node-compat-require" }, ({ path }) => ({
        contents: `import mod from "node:${path}"; module.exports = mod;`,
        loader: "js",
      }));
      build.onLoad({ filter: /.*/, namespace: "node-compat" }, async ({ path }) => ({
        contents: path === globals ? globalsSource : await load(path),
        loader: "js",
      }));
    },
  };
}
