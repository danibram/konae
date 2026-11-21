# Functions research and current scope

This note records what the investigation established and what remains outside implementation baseline `43decb081494dbfe854327dd1279f9b9c69f17df`. It does not describe isolated patches as shipped code. The [Functions guide](../functions.md) documents the current implementation behavior.

## Current branch and candidate work

The baseline at `43decb08` used the runner in `spikes/functions-runtime/rust-host` and reused JavaScript globals between requests. The current Kernel starts `konae-functions-runner` from `kernel-rs/functions-runner` on demand. `functions-executor` supervises the child and owns the private IPC/2 protocol. A worker can keep its process warm until the idle timeout. Each request evaluates the bundle in a fresh V8 isolate. When a request is cancelled, the runner gives JavaScript a short grace period before terminating the isolate. The runner stops and joins the watchdog before dropping the isolate. The Kernel authenticates the request, fixes its actor for the invocation, checks each action call and owns SQLite transactions and receipts.

The baseline branch kept a successful POST response in app data. A matching retry returned that response without invoking the handler or rechecking row access. The current branch returns `409 idempotency_replay` for a matching receipt. Callers read current state through an authorized request. New receipts keep the existing table layout and store a marker instead of the response body, while older exported receipts remain readable. An isolated `204` candidate and its coordinated Kamynando client adaptation remain separate research; the current branch does not require client recovery by intent.

Other candidate patches explored a correction to the existing x86-64 seccomp filter, per-deployment worker and body quotas, stream EOF checks, removal of temporary bundle files, type-resolution fixes, and the 100,000,000-byte HTTP request path. The present source includes the request streaming and bounded IPC design in ADR-0019. It does not include every isolated candidate change or imply that each candidate passed the final Linux deployment contract. The current implementation's limits and behavior are in the guide and ADRs.

## Runtime and package boundary

The study selected `deno_core` as an implementation candidate because it embeds V8 and provides Web APIs through extensions while keeping identity and data operations in the Kernel. It did not establish that arbitrary npm packages work. Hono executed in a bundled app, but packages must be bundled for the browser ESM target, use exact dependency versions, avoid `node:` and host modules, and stay within the Web APIs the runner exposes. This is not Node.js, `deno_runtime`, or a general-purpose `fetch` environment. Native addons, Node builtins and unsupported Web APIs need separate implementation and evidence.

Hono is optional. Apps can export `handle(request, context)` directly. Both forms use the same Kernel mount, actor and action rules. The app chooses packages and versions; the Kernel does not resolve npm at request time.

## Replay and authorization

The Kernel authenticates each function call before receipt lookup. A matching key and fingerprint returns `409 idempotency_replay` without invoking the handler or returning the old response body. The caller must read current state through a request that follows current action authorization. This applies when row access changes between the original request and a retry.

The isolated `204` candidate aimed to avoid returning stored response data after access changed. It was paired with client recovery for app operations that need the original generated IDs. That behavior is not part of this repository branch. The implemented `409` response asks callers to read current state through existing authorization rules.

## Runtime reuse measurements

A macOS arm64 experiment compared three candidate worker policies with one synthetic app. The series completed 186 HTTP requests and verified database effects. The measurements help explain tradeoffs in that workload; they do not set a production default.

| Policy | Authenticated create p50, ms | Three parallel reads p50, ms | What it reused |
| --- | ---: | ---: | --- |
| A | 5.98 | 10.39 | Process, isolate and modules across requests |
| B | 37.76 | 105.93 | A process per request for sensitive work |
| C | 23.62 | 61.60 | Process retained, fresh runtime per request |

C was slower than A and faster than B in those samples. A retained JavaScript globals. B paid process startup. C retained native process memory while rebuilding the JS runtime. Ten rounds per policy completed, with the first round excluded and nine comparable rounds per policy. The nine-round sample does not settle CPU use, sustained or peak RSS, p95 under load, Linux behavior, or the right default. A separate profile found serialized reads for the same function key, but did not attribute total latency precisely to the mutex, initialization, or handler work. The current runner uses the process and isolate lifetimes described by policy C. The table measures the earlier prototype.

## Linux checks and the current branch

The latest Linux ARM64 lab run packaged a runner candidate and started a Kernel image under a disposable delegated cgroup. The image passed `/_konae/health` with no apps installed. Separately, a harness that used the packaged runner passed two HTTP cases for authentication, SQLite, OOM recovery, and 100,000,000-byte request streaming. Those tests did not send an app HTTP request through the final image. They also do not validate a production service manager or deployment runtime. Earlier hardening records and later IPC/2 tests cover different source and protocol revisions; do not combine them into a claim that one final image passed every check.

The current operator procedure still requires a target-runtime check before serving Functions. Confirm the actual image, runner pair, delegated cgroup, private namespaces, authenticated app request, denied-sandbox behavior and cleanup in that environment. No production deployment was part of this research.

## App usage investigation

The Kamynando study mapped existing Hono handlers to 38 named SQL actions. It proposed combining patient-directory metadata with its archived count, and returning session files with the selected session. The first could reduce three business requests to two on initial directory load; the second could reduce two GET requests to one when opening a session. These were code-path counts, not latency measurements, and no Kamynando changes belong to this branch.

The study recommended keeping photo registration separate from choosing a photo until retry and failure behavior are proven. It also advised preserving existing row rules, file ownership checks, upload reconciliation, and autosave ordering. These are app-specific findings, not new Functions APIs.

## Observability proposals

Automatic function waterfalls, method-level spans, sampled CPU profiles and OTLP/HTTP export were proposals in an unimplemented plan. They are not current Usage features. The plan proposed adding elapsed time per function and day to the existing Usage writer in batches. A sampled waterfall would use bounded traces and avoid adding nested spans together in totals. TypeScript hooks would exist only in diagnostic bundles, buffer numeric IDs, and stop recording at a configured cutoff. An optional asynchronous Kernel exporter would use a bounded queue and drop data without blocking requests when the collector is unavailable. The cost remains to be measured by comparing runtime off, metrics only, sampled tracing, and export with the collector up and down. Existing Usage records request-level timing and outcome.

## Research sources

The investigation sources were research closeout, critic rounds, F3 policy measurements, an OTLP/waterfall design plan and the Kamynando usage investigation, captured in BB threads on 2026-11-20 and 2026-11-21. They remain local to the research environment and are not required to read this repository document. `functions-reviewed.patch`, an earlier Functions candidate applied to implementation baseline `43decb08`, had SHA-256 `e256a8bd695524c3d82a14df6624e520d4a9447b07c7011917d909d06fd48fa3`. The separate `functions-recovery-candidate.patch` applied to the same baseline and had SHA-256 `9be74c0a839436e5814df3ce67c25d0c8cf1523234bbabfea814b3dd430f39fd`. The coordinated Kamynando candidate `kamynando-recovery-final.patch` used base `24da49b6` and had SHA-256 `d4c1cc1b4975673914924c58cf81c78437512c16b777a7689cb6368945329693`. These candidates remained isolated and are not the source of this branch. The F3 A/B/C worker-policy study above was a separate experiment from the replay recovery candidate. The Linux ARM64 runner tested in the latest package-and-health run had SHA-256 `9c74ed3d77259646477d3b6b1cd16daf40c17f257e00c6902da119d133a409b1`; the image passed health without apps, while two separate harness HTTP tests used that runner. The benchmark figures above came from a separate macOS experiment and are not measurements of this Linux artifact.

Durable code decisions and experiment records:

- [ADR-0018](../../docs/adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md) for the worker and Kernel boundary.
- [ADR-0019](../../docs/adr/0019-functions-stream-request-bodies-with-bounded-ipc.md) for streamed request bodies and separate limits.
- [Functions experiment index](../../spikes/functions-runtime/README.md) for tracked methods, reports and evidence.
- [2026-11-20 benchmark report](../../spikes/functions-runtime/results/2026-11-20/README.md) for the earlier integration comparison, which is a different experiment from the A/B/C reuse sample above.
