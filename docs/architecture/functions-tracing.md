# Function and action timings

Functions reach rows through `context.actions.call`. The Kernel runs the named Server Action in Rust, on the invocation's connection, with its existing access, actor bindings, LIMIT and guard checks. The connection stays warm across actions within one invocation. Today a POST with actions opens a transaction, and a GET reuses a read-only connection without an explicit transaction. Keep both behaviors and the shared refusals from [ADR-0004](../adr/0004-the-gate-and-the-kernel-refuse-with-the-same-code-and-words.md).

## Record at the Kernel boundary

Use a small Rust recorder with explicit parent indices. Hold one wall-clock anchor and use monotonic offsets and durations after that. Store typed IDs, counters and bounded names in one request buffer. Move the buffer across the async and blocking boundaries. Let the native task finish recording after HTTP cancellation. A shared completion slot should finalize once after it has both the HTTP outcome and the native outcome. The HTTP response and worker release must stay independent of that slot. A commit after a response deadline must keep its real outcome and the HTTP `mutation_outcome_unknown` code.

The Kernel does not use `tracing` directly today. A `tracing` subscriber and OpenTelemetry layer would supply general instrumentation. This request already carries explicit state across a blocking IPC loop. A compact recorder lets that state supply both Usage and export, and gives the request path control over memory and allowed fields. The benchmark measures this choice against the current path.

```text
GET /api/orders                         SERVER, matched function mount
  konae.function.admission              publication, access, rate and admission checks
  konae.function.worker_wait            app, installation and execution slots
  konae.function.prepare                pin or load verified app artifacts
  konae.function.worker                 process start or warm reuse
    konae.function.init                 IPC Init to Ready, cold only
  konae.function.handler                Invoke to terminal reply, including IPC and body pulls
    konae.action                        each validated action operation, in IPC order
      konae.api                         optional named Kernel service work, such as data admission
      SELECT | INSERT | UPDATE | DELETE CLIENT, SQLite statement path
  konae.transaction.commit | rollback    actual transaction end, when attempted
```

Wait spans can occur at more than one point in the request. Count resource acquisition separately from SQL. Init on a warm worker is absent. The handler duration includes isolate or realm setup and IPC. IPC/3 has no timestamp for entry into JavaScript. Time release or startup preparation at `prepare_artifact` as a separate trace when it runs before any request. The warm handler span includes its action children, so the waterfall uses offsets from the request and never sums nested durations. Late rollback can end after the HTTP span. Replay has an HTTP span and its actual receipt work, with no handler span.

The supervisor checks capabilities and sequence numbers before recording an action attempt. An operation that reaches the supervisor after an invocation has a native error still gets the existing refusal code and a count, with no SQLite child. Count actions by the resolved declaration. Group unknown names under a fixed sentinel and never store the worker's proposed name for a missing declaration. Protocol-invalid requests use a fixed executor failure on the handler span. Cap the request at 160 records, with room reserved for the HTTP result and final transaction. Mark truncation on the root. Capture matching function refusals even when a worker never starts. Carry codes from the existing refusal constructors. Never parse response text for a code.

Time ordinary Server Actions with the same action and statement fields. Start at the runtime or declared HTTP action entry, and use the existing origin for grouping. Leave scheduled runs and existing Usage totals as they are. A call action has an enqueue span. Its outbox sends later and each attempt has an HTTP CLIENT span. The function allowlist still forbids call actions and other external effects. Time the Kernel service calls the runtime already makes. Reserve the `konae.api` span for these known operations and route-to-route calls. No host API is added.

## Fields and privacy

Use the following field allowlist at collection time. `error.type` is a canonical refusal code when the Kernel has one, or a fixed error class such as `timeout`. Omit it on success. Use HTTP server/client status semantics for HTTP spans and mark failed internal work as an error. Use `_OTHER` for unrecognized methods and omit the original method text. A Server Action can fail even if a handler catches its Promise rejection.

| Span or record | Fields |
| --- | --- |
| HTTP server | `http.request.method`, `http.route` as the declared mount or route template, `http.response.status_code`, `http.request.body.size`, `http.response.body.size`, `error.type` |
| Function | `konae.function.name`, `konae.artifact.version`, `konae.runner.kind` (`deno` or `wasm`), `konae.worker.reused`, `konae.outcome`, `konae.trace.truncated` on the root when true |
| Action | `konae.action.name`, `konae.action.kind`, `konae.action.origin`, `konae.action.sequence` within the invocation, `konae.action.rows_read`, `konae.action.rows_written`, `konae.action.reply_bytes`, `konae.action.sqlite.duration_us`, `error.type` |
| SQLite | `db.system.name=sqlite`, `db.operation.name` when the parsed statement supplies a known verb, `db.response.returned_rows` for a successful query, `error.type` |
| Transaction | `konae.transaction.outcome` (`committed`, `rolled_back`, `unknown`), `error.type` |
| Kernel API | `konae.api.operation` from a fixed list of permitted operations or existing tool names, `error.type` |
| Outbox HTTP client | `http.request.method`, `http.response.status_code`, `http.request.body.size`, `http.response.body.size`, `konae.action.name`, `konae.call.attempt`, `error.type` |

Body sizes count observed bytes before transfer framing. Omit a size if the Kernel never saw the complete body. Count action reply bytes from the encoded reply before the IPC length prefix. For a direct Server Action, use the encoded HTTP body size and keep its origin distinct. Reuse existing serialization. Rows read means query rows produced for the caller. Rows written means direct SQLite changes reported by the statement. A later rollback can discard those changes. Use zero when there was no statement and omit the count when a failed statement left it unknown. SQLite time brackets the shared statement executor, including statement preparation, stepping and bounded row conversion. It excludes slot wait, connection setup and IPC. Use the existing database result for row counts.

Allow deployment ID as `konae.deployment.id` on the trace root and use `service.name=konae-kernel`. Capture no row values, inputs, SQL text, paths with parameter values, query strings, raw hostnames or URLs, headers, cookies, actor IDs, tokens, or exception text. Exclude `url.full` and `server.address` on the client span because a call action can derive the target from app data. These are deliberate omissions from the HTTP conventions. Use codes and fixed labels for new Usage records. Stop writing and exposing raw function error detail there. Old stores can retain the column for compatibility. Suppress old details on read and clear stored values through the existing writer, so a later Usage export omits those values. Existing copies have their own retention. HTTP responses and error rules keep their existing behavior.

Reference the [OpenTelemetry HTTP conventions](https://opentelemetry.io/docs/specs/semconv/http/http-spans/) and [database conventions](https://opentelemetry.io/docs/specs/semconv/db/database-spans/).

## Trace context

Accept a single valid W3C `traceparent` on matched function and action requests. Use its trace ID and remote span ID. Start a new trace when the header is absent, malformed or duplicated. Check zero IDs, lowercase hex, version and length according to W3C. Keep the version 00 layout for outgoing headers. Do not collect or forward `tracestate` or baggage. Generate a new server span ID and return `traceparent` with it on matched success and refusal responses. The trace ID continues, while the returned parent ID identifies the Kernel response. This is Konae's response convention. Never pass trace headers to JavaScript, authorization, receipts or the idempotency fingerprint.

At call-action enqueue, persist only the validated trace context and enqueue span ID in the installation outbox. Nullable fields keep existing pending jobs deliverable. Each attempt makes a new client span with that enqueue span as parent and sets its `traceparent` after the existing signing and header rules. Propagate through both the in-process target and the guarded external sender. Keep the header out of stored request headers and public call history. Job retries reuse the parent context, and never reuse attempt span IDs. A captured test-mode call keeps its existing no-send behavior. [W3C Trace Context](https://www.w3.org/TR/trace-context/)

## Usage and local events

Keep the current `functionLatency` series and add error-code counts. Add action latency series keyed by action, artifact version and origin. Show sample count, p50/p95 for action and SQLite duration, row and byte totals, and counts by error code. Keep the newest 10,000 action samples per Deployment up to the existing Usage age limit. Group at most 128 series in the owner read and report retention and truncation. Use the existing bounded Usage writer, one message per invocation carrying its action samples, and one write transaction per batch. Keep existing count, drop and coverage behavior. Old stores should read as empty for the new series.

With no endpoint, a live installation stores only metrics in Usage. In `konae dev`, keep a separate local ring of the newest 100 completed waterfalls for up to 24 hours, limited to 160 records per request. Write it off the request path into the app's installation-private state. The existing `kernel events` command reads it in test mode and returns it to `konae events --json`. The local `konae dev` events page shows one expandable waterfall per invocation. The plain CLI can print offset and duration. It needs no inspector, port or collector beyond the existing events page. Production Usage and app exports exclude this waterfall.

Usage already travels as `data/usage.sqlite` when data is included. Its new tables hold bounded measurements, independent of the app's SQL, accounts and rules. An older Kernel can ignore the new tables. Keep exporter config, trace queue, outbox trace metadata and local trace files on the installation side. Do not add modules, app settings, roles, export fields or a new capability.

## OTLP export

The operator sets `KONAE_OTLP_TRACES_ENDPOINT` to the full OTLP/HTTP traces URL, normally ending in `/v1/traces`. Optionally set `KONAE_OTLP_TRACES_HEADERS` to a JSON object of collector headers and `KONAE_OTLP_TRACES_SAMPLE_RATE` to a number from 0 to 1, default 1. Reject invalid settings at configured startup. Accept absolute HTTP and HTTPS URLs. Disable redirects. Send OTLP/HTTP JSON using the existing HTTP client dependency and the OTLP schema. Use lowercase hex IDs and string nanosecond timestamps as OTLP JSON requires.

Make the local sampling decision independent of an untrusted sampled bit. Use a per-installation keyed hash of the trace ID for intermediate rates. Reflect the decision in the outgoing sampled bit and accept that a trace can be partial across installations. Rates apply only to export. Usage counts and dev capture remain independent. With no exporter, propagate IDs and clear the sampled bit unless recording a local waterfall.

Use a process-wide queue of up to 256 completed trace buffers, each capped at 24 KiB. Queued payloads take at most 6 MiB before queue bookkeeping. Enqueue with `try_send`. One background task drains up to 512 spans or 512 KiB per HTTP post, flushing within one second. Set a three-second total timeout per batch. Drop failed, rejected or timed-out batches without retrying in this slice. Count lost traces or spans on full queue, failed export, collector partial success and shutdown. Report counters in rate-limited operator logs without URLs, credentials or payload. Allow up to one second for best-effort shutdown drain. Usage and application requests should succeed with the collector down. [OTLP protocol](https://opentelemetry.io/docs/specs/otlp/) and [exporter specification](https://opentelemetry.io/docs/specs/otel/protocol/exporter/).

## JavaScript client

Provide a server-only virtual module `konae:actions` in both builders. Its only runtime export is `actions(context)`, which returns `context.actions`. Generate the ambient module declaration in `function-actions.d.ts` from the same validated Action Contract and the `functions.json` allowlists. It exports `FunctionContext<"function-name">` and types the helper's input and output. Keep required and nullable inputs and output types in sync with the Action Contract. Use a separate server map from the page's `KonaeActions` because the call doors differ.

The editor makes the generated declaration available to function source tooling. CLI build, dev, contract and app folder generation refresh it. The folder's standalone build uses the same generator, shipped with its build support and run before its existing TypeScript check. Keep the virtual module out of page builds and both generated files out of the Capsule. TypeScript diagnostics belong to author tooling. The Gate and Kernel keep their existing shared refusals and cases. This client adds no host operation, protocol field, package dependency or runner branch.

Do not add `context.trace.span(name, fn)`. A native begin/end API would cost two sequential host operations and four IPC/3 frames per guest span on each runner, plus JS scheduling and validation. This is a count derived from the protocol. There is no comparable timing for that operation on either runner. A guest API would also need rules for nested spans, cancellation, labels and budget. The Kernel already observes every row access and the total handler interval.

## Benchmark and changes

See [the benchmark driver](../../spikes/functions-tracing/measure.py) and its [temporary instrumentation](../../spikes/functions-tracing/instrumentation.patch). It adds only development probe code and leaves the runtime files out of this branch's committed diff.

Measured November 23, 2026 on an Apple M3 Pro with macOS 26.6.1. The Kernel used the dev profile and the IPC/3 deno_core runner used the release profile. The loopback driver sent GET `/api/probe` over one persistent connection. The handler called the same named one-row query ten times. It returned HTTP 200 and `{"sum":10}` for every request. The trace mode returned a trace ID and verified ten action records and ten SQLite records. The worker PID stayed at 76539. After 40 warm-up requests, the driver collected 1,200 requests per mode in shuffled groups with all six mode orders. Host load was 9.17 to 8.83.

| Mode | Loopback p50 | Loopback p95 | p50 delta | Kernel p50 |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 6,098.834 µs | 8,000.916 µs | 0 µs | 5,595.416 µs |
| Metrics | 6,121.708 µs | 8,633.125 µs | +22.874 µs | 5,616.875 µs |
| Full capture | 6,148.750 µs | 8,553.875 µs | +49.916 µs | 5,639.000 µs |

The loopback budget was 121.977 µs, 2% of baseline p50. Full capture added 0.82%. The median within-group loopback deltas were +54.104 µs for metrics and +65.958 µs for full capture. A 60-per-mode smoke run exceeded the gate. Use the fixed 1,200-per-mode run for the comparison. The p95 spread and shared host load make this a local estimate.

Reproduce from a clean checkout with dependencies installed. Apply the probe in a disposable copy of the checkout and use its built binary.

```sh
df -h /System/Volumes/Data
cargo build --release --locked --manifest-path kernel-rs/functions-runner/Cargo.toml
git apply spikes/functions-tracing/instrumentation.patch
CARGO_INCREMENTAL=0 cargo build --locked --manifest-path kernel-rs/Cargo.toml -p kernel --bin kernel
python3 spikes/functions-tracing/measure.py kernel-rs/target/debug/kernel \
  kernel-rs/functions-runner/target/release/konae-functions-runner --rounds 200
git apply -R spikes/functions-tracing/instrumentation.patch
```

Kernel time comes from a header stamped after Usage enqueue and probe handoff. Loopback time includes HTTP handling and receipt of the full response. The probe appends bounded typed timing records and hands the buffer to a background sink. Full capture also generates a trace ID and a root span ID. The off mode uses the same patched binary and checks the probe header. Parent graph assembly, propagation of inbound context, OTLP serialization and transport, persistent per-action writes and POST finalization remain implementation checks. Repeat on both runners after B2c merges.

| Slice | Files |
| --- | --- |
| T1, record and read | `kernel-rs/kernel/src/functions/{http,runtime,invocation,preparation,trace}.rs`, module declaration, executor `supervisor.rs` for phase and operation observations, `actions/{http,execution}.rs`, `integrations/http.rs`, runtime dispatch where needed, `usage.rs` and `usage/function_latency.rs`, `app_folders/layout.rs` and `server/operator.rs` for local reads, `scripts/events.ts`, `src/lib/usage.ts`, `src/components/UsagePage.tsx` and focused tests |
| T2, send | `kernel-rs/kernel/src/functions/otlp.rs`, module declaration, startup and shared state wiring, `integrations/calls.rs` for outbox trace context and headers, `scripts/konae.ts` for the dev environment, `docs/operate.md` and focused tests |
| T3, generate | `src/sdk/functions.ts`, `src/lib/{action-types,action-contract,app-folder,export-project}.ts`, `src/lib/local-builder.js`, CLI embedded resources, `src/runtime/browser-build.ts`, `scripts/konae.ts`, `src/agent/system-prompt.ts`, `docs/functions.md` and focused tests |

All three start after B2c merges. T2 integrates on top of T1's recorder and context. T3 can proceed from B2c independently. Both runners keep IPC/3 unchanged.
