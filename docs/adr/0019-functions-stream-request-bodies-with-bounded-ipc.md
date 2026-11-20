# Functions stream request bodies with bounded IPC

Status: accepted

## Decision

Functions accept at most 100000000 HTTP request-body bytes, which means 100 MB in decimal units. The Kernel counts bytes as Axum yields them. It ignores Content-Length for enforcement, so chunked requests and incorrect length headers cannot bypass the limit. Byte 100000001 returns 413 with `The function request exceeds 100000000 bytes.` The Kernel does not decompress Content-Encoding, so it counts and forwards the encoded body bytes unchanged.

After authentication, rate limits and Function admission, the Kernel reads the request incrementally. It hashes and counts each byte, then sends at most 64 KiB per binary IPC frame when the worker asks for another chunk. Hono receives a standard Web `Request` whose body is a `ReadableStream`. The worker keeps its existing heap and RSS limits. The Kernel allows 8 MiB of pending body chunks across active Functions and fails fast with 503 when that budget is full. It does not allocate a 100 MB buffer, spool full requests to disk or reserve body memory when an app starts. The deadline covers upload, worker queueing, IPC and handler execution.

The private Kernel-to-runner protocol uses ABI `konae-functions-ipc/2`; the public bundle manifest remains on `konae-functions/1`. Operators must update the Kernel and runner as a pair. Their startup handshake rejects a mixed pair.

A POST handler with an action allowlist must consume the complete body before its first action call. An early call creates a sticky native failure, including when JavaScript catches the rejected Promise. The Kernel opens no write transaction before body EOF and checks the complete body size before committing an action or receipt. A request that exceeds the limit cannot leave a data mutation or idempotency receipt.

New idempotency fingerprints use an explicit `v3:` version. V3 hashes the method, path and canonical headers with the complete byte length and streaming SHA-256 body digest. Existing unversioned v2 receipts keep their original length-prefixed raw-body fingerprint. A prelookup identifies those receipts before starting a worker. The legacy replay path buffers at most the old 16 KiB request limit. A larger request cannot match a v2 receipt, so the Kernel counts it through EOF without retaining it and returns an idempotency conflict. Receipts remain part of exported app data.

Each serialized action operation has a separate 1 MiB cap measured after JSON escaping. The 64-operation limit, action schemas, SQL bindings, query row and result limits, response cap and file limits stay independent. `request.json()` and `request.text()` retain standard Web behavior, but parsing a 100 MB body requires worker heap and may exceed the default 64 MiB heap or 256 MiB RSS cap. Large-body handlers should consume `Request.body` incrementally.

## Consequences

- A normal Hono handler can use `await request.json()` for ordinary JSON payloads larger than the former 16 KiB IPC inline body and the prior 64 KiB HTTP route limit.
- Streaming handlers can process bodies larger than the worker heap without storing the full body in JavaScript.
- Uploads share a small pending-byte budget. They do not reserve 100 MB each, and slow uploads do not hold SQL permits or action locks before their first data operation.
- The 100 MB request-body contract does not imply that one JSON object, action input, SQL value, response or uploaded file may be 100 MB.

## Verification

Evidence and test results are recorded in `spikes/functions-runtime/evidence/2026-11-20-request-body-100mb/`.

The recorded IPC/2 HTTP boundary tests ran on macOS arm64 with the matching release runner. They do not verify IPC/2 streaming on Linux. The Linux image and sandbox evidence under ADR-0018 covers earlier protocol runs only.
