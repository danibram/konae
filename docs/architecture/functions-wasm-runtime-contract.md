# StarlingMonkey function runtime contract

Konae will use StarlingMonkey for portable Functions. An export carries the built function with its app accounts, data and rules. The destination Kernel runs that artifact without evaluating the app's source again during preparation. It still authorizes each request and Server Action against the app's current configuration. See [Functions](../functions.md), [ADR-0018](../adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md) and [ADR-0019](../adr/0019-functions-stream-request-bodies-with-bounded-ipc.md).

This document specifies the production behavior. The [parity list](../../kernel-rs/functions-runner-wasm/tests/PARITY.md) gives the inputs and expected outputs. The measured baselines are the StarlingMonkey IPC/2 spike at `76d98e32` and the deno_core IPC/3 runner at `0e3bf8e9`. The V8 startup snapshot contains the runtime bootstrap. The Wizer snapshot contains the evaluated app. The [spike report](./functions-wasm-spike.md#build-and-measurement-commands) pins the StarlingMonkey engine and its build tools.

## Module initialization and time

The builder evaluates module-level code once. Each request restores that state in a fresh instance. A counter declared as `let counter = 0` returns 1 on every request if the handler increments it once. An app stores durable changes through Server Actions.

At the top level, `Date.now()`, `new Date()` and the callable `Date()` use one build timestamp embedded in the artifact. Inside a handler or retirement hook, those forms read wall time. Local date formatting uses UTC, as it did in the spike. Explicit dates such as `new Date(0)` keep standard JavaScript semantics. A new build can capture a new timestamp. Installing the same artifact preserves its timestamp.

The deno_core probe returned counter 1 three times and evaluated the top-level clock anew each time. The spike returned counter 1 three times and froze top-level `Date.now()` at zero. Top-level `+new Date()` froze a nonzero timestamp. In the handler, `Date.now()` advanced and `+new Date()` stayed frozen. The spike overrides only `Date.now()` in `guest/starling-runtime.js`.

Use the same clock provider for all implicit Date reads. Set one build timestamp during componentization and carry it in the portable artifact. Switch to the wall-clock import when the runner enters a real invocation or retirement hook. Test two requests at distinct known times and install the same artifact in a second Kernel cache. Check the build timestamp on both installations.

### Initialization refusals

The builder drains finite microtasks before snapshotting. It refuses module evaluation that leaves a pending Promise, attempts to use a timer, fetch, or entropy, or exceeds the build budget. A resolved Promise is allowed. Timer and fetch attempts are refusals even if app code catches the exception. Calls through an alias follow the same rule. An unused function declaration that refers to one of those APIs does not execute the call.

Propose one shared refusal, `function.initialization-cannot-snapshot`, with the message `The function's top-level code must finish without pending promises, timers, fetch, or random values.` A Capsule hint tells the author to move request work into `handle` and use a Call Action for external HTTP. Use the same build outcome in the Validation Gate and Kernel release path. Register the code and run shared refusal cases under `kernel-rs/kernel/src/`. Keep a bounded build timeout and reap the build process group. Report the failed initialization before accepting the artifact. An import verifies the portable artifact under the artifact contract and can use a valid snapshot directly. See [ADR-0004](../adr/0004-the-gate-and-the-kernel-refuse-with-the-same-code-and-words.md).

Use an engine hook to observe the app module's promise creation and settlement during initialization, including promises that never enqueue a job. Distinguish app promises from the componentizer's own initialization. A drained job queue alone can miss `new Promise(() => {})`. Use guarded build-phase entry points to record disallowed calls, then strip those entry points where the production API is absent. Convert engine traps and build timeout into the shared diagnostic. Static detection of bare identifier calls would miss aliases and code in dependencies.

With deno_core the bare pending Promise survived module evaluation and `handle` returned `true`. Pending top-level await, timer and fetch calls failed on invocation with `worker_failed`. The deno_core snapshot lets top-level `Math.random()` execute on each request. The spike built a bare pending Promise and the handler returned `true`. Pending top-level await failed export extraction. The timer and fetch calls raised ReferenceError during preparation. A synchronous loop hit the 60-second build watchdog. StarlingMonkey's direct top-level random probe failed `--prepare` because Wizer could not call `wasi:random/random@0.2.10/get-random-u64`.

### Randomness

Reseed `Math.random()` in every instance before invoking the handler. The deno_core probe drew different pairs across requests. Three restored StarlingMonkey instances drew the same pair, `[0.48401551228016615,0.866216232534498]`. Use a narrow host entropy import and seed the engine's per-realm generator at request entry. Give a retirement-only instance its own seed if it can run app code. Resolve entropy inside the runner and fail closed if it is unavailable. Keep WASI randomness and Web Crypto outside this change. `Math.random()` remains for non-cryptographic use.

In the test host, supply known distinct seeds for successive restores and check that the first draws follow those seeds. Reuse one worker and test two new worker processes. A live smoke test also checks that adjacent requests do not repeat an entire sequence.

## Response memory

Keep the response limit. Transfer bounded byte chunks from the guest to a host accumulator (up to 64 KiB per transfer), then send one IPC/3 Reply. The HTTP response still waits for validation and for the Kernel to finish any action transaction. The worker counts `WorkerLimits.response_bytes` before copying a chunk. The host enforces the same cap and accepts one final result per capability. Validate UTF-8 across chunk boundaries, preserve the existing leading BOM behavior and check the serialized envelope separately. Delete the staged chunks on failure or cancellation.

The spike's `guest/dispatch.js` retains the chunks, allocates a second byte buffer, decodes it, serializes the reply as JSON and lifts the full string through WIT. The deno_core runner also buffers but passed the measured boundary. The spike hit the linear memory limit at both 1 MiB and 2 MiB with default 64 MiB guest memory. A byte over the IPC limit was refused as `response_too_large`. The Kernel's HTTP response body and serialized validation envelope cap is 1 MiB. Exactly 1 MiB of text can still exceed the HTTP envelope cap.

Use a WIT chunk import and return only status and headers at completion. Keep the header cap and the canonical failure path. For an ordinary buffered response and a generated stream, test 256 KiB, 1 MiB, 2 MiB, the configured limit plus one, and invalid UTF-8 split across a transfer. Limit breach wins if the runner has counted enough bytes to establish it. A handler can still exhaust guest memory while generating a reply before the worker can count it.

### Measured response curve

Each cell gives the IPC result and peak process RSS in MiB (bytes reported by macOS `/usr/bin/time -l`, divided by 1,048,576). I started a fresh worker for one request per cell, used a prepared component, set the worker heap and response limits to 64 MiB and 2 MiB, and generated ASCII `x` as a string or in 64 KiB stream chunks. RSS includes the engine and host allocations. It measures resident pages. Wasmtime limits linear memory growth, which can fail at a lower observed RSS.

| Response bytes | deno_core text | StarlingMonkey text | deno_core stream | StarlingMonkey stream |
| ---: | --- | --- | --- | --- |
| 262,144 | Reply, 22.17 | Reply, 19.98 | Reply, 21.46 | Reply, 19.70 |
| 1,048,576 | Reply, 30.00 | Memory refusal, 19.73 | Reply, 29.91 | Memory refusal, 18.31 |
| 2,097,152 | Reply, 39.92 | Memory refusal, 25.77 | Reply, 38.06 | Memory refusal, 23.86 |
| 2,097,153 | Size refusal, 23.38 | Size refusal, 17.91 | Size refusal, 21.23 | Size refusal, 15.55 |

## Multipart files and request streaming

Multipart file data stays binary. A local `Blob` from `Uint8Array.of(0,255)`, the file stored by `FormData.append` preserved bytes in the spike probe. The encoded local body contained the original consecutive bytes 0 and 255. `Request.formData()` produced `[48,50,53,53]` for both an in-guest multipart round trip and a raw multipart IPC request. The in-guest reproduction keeps the file inside the component throughout. deno_core returned `[0,255]` for both round trips. The pinned source explains the conversion. `MultipartParser::parse` hands a Uint8Array directly to `File::create`. The Blob initializer iterates that array as individual numeric parts and converts each number to text. [Multipart parser](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/form-data/form-data-parser.cpp#L151-L205) and [Blob parts](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/blob.cpp#L421-L495)

Pass the parser's byte range to `File` as one binary part. Preserve file metadata and decode the text fields separately. Test `Request.formData()` as well as `Response.formData()` if the engine exposes it. Test empty files, repeated fields, NUL and invalid UTF-8 in file content, and a multipart boundary split across reads. `formData()` can buffer the whole request. A handler that needs to consume a 100 MB body within the default heap reads it incrementally.

Keep ADR-0019's 100,000,000-byte HTTP limit, 64 KiB binary frames and 8 MiB pending budget. Test the real Kernel with the same incremental handler for both engines. The first excessive byte must produce 413 and leave the transaction and receipt untouched. A client that stalls before the limit is detected can still receive 504. Keep per-invocation byte counting and its refusal in the Kernel. Give a recorded size refusal priority over the deadline after the Kernel detects the excess. Trace BodyRead offsets, Wasm memory growth and the native-body failure along the full path before adjusting timeout or memory. The worker should release each consumed chunk and signal EOF explicitly. A direct spike IPC run with the same checksum handler consumed 3,080,192 of 100,000,000 bytes before its 65-second watchdog expired. The Python parent supplied binary frames on demand. The shared Mac was under load. The deno_core probe completed the same 100,000,000-byte IPC request with the correct checksum. On a later run with the rebuilt spike worker, the 1 MiB handler took 34 ms when it counted chunks and 3.129 s when it also summed the bytes with `for...of`. An indexed loop returned the same 1 MiB checksum in 714 ms. Both the count-only and indexed variants then hit `worker_memory_limit` after the parent sent 19,988,480 bytes of a 100 MB request. This happened even when app code retained only the current chunk. Trace allocations at the WIT byte-list lift and the extra `new Uint8Array(chunk)` copy. Recycle bounded buffers when ownership allows it, and profile typed-array iteration separately. Keep the 100 MB checksum assertion and the memory and deadline limits.

## Clones, headers and HTTP framing

- `Response.clone()` must work for buffered bodies and streams. deno_core returned `é` from both copies of a 201 response and `AB` from both branches of a stream. The spike returned `TypeError: a.clone is not a function`. Implement Fetch's body tee in the engine's Response type. Preserve status, status text and independent headers. Refuse a locked or disturbed body with `TypeError`. Hono ETag with a supplied digest reproduced the missing method.
- `Request.clone()` passed the IPC POST probe in both runners. The two copies returned `é`. Cloning a consumed or locked body threw `TypeError`. Changing the clone's headers left the original alone. Keep the engine implementation and the tests, including a split UTF-8 body.
- `Headers` iteration matched in both probes. Given reverse-order names, duplicate mixed-case `b` and duplicate `Set-Cookie`, the order was lowercase and sorted, `b` became `1, 2`, and the cookies stayed distinct. Pin this behavior at the guest and IPC boundaries. [Fetch Standard](https://fetch.spec.whatwg.org/#concept-header-list-sort-and-combine)
- The Kernel owns framing. The IPC/3 executor refuses handler-supplied `Transfer-Encoding` and `Content-Length` as `invalid_response`. deno_core forwarded both as written. StarlingMonkey silently omitted `Transfer-Encoding: chunked` and Rust stripped either a generated or an explicit matching length. An incorrect explicit length survived. StarlingMonkey's `RequestOrResponse::extract_body` inserts the length and its Response header guard can silently drop framing fields. Suppress the engine's automatic length for local Responses and preserve authored framing fields through validation, or reject them in the guest with the same code before any Web API guard drops them. A `new Response('é')` succeeds over HTTP with correct framing. Explicit lengths 2 and 9 and explicit chunked framing all fail. Include headers passed to the constructor or set through `response.headers`, a Hono `streamText` response, and a normal stream with no framing header. [Body extraction](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/fetch/request-response.cpp#L303-L469) and [header guards](https://github.com/bytecodealliance/StarlingMonkey/blob/9dda8ba7fcda2e17c6795d402f0478cf4c1f7f37/builtins/web/fetch/headers.cpp#L356-L379)

## Deadlines and cancellation

A function may use its own `AbortController`. A Kernel deadline does not promise a `request.signal` event, catch block or finally block. The Kernel cancels the invocation and its action capability. An in-flight action may be interrupted in the host or its result may be discarded. The handler cannot rely on receiving any result or running cleanup. Keep the HTTP deadline and mutation outcome rules. A pre-commit cancellation rolls back. A timeout racing with commit can return `409 mutation_outcome_unknown` and requires the same idempotency key on retry.

The deno_core bootstrap attaches a controller and can deliver a cooperative abort on an event-loop tick. A blocked operation can reject, or V8 can stop before the guest processes that event. The spike receives `Cancel` on the reader thread. Wasmtime checks for cancellation at the epoch boundary and the bridge checks it while waiting for the action. There is no guest callback in this path. In the IPC probe a Cancel after the action Operation produced `cancelled`. The next request to the same worker returned 200 with an unaborted signal. The lack of a guest callback follows from the code.

Keep the presence of a normal Request signal for Web API compatibility. Ensure Cancel drops the capability and any staged response and preserves a synchronized worker where the protocol handshake finishes. Test a waiting action, a guest loop and late operation results. After cancellation, the next request must succeed with a fresh Store. The HTTP proof checks the timed-out call, the row and receipt, and a subsequent successful request.

## Handler errors

On IPC/3 an uncaught guest exception returns `Failure { capability, code: "handler_error", detail }`. deno_core formats its JS exception in `src/runtime.rs`, bounds it at a UTF-8 character boundary in `WorkerError::with_detail` and sends it from `src/worker.rs`. The measured `TypeError('fixture detail')` returned the message and three stack frames. A long emoji message returned 4,095 bytes. The integration Kernel keeps the detail in owner Usage and returns the canonical HTTP error to the caller. The spike returned `worker_execution_failed` with no IPC detail for both throws.

Capture the JS exception before StarlingMonkey raises a generic Wasm trap. Send a bounded message and JS stack over the guest bridge. Use 4,096 UTF-8 bytes for the entire field, reserving room for at least one app frame when it exists. Normalize app frames to a portable bundle location. Strip host paths, native backtraces and bindings from frames. Treat the author-controlled message as sensitive. Only owner diagnostics receive it. Never attach request headers, body values, action inputs or host trap text automatically. Discard native detail when a sticky action, deadline or memory refusal supplies the effective code. Test thrown Error, rejection, multibyte truncation, a forged native path in a stack frame, and a public HTTP response with no private detail. Inspect the diagnostic through owner Usage.

## Web APIs

These probes check named behavior. The Fetch and Streams APIs have more cases than these probes cover.

| API | Input and observed results |
| --- | --- |
| `URL`, `URLSearchParams` | Both resolved `../x?b=a+b` relative to `https://example.test/a/y`, decoded repeated `b` parameters and serialized an appended emoji as UTF-8 percent escapes. |
| `TextEncoder`, `TextDecoder` | Both encoded `é🦊`, decoded `[195,169]` as `é` and threw `TypeError` for `[255]` with fatal UTF-8 decoding. |
| `ReadableStream`, `WritableStream`, `TransformStream` | Both read `a` through EOF, wrote `a` then closed, and transformed `a` into `A` with a reader and writer. See the large-body checks for resource behavior. |
| `AbortController` | Both fired one event with the first supplied reason after two calls to `abort`. The request deadline has the separate contract above. |
| `Blob`, `File`, `FormData` | Both preserved typed-array and ArrayBuffer bytes, normalized the file MIME type, kept its name and timestamp, and returned two text values in insertion order. StarlingMonkey corrupted file data on multipart parsing. |
| `Headers`, `Request`, `Response` | Both passed header iteration, a UTF-8 POST and JSON/status checks. The clone and framing differences are above. |
| `structuredClone` | StarlingMonkey cloned a cyclic Map, transferred an ArrayBuffer with detachment and threw `DataCloneError` for a function. deno_core did not expose it. Keep these cases in the suite. |
| `queueMicrotask` | deno_core returned `["sync","micro"]`. The spike strips it in the adapter. Keep it in production and test ordering, callback type errors and the end of module initialization. |
| `crypto.subtle`, `crypto.randomUUID`, `setTimeout`, `performance.now`, `fetch` | Absent in both. Keep them outside this parity change. |
| `DOMException`, `Event`, `EventTarget`, `CustomEvent`, `AbortSignal`, `console` | Both exposed these globals. Both runners deliberately refuse `AbortSignal.timeout()`. |

### Proposed wording for Functions documentation

Use after the parity tests pass. This replaces the IPC/3 `docs/functions.md` paragraph that begins "The runner exposes these Web APIs."

> The runner exposes `DOMException`, `Event`, `EventTarget`, `CustomEvent`, `URL`, `URLSearchParams`, `TextEncoder`, `TextDecoder`, `ReadableStream`, `WritableStream`, `TransformStream`, `AbortController`, `AbortSignal`, `Blob`, `File`, `Headers`, `FormData`, `Request`, `Response`, `structuredClone`, `queueMicrotask`, `console` and standard JavaScript built-ins. It provides no Web Crypto, network `fetch`, timers, `AbortSignal.timeout()`, `performance.now()` or Cache API. The Kernel owns response framing. You can return a bounded UTF-8 response stream. Hono routing and ETag with a supplied digest work after the clone fix. Hono streaming helpers that set framing headers need adaptation.
>
> A function's top-level code runs during the build. Every request starts from the same snapshot. Read the clock inside `handle` for request time and use Server Actions to store durable state. The Kernel can stop execution at its deadline without delivering an abort event to the handler.

### Hono 4.13.19 with Web Crypto

These implications come from the exact Hono version's source. They assume the listed algorithms and API methods work. This slice leaves Web Crypto off.

- `jwt`, locally supplied `jwk` verification keys, and cookie signing and verification use `subtle.importKey`, `sign` or `verify`. Remote JWKS needs `fetch`. Konae passes only `Accept` and `Content-Type` to the handler. Enabling crypto alone gives these middleware no inbound Authorization or Cookie header. Test signed-cookie writing as an outbound operation. The Kernel remains responsible for app session authentication. [Hono JWT](https://github.com/honojs/hono/blob/v4.13.19/src/middleware/jwt/jwt.ts), [JWK](https://github.com/honojs/hono/blob/v4.13.19/src/utils/jwt/jwt.ts) and [cookies](https://github.com/honojs/hono/blob/v4.13.19/src/utils/cookie.ts)
- Default `etag` needs `subtle.digest` (SHA-1) and `Response.clone()`. Its conditional response also needs `If-None-Match`, which the Kernel does not pass. Default `requestId` needs `randomUUID`. The CSP `NONCE` helper in `secureHeaders` needs `getRandomValues`. Plain security headers do not need a nonce. [ETag](https://github.com/honojs/hono/blob/v4.13.19/src/middleware/etag/index.ts), [Request ID](https://github.com/honojs/hono/blob/v4.13.19/src/middleware/request-id/request-id.ts) and [Secure Headers](https://github.com/honojs/hono/blob/v4.13.19/src/middleware/secure-headers/secure-headers.ts)
- `cache` needs a Cache API, `timeout` needs timers, and `stream.sleep` needs timers. A stream with application-supplied framing still fails. [Cache](https://github.com/honojs/hono/blob/v4.13.19/src/middleware/cache/index.ts), [Timeout](https://github.com/honojs/hono/blob/v4.13.19/src/middleware/timeout/index.ts) and [streamText](https://github.com/honojs/hono/blob/v4.13.19/src/helper/streaming/text.ts)

## HTTP observations on the exported app

I built and exported one app folder, then served copies with the matching StarlingMonkey IPC/2 and deno_core IPC/3 development Kernels. Both returned HTTP 200 and `é` for an ordinary buffered response. Both wrote `Content-Length: 2` on the HTTP response. Supplying a matching length or `Transfer-Encoding: chunked` in JavaScript returned 200 on the spike and 502 `invalid_response` on deno_core. An incorrect length returned the same 502 in both. A POST with a multipart file containing bytes `00 ff` returned `[48,50,53,53]` on the spike and `[0,255]` on deno_core. The response also confirmed the guest's local multipart encoder emitted bytes `00 ff`.

The full ignored spike suite passed export/import with app identity, both lifecycle tests and the SQLite benchmark. The 100 MB HTTP test reset its TCP connection. The authenticated HTTP test passed the oversized-body 413 assertion on this run. It then returned 502 at the later test that directly publishes a top-level loop and expects a runtime 504. The cache-only launcher had no component for that bundle. Direct preparation hit the build watchdog. Move that assertion to build refusal for Wasm and keep runtime deadline checks with a loop inside `handle`. The [spike report](./functions-wasm-spike.md#export-and-test-results) recorded an earlier 504 at the oversized-body assertion. A focused rerun with the rebuilt worker again passed 413 and reached the later 502. Keep the 413 case in the parity suite.
