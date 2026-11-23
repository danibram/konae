# Node compatibility for server functions

Proposed. An app folder carries its source, exact npm dependencies and self-contained Server Artifact. The destination Kernel runs the bundle with either runner. Node compatibility changes what the two builders put in that bundle. It adds Web APIs to the runners and leaves app accounts, rows and permissions under the Kernel.

## Module contract

The scope is server functions. Browser pages retain their current package rules. Recognize `node:` names, equivalent bare built-ins, and esm.sh's `/node/*.mjs` imports from package modules. Treat an explicit npm reference such as `process/` as an npm package. Case matters. Unknown `node:` names get the build refusal.

| Module | Provider and supported behavior |
| --- | --- |
| `buffer` | Build-time `unenv@2.0.0-rc.20` Buffer and SlowBuffer. The `Buffer` global and import share an instance of the implementation. Use the pinned inner Buffer module. The public entry binds `atob` and `btoa` at load time and includes unrelated stubs. |
| `events` | Build-time `events@3.3.0`. EventEmitter, including the callable constructor expected by older CommonJS packages, and `once`. unenv's class cannot serve as the base constructor for those packages. |
| `stream`, `stream/promises` | Build-time `readable-stream@4.7.0`. Start with Readable, Writable, Duplex, Transform, PassThrough, pipeline and finished. The unenv stream shim has inert methods, including a Readable.from that produces no data. |
| `stream/web` | Build-time exports of the runner's existing ReadableStream, WritableStream and TransformStream. |
| `util`, `util/types` | Build-time unenv. Promise adapters, formatting, text codecs and type predicates. List the supported exports. Omit unenv's placeholder methods. |
| `path`, `path/posix` | Build-time unenv with POSIX string operations. Expose join, normalize, dirname, basename, extname, relative, isAbsolute, parse and format. Omit Win32 and host working-directory assumptions. |
| `url` | Build-time unenv. The URL and URLSearchParams constructors come from the runner. URL parsing and file URL conversion operate on strings. |
| `crypto` | Build-time facade. Support synchronous incremental createHash and createHmac for SHA-1, SHA-256, SHA-384 and SHA-512 through `@noble/hashes@1.8.0`. Support getHashes. WebCrypto's async digest/sign cannot implement synchronous Node calls. Delegate randomBytes and randomUUID to the runner's WebCrypto. Split randomBytes into calls of at most 65,536 bytes and return Buffer. Test the callback form as well. |
| `assert`, `assert/strict`, `querystring` | Build-time unenv. Exercise strict equality, deep equality, escaping and repeat keys. List and test the shipped exports. |
| `string_decoder` | Build-time UTF-8 StringDecoder facade over the runner's streaming TextDecoder. The unenv constructor is a stub. Refuse other encodings on use with `ERR_KONAE_UNSUPPORTED_NODE_API`. |
| `process` | Build-time facade. `env` is a frozen empty object. `nextTick(fn, ...args)` uses queueMicrotask with FIFO microtask ordering. Inject the same object for the free `process` identifier. The app cannot read the installation environment. |
| `timers`, `timers/promises` | Build-time facade over native Web timers. Support setTimeout, clearTimeout, setInterval, clearInterval and promise setTimeout with an optional value. Use numeric Web timer handles. |
| `async_hooks` | Import-only stub for AsyncLocalStorage and AsyncResource. Construction throws `ERR_KONAE_UNSUPPORTED_NODE_API`. The unenv fallback loses the store across await. An actual async context API would need both engines to propagate it through promises and timer jobs. |
| `fs`, `fs/promises`, `net`, `tls`, `http`, `https`, `http2`, `child_process`, `worker_threads`, `dgram`, `cluster`, `vm` and unlisted built-ins | Build refusal, including subpaths. App code does not open host files, sockets, servers, workers or other processes. |

Use explicit exports and the same default object or callable value for CommonJS. Test ESM named imports, ESM default imports and `require()`. Pin the full provider closure. It includes `unenv@2.0.0-rc.20` and the exact dependencies in the spike lockfile.

## Both builders

Install the compatibility resolver ahead of the server-only file and package resolvers. Preserve the existing local source boundaries and the final ESM and size checks. Keep every supported import inside the bundle.

- In `src/lib/local-builder.js`, resolve the allowlist from selected entries in the pinned provider's node_modules, with explicit facades for the rows above. Carry the generated resolver and provider with the app folder's build script in `src/lib/app-folder.ts`. Include them in the embedded CLI build and in the stale script refresh. Existing app folders can rebuild after a CLI upgrade. An app that needs only built-ins can build with the embedded provider. Cover `konae build`, `konae dev`, publish after rebuild and standalone `npm run build`.
- In `src/runtime/browser-build.ts`, vendor the same provider bytes. Build them from the pinned packages once with the editor. This avoids a CDN deciding which unenv exports are real, and keeps Node imports independent of the network when the app imports no npm package. App packages still use `packagesPlugin` and esm.sh. Match browser entry and subpath selection for npm packages before applying the Node rule. For example, a package can replace a Node dependency with a browser entry. Intercept the CDN's Node modules at resolution, including replacements for free Buffer and process. Apply the mapping only to imports from the esm.sh package namespace. The spike observed a 404 from the CDN for the pinned `unenv/node/stream` subpath.
- Inject free Buffer and process references in server bundles. Do not write to globalThis. Map the free `global` identifier to globalThis. Set `define: { process: "process" }` on the final server pass to disable esbuild's implicit `process.env.NODE_ENV` substitution. App code sees an empty environment. esm.sh folds `process.env.NODE_ENV` to production in dependency code. Mirror that single compile-time constant in the CLI npm loader in both dev and release. Keep other environment reads pointed at the empty object. Pin direct package versions and use the app's lockfile on a local rebuild. The CDN can pick other transitive versions or a different package browser entry. Parity here means the supported APIs and tested package behavior. It needs a fixture whenever dependence on an entry or transitive version changes. [esm.sh build options](https://github.com/esm-dev/esm.sh/blob/main/server/build.go)
- The editor release code sends the result from the browser builder. The CLI release code sends the result from the local builder. `src/deployments/release.ts` validates and transports the Server Artifact. It does no npm resolution. `src/lib/build-project.ts` handles authored build projects and needs no Node resolver of its own.

## Build refusal

Use code `function.node-module-unsupported`, hint `fix: capsule`, and the sentence `Server functions cannot import the Node.js module node:<name>.` Substitute the canonical name including a subpath. Unwrap structured esbuild error details at the Gate and CLI boundary. Keep file and package provenance in diagnostic details separate from the sentence.

The builders check every import they resolve, including literal dynamic imports, re-exports and CommonJS dependencies. The Gate also checks reachable project sources on the declared function graph. Use Babel to parse TS and JavaScript. The Kernel uses its Oxc parser on those same app-local sources when it checks a release or imports the folder. Both also check for leftover unsupported Node specifiers in the submitted bundle before the generic self-contained ESM check. A leftover supported specifier still fails the generic check. Skip type-only imports and lexically shadowed require calls. A string or comment with the letters `node:fs` is ordinary text. Both sides use deterministic source traversal and the same refusal precedence.

The Kernel does not have npm source or a package resolver. It can check supplied app sources and the actual bundle. A disallowed transitive import in a dependency the builders resolve fails before an artifact exists. If the package's selected browser entry contains no such import, the Kernel does not infer one from its package name. The worker's existing import boundary still applies to every loaded bundle.

N1 owns `kernel-rs/kernel/src/functions-node-module-cases.json`, the refusal registry entry, and both consumers. Include bare and prefixed imports, a relative helper, re-export, literal dynamic import, CommonJS require, an allowed module, an unreferenced source, string/comment decoys and refusal precedence. Exercise the transitive `graceful-fs` and CDN URL forms through the builders. The spike checks resolver parity. It does not change Kernel validation.

## Web APIs and request lifetime

Both runners expose callback-only setTimeout and setInterval, their clear methods, and queueMicrotask. Keep timers callable only in an active request. For delayed work use the existing request deadline, with no background work, network fetch, new permissions or extra grace for timers. Cancel and revoke timer callbacks at response completion, request abort, timeout or Store/isolate disposal. If the runner retires a request's realm for shutdown, clear its request timers first. Give shutdown only its existing grace. `performance.now()` uses a monotonic request clock. `structuredClone` must clone a cyclic Map, transfer an ArrayBuffer with detachment and reject a callable with DataCloneError.

WebCrypto supplies getRandomValues, randomUUID and SubtleCrypto digest, importKey, sign and verify. First algorithms are SHA-1/SHA-2 digest, HMAC with SHA-256, ECDSA P-256 with SHA-256 and RSASSA-PKCS1-v1_5 with SHA-256. The JWT fixture imports JWKs, signs and verifies with both algorithms. Expose CryptoKey with the same brand as returned keys. Keep crypto entropy and key operations request-only. Top-level declarations may capture guarded function references and run pure JavaScript hashes. They cannot perform entropy, WebCrypto, timer or fetch effects. Use `unsupported_runtime_api` for rejected initialization effects and make a caught exception insufficient to publish a Wizer artifact.

### deno_core

Use the existing `deno_web@0.291.0` modules `02_timers.js`, `13_message_port.js` and `15_performance.js`. Export only the required globals. Clone with transfers needs the message-port implementation of structuredClone. The core already installs queueMicrotask. deno_web's raw timer functions accept strings through indirect eval. Bind a wrapper that rejects every non-function callback. Preserve `AbortSignal.timeout()` as a refusal.

Start with `deno_crypto@0.274.0`, which depends on the same `deno_core@0.413.0`. Register the extension in both `build.rs` and `src/runtime.rs`, in the same order and with no seed. Load the extension script in the snapshot, then defer creation of native crypto objects until a request uses them. Loading the script only after restoring the snapshot failed because the core omitted the lazy source from the binary. Reading the crypto singleton during snapshot creation panicked in deno_core because the snapshot isolate had no CppGC heap. Keep key material and entropy out of the snapshot. Capture trusted callbacks in the bootstrap before deleting Deno internals. Test the supported subset under the existing Linux containment policy. Rust 1.91 rejects four `if let` guards in this crate with E0658. The previous deno_crypto pin targets deno_core 0.412.0. The scratch build rewrote the four guards as stable `is_some()` branches and bound the same variant inside each branch. N2 needs an equivalent scoped patch or an upstream release compatible with the pinned core and toolchain. The extension links algorithms beyond the contract, including AWS-LC. Make no syscall or capability policy change without an observed need. [deno_crypto package](https://docs.rs/crate/deno_crypto/0.274.0/source/)

Use the normal Rust snapshot build path. Measure the before and after stripped release executables and generated `startup_snapshot.bin` with `wc -c`, on the same architecture and profile. The Web bindings come from crates already in the runner. The main expected link cost comes from deno_crypto. An estimate from the dependency list cannot give a reliable byte delta after linking.

### StarlingMonkey

Start after Luna B1. Keep the runner and ComponentizeJS engine pin from that branch. The current builder disables `random` and `clocks` as well as `stdio`, `http` and `fetch-event`. The current guest deletes timers, crypto and performance. Reintroduce guarded native bindings and enable only the clock, poll and secure random imports they need. Link those explicit imports with bounded clocks and request-phase entropy. Avoid wiring a general WASI context. Fail preparation on disallowed top-level calls, including aliases captured through an import. A trusted initialization flag records a forbidden call even when the source catches the error. Check the flag before accepting the Wizer output. Install the guards before the app module evaluates. An assignment in the entry's module body runs after static dependencies have evaluated.

Keep Wizer deterministic. No real time, entropy, polling or I/O runs during Wizer initialization. Use the existing monotonic deadline and epoch interruption for guest code. A blocked host poll must wake on Cancel or the same deadline, since epoch interruption cannot preempt a blocking native import. Track timers in the request's Store and cancel native tasks when the handler finishes, including intervals created in timer callbacks. Drop the Store after retirement. Keep the component import allowlist, engine fingerprint and artifact/cache version consistent with the change.

The pinned engine exposes getRandomValues and randomUUID through host entropy. Its SubtleCrypto source lists digest, importKey, sign and verify and includes HMAC, ECDSA and RSASSA-PKCS1-v1_5. It exposes no generateKey. This is source evidence. Run the JWK fixture on the real component to check import and signature format. [Pinned crypto binding](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/crypto/subtle-crypto.cpp), [pinned algorithms](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/crypto/crypto-algorithm.cpp), [pinned timers](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/timers.cpp), [ComponentizeJS feature controls](https://github.com/bytecodealliance/ComponentizeJS#componentizejs-options).

## Cloudflare Workers

Cloudflare's current matrix lists native Buffer, Events, crypto, streams, AsyncLocalStorage, virtual fs, net and HTTP, partial TLS and compatibility-date-controlled stubs. Wrangler combines workerd's native Node APIs with unenv where needed. Konae exposes Web APIs on the runners and bundles the Node facades. Its build refuses filesystem and socket modules. It has no compatibility date switch, Node HTTP server or automatic population of process.env from app bindings. Konae's AsyncLocalStorage stub is smaller than workerd's implementation. The narrower scope keeps the same app folder runnable on either Konae runner. [Workers compatibility matrix](https://developers.cloudflare.com/workers/runtime-apis/nodejs/), [Workers crypto](https://developers.cloudflare.com/workers/runtime-apis/nodejs/crypto/), [Wrangler dependencies](https://github.com/cloudflare/workers-sdk/blob/main/packages/wrangler/package.json).

## Acceptance functions

[Cases and response assertions](../../spikes/node-compat/cases.mjs) fix versions and inputs in the spike. Every successful row expects HTTP 200. Random values become booleans so the two outputs can be compared without fixing an entropy seed.

| Package | Function and expected result |
| --- | --- |
| `zod@4.1.13` | `zod` coerces `"7"` and returns `{"n":7}`. |
| `hono@4.13.19` | `hono` dispatches to `/api/hono` and returns `{"route":"hono"}`. |
| `date-fns@4.1.0` | `dateFns` adds one day and returns `"2024-02-29"`. |
| `uuid@13.0.0` | `uuid` generates and validates a v4 UUID and returns `true`. |
| `nanoid@5.1.6` | `nanoid` checks the size, alphabet and difference of consecutive IDs and returns `true`. |
| `jose@6.1.3` | `jose` imports test-only JWK pairs and signs and verifies a JWT. It returns `{"ES256":"konae","RS256":"konae"}`. |
| `safe-buffer@5.2.1` | `bufferEvents` emits UTF-8 bytes once with EventEmitter and checks Buffer identity. It returns `{"bytes":["c3a9"],"same":true}`. |
| `readable-stream@4.7.0` | `stream` writes Buffer to a package PassThrough and a `node:stream` PassThrough. It returns `["stream","stream"]`. Both stream constructors inherit from EventEmitter. |
| `ssri@8.0.1` | `createHash` uses transitive `crypto.createHash` through SSRI plus a direct HMAC call. It checks both SHA-256 vectors. |
| `tiny-invariant@1.3.3` | `dependencyMode` checks the package's production error message and app environment. It returns `{"error":"Invariant failed","env":[]}`. |
| `graceful-fs@4.2.11` | `fs` requires filesystem access. Both builders refuse with `function.node-module-unsupported` and name `node:fs`. |

Two additional functions check the Web APIs and remaining Node modules. `web` checks WebCrypto digest and HMAC, randomBytes across the 65,536-byte boundary, UUID, timers, structured clone and the clock. `modules` checks process identity, empty env, nextTick, POSIX path, URL, util.format, assertions, split UTF-8, repeated query keys and the AsyncLocalStorage error code.


## Spike results

On macOS arm64, the [spike commands](../../spikes/node-compat/README.md) ran the CLI builder against installed npm packages and the editor builder in Chrome against live esm.sh. Both artifacts passed all 12 positive cases in Chrome. The two builders returned the same refusal for 14 cases, including a transitive CommonJS fs import, bare fs, fs/promises and an unknown prefixed module. All positive outputs passed the existing Server Artifact validation. Bytes varied. The SSRI case used 153,844 bytes from the CLI and 155,184 bytes from the editor with the dependency mode probe enabled.

The stock IPC/3 deno_core runner served the CLI folder through `konae dev` and the editor folder through a separate Kernel. In each lane, `zod`, `hono`, `dateFns`, `bufferEvents`, `createHash`, `dependencyMode` and `modules` returned HTTP 200 with their expected bodies. The remaining five returned HTTP 500 `handler_error`.

| Route | Missing Web API in the stock runner, based on code |
| --- | --- |
| `uuid`, `nanoid` | WebCrypto randomness |
| `jose` | WebCrypto key import, sign and verify |
| `stream` | Timers used by the browser process shim |
| `web` | WebCrypto, timers, performance and structured clone |

The baseline `GET /api/createHash` returned the same JSON body in both installations.

```json
{"hash":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","hmac":"9c196e32dc0175f86f4b1cb89289d6619de6bee699e4c378e68309ed97a1a6ab"}
```

A scratch deno_core build added the Web bindings and the patched crypto extension described above. It kept the extension script in the snapshot and used a getter to create the native crypto singleton in the isolate. All 24 HTTP requests to the two folders returned the expected HTTP 200 bodies. Both `GET /api/jose` requests returned `{"ES256":"konae","RS256":"konae"}`. Both `GET /api/web` requests verified the HMAC, cloned the Map, ran the timers and returned the SHA-256 digest. The scratch runner proved API availability. N2 still owns request-phase guards, timer cleanup, clone transfer tests and Linux containment. The StarlingMonkey design comes from source inspection.

I built both macOS arm64 binaries with Rust 1.91 and `cargo build --release` using the runner's `strip = "debuginfo"` profile. I measured the linked executables and the matching `target/release/build/konae-functions-runner-*/out/startup_snapshot.bin` files with `wc -c`. The snapshot is included in the executable.

| Measured file | Stock, bytes | Scratch, bytes | Increase, bytes |
| --- | ---: | ---: | ---: |
| `konae-functions-runner` | 84,694,768 | 88,852,784 | 4,158,016 |
| `startup_snapshot.bin` | 1,844,395 | 1,918,741 | 74,346 |
