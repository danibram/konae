# Functions runtime research

This directory keeps experiment methods and evidence for Konae server functions. The user-facing app contract is in [Server functions](../../docs/functions.md). The runtime decisions are in [ADR-0018](../../docs/adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md) and [ADR-0019](../../docs/adr/0019-functions-stream-request-bodies-with-bounded-ipc.md). Current deployment requirements are in [Operate an installation](../../docs/operate.md#typescript-function-workers).

## Evidence index

- [Original runtime comparison](results/2026-11-20/README.md) records the experiment that compared Rust, `deno_core`, Node and Next. It predates the current child process and IPC/2 request streaming.
- [Independent checks](checks/review.md) records the original experiment's functional differences and limits.
- [Hardening evidence](evidence/2026-11-20-production-hardening/README.md) records isolated Linux runner and OS-sandbox checks for its identified source revisions.
- [Linux executor evidence](evidence/2026-11-20-linux-executor/README.md) records an earlier packaging and startup check. It does not prove final-image app traffic.
- [Kernel function evidence](evidence/2026-11-20-kernel-functions/logs.txt) and [worker evidence](evidence/2026-11-20-worker-final/logs.txt) record earlier integration test runs. Their source hashes and protocol revisions matter; they are not one cumulative test of a single final build.
- [Request body evidence](evidence/2026-11-20-request-body-100mb/decision.md) documents IPC/2 streaming limits and the tested platform.
- [Linux ARM64 executor check](evidence/2026-11-20-linux-v8-final/README.md) records V8 worker checks from an earlier protocol revision.

## Current source and experimental scope

The current tracked branch is `43decb081494dbfe854327dd1279f9b9c69f17df`. `kernel-rs/kernel` owns function declarations, HTTP authorization, action calls, SQLite transactions and receipts. `kernel-rs/functions-executor` owns the paired process and IPC protocol. The app bundle uses ABI `konae-functions/1`; private IPC is `konae-functions-ipc/2`. See [Functions research](../../docs/architecture/functions-research.md) for the distinction between branch source, isolated candidates and measurements.

The Linux work most recently packaged an ARM64 runner candidate and started a Kernel image under a disposable delegated cgroup. The image passed health with no apps. Two separate HTTP harness tests using that runner passed authentication, SQLite, OOM recovery and request streaming checks. They did not send app HTTP requests through the final image or a production runtime. Functions remain experimental.

## Historical benchmark reproduction

The source archive `results/2026-11-20/source-before-hardening.tar.gz` is not tracked. It is absent from a clean clone, so the older benchmark cannot be reproduced from this repository alone. The tracked benchmark scripts, fixture manifests and result reports remain available for inspection; they do not replace the missing source archive.

Tests under `checks/` use disposable processes and databases. Do not aim them at a live installation. They preserve historical failures as well as passes. Existing evidence directories, manifests and source hashes stay with the experiment so readers can identify what each run tested.
