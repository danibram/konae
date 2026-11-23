import keys from "./test-jwks.json" with { type: "json" };

export const cases = {
  zod: {
    source: `import { z } from "zod";
      export const handle = () => Response.json(z.object({ n: z.coerce.number() }).parse({ n: "7" }));`,
    expected: { n: 7 },
  },
  hono: {
    source: `import { Hono } from "hono";
      const app = new Hono().get("/api/hono", (c) => c.json({ route: "hono" }));
      export const handle = (request) => app.fetch(request);`,
    expected: { route: "hono" },
  },
  dateFns: {
    source: `import { addDays, formatISO } from "date-fns";
      export const handle = () => Response.json(formatISO(addDays(new Date(2024, 1, 28, 12), 1), { representation: "date" }));`,
    expected: "2024-02-29",
  },
  uuid: {
    source: `import { v4, validate, version } from "uuid";
      export const handle = () => { const id = v4(); return Response.json(validate(id) && version(id) === 4); };`,
    expected: true, runnerGap: true,
  },
  nanoid: {
    source: `import { nanoid } from "nanoid";
      export const handle = () => { const id = nanoid(); return Response.json(/^[a-zA-Z0-9_-]{21}$/.test(id) && id !== nanoid()); };`,
    expected: true, runnerGap: true,
  },
  jose: {
    source: `import { SignJWT, importJWK, jwtVerify } from "jose";
      export const handle = async () => {
        const results = {};
        for (const alg of ["ES256", "RS256"]) {
          const keys = ${JSON.stringify(keys)}[alg];
          const privateKey = await importJWK(keys.privateKey, alg);
          const publicKey = await importJWK(keys.publicKey, alg);
          const token = await new SignJWT({ sub: "konae" }).setProtectedHeader({ alg }).sign(privateKey);
          const { payload } = await jwtVerify(token, publicKey, { algorithms: [alg] });
          const segments = token.split(".");
          segments[2] = (segments[2][0] === "A" ? "B" : "A") + segments[2].slice(1);
          const forged = segments.join(".");
          let forgedRejected = false;
          try { await jwtVerify(forged, publicKey, { algorithms: [alg] }); }
          catch { forgedRejected = true; }
          if (!forgedRejected) throw new Error("forged JWT verified");
          results[alg] = payload.sub;
        }
        return Response.json(results);
      };`,
    expected: { ES256: "konae", RS256: "konae" }, runnerGap: true,
  },
  bufferEvents: {
    source: `import { Buffer as SafeBuffer } from "safe-buffer";
      import { EventEmitter } from "node:events";
      import { Buffer as TypedBuffer } from "node:buffer";
      export const handle = () => {
        const ee = new EventEmitter();
        const bytes = [];
        ee.once("bytes", (value) => bytes.push(value.toString("hex")));
        ee.emit("bytes", SafeBuffer.from("é"));
        ee.emit("bytes", Buffer.from("x"));
        return Response.json({ bytes, same: Buffer === SafeBuffer && Buffer === TypedBuffer });
      };`,
    expected: { bytes: ["c3a9"], same: true },
  },
  stream: {
    source: `import { PassThrough as PackageStream } from "readable-stream";
      import { PassThrough } from "node:stream";
      async function roundTrip(Stream) {
        const stream = new Stream();
        const chunks = [];
        return new Promise((resolve, reject) => {
          stream.on("data", (chunk) => chunks.push(chunk.toString()));
          stream.on("end", () => resolve(chunks.join("")));
          stream.on("error", reject);
          stream.end(Buffer.from("stream"));
        });
      }
      export const handle = async () => Response.json([await roundTrip(PackageStream), await roundTrip(PassThrough)]);`,
    expected: ["stream", "stream"], runnerGap: true,
  },
  createHash: {
    source: `import ssri from "ssri";
      import { createHmac } from "node:crypto";
      export const handle = () => Response.json({
        hash: ssri.fromData("abc", { algorithms: ["sha256"] }).hexDigest(),
        hmac: createHmac("sha256", "key").update("abc").digest("hex"),
      });`,
    expected: {
      hash: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
      hmac: "9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab",
    },
  },
  dependencyMode: {
    source: `import invariant from "tiny-invariant";
      export const handle = () => {
        try { invariant(false, "only in development"); }
        catch (error) { return Response.json({ error: error.message, env: Object.keys(process.env) }); }
      };`,
    expected: { error: "Invariant failed", env: [] },
  },
  fs: {
    source: `import fs from "graceful-fs";
      export const handle = () => new Response(typeof fs.readFile);`,
    refused: "fs",
  },
  web: {
    source: `import { randomBytes, randomUUID } from "node:crypto";
      import { setTimeout as delay } from "node:timers/promises";
      export const handle = async () => {
        const sample = new Uint8Array([97, 98, 99]);
        const key = await crypto.subtle.importKey("raw", sample, { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
        const mac = await crypto.subtle.sign("HMAC", key, sample);
        const digest = await crypto.subtle.digest("SHA-256", sample);
        const started = performance.now();
        const cloned = structuredClone(new Map([["n", 7]]));
        const cycle = new Map(); cycle.set("self", cycle);
        const transferable = new ArrayBuffer(4);
        const transferred = structuredClone({ cycle, transferable }, { transfer: [transferable] });
        let callableRejected = false;
        try { structuredClone(() => {}); } catch (error) { callableRejected = error.name === "DataCloneError"; }
        const timer = setInterval(() => {}, 2);
        try { await delay(3); } finally { clearInterval(timer); }
        return Response.json({
          digest: Buffer.from(digest).toString("hex"),
          hmac: await crypto.subtle.verify("HMAC", key, mac, sample),
          random: randomBytes(65537).length === 65537 && randomUUID().length === 36,
          cloned: cloned.get("n"),
          cycle: transferred.cycle.get("self") === transferred.cycle,
          detached: transferable.byteLength === 0,
          callableRejected,
          clock: performance.now() >= started,
        });
      };`,
    expected: { digest: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", hmac: true, random: true, cloned: 7, cycle: true, detached: true, callableRejected: true, clock: true },
    runnerGap: true,
  },
  modules: {
    source: `import processModule, { env, nextTick } from "node:process";
      import { join } from "node:path";
      import { URL } from "node:url";
      import { format } from "node:util";
      import { strictEqual } from "node:assert";
      import { StringDecoder } from "node:string_decoder";
      import { parse } from "node:querystring";
      import { AsyncLocalStorage } from "node:async_hooks";
      export const handle = async () => {
        strictEqual(process, processModule);
        strictEqual(process.env, env);
        const decoder = new StringDecoder("utf8");
        const text = decoder.write(Buffer.from([0xc3])) + decoder.end(Buffer.from([0xa9]));
        let asyncHooks;
        try { new AsyncLocalStorage(); } catch (error) { asyncHooks = error.code; }
        return Response.json({
          env: Object.keys(env),
          mode: process.env.NODE_ENV ?? null,
          next: await new Promise(resolve => nextTick(resolve, 7)),
          path: join("a", "../b"),
          host: new URL("https://example.test/").host,
          format: format("%s", text),
          query: parse("a=1&a=2").a,
          asyncHooks,
        });
      };`,
    expected: { env: [], mode: null, next: 7, path: "b", host: "example.test", format: "é", query: ["1", "2"], asyncHooks: "ERR_KONAE_UNSUPPORTED_NODE_API" },
  },
};

const rejectedSpecifiers = {
  bareFs: "fs", fsPromises: "node:fs/promises", net: "node:net", tls: "node:tls",
  http: "node:http", https: "node:https", http2: "node:http2", childProcess: "node:child_process",
  workerThreads: "node:worker_threads", dgram: "node:dgram", cluster: "node:cluster",
  vm: "node:vm", unknownNode: "node:unknown",
};
for (const [name, specifier] of Object.entries(rejectedSpecifiers)) {
  cases[name] = {
    source: `export * from ${JSON.stringify(specifier)}; export const handle = () => new Response("unused");`,
    refused: specifier.replace(/^node:/, ""),
  };
}

export function filesFor(names, packageText) {
  const functions = Object.fromEntries(names.map((name) => [name, {
    entry: `functions/${name}.ts`, mount: `/api/${name}`, methods: ["GET"], access: "anonymous",
  }]));
  return {
    "index.html": "<!doctype html><html><body>Node compatibility spike</body></html>",
    "package.json": packageText,
    "functions.json": JSON.stringify({ version: 1, functions }),
    ...Object.fromEntries(names.map((name) => [functions[name].entry, cases[name].source])),
  };
}

export const settings = {
  id: "nodecompat", name: "Node compatibility", slug: "nodecompat",
  kernel: "0.12.1", visibility: "public",
  signupPolicy: { mode: "closed", allowlist: [] }, signInMethods: ["password"],
  manifest: { version: 1, visibility: "public" },
  modules: { auth: { on: true }, files: { on: false }, functions: { on: true } },
};
