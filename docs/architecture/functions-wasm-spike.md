# Wasm functions runner spike

The spike asks whether an exported app folder can carry its function code to another machine and keep the same isolation. The HTTP-facing Kernel must be able to run under an ordinary account on macOS, stock Debian 12 with Linux 6.1, and stock Ubuntu 24.04 with Linux 6.8. The performance limits are 250 ms for a cold instantiate and for the slowest case in the existing suite, and 5 ms for the warm median of a trivial GET. These limits use a 1 s function deadline.

## Engine check

Both candidates ran on an Apple M3 Pro with 18 GiB RAM under macOS 26.6.1. The feasibility runs used Wasmtime 38.0.4. Each result in this table is an initial probe, with the exception of the three QuickJS startup samples.

| Candidate | Probe | Result |
| --- | --- | --- |
| StarlingMonkey and ComponentizeJS 0.21.0 | Wizer component for an ESM function that returns `new URL(url).pathname`. Disabled stdio, random, clocks, HTTP and fetch events. | 13,427.9 ms to componentize, 12,451,589 bytes, zero component imports. |
| StarlingMonkey and Wasmtime | `wasmtime run --invoke 'handle("http://example.test/path")' starling.wasm` | Returned `"/path"` in 16.241 s wall time, including process startup and native compilation. |
| StarlingMonkey and Wasmtime | `wasmtime compile -o starling.cwasm starling.wasm`, then the same invocation with `--allow-precompiled` | Returned `"/path"` in 2.202 s wall time and 0.14 s process CPU time. The compiled component was 47,090,568 bytes. The machine was running other builds. |
| QuickJS-ng 0.18.0 | Compile the shared reactor at worker startup and time process launch through IPC `Ready`. | 1,655.2 / 470.8 / 378.8 ms. The first build of the guest lacked the monotonic clock import and could acknowledge Init but could not invoke a handler. |

ComponentizeJS pre-initializes a separate runtime for each source bundle. That removes JavaScript parsing from an already built component's request path and provides Fetch and Streams through StarlingMonkey. A cache miss at Init would exceed the deadline before the first handler call. Moving componentization into `konae build` could avoid that cost during service. The spike chose QuickJS because the engine can be compiled once for all bundles, and the runner can accept existing ESM through IPC v2 without a componentization tool on the request path. [ComponentizeJS](https://github.com/bytecodealliance/ComponentizeJS/blob/main/README.md), [StarlingMonkey](https://github.com/bytecodealliance/StarlingMonkey), [QuickJS-ng](https://github.com/quickjs-ng/quickjs/releases/tag/v0.18.0).

## Runtime boundary

The new runner uses the executor's `konae-functions-ipc/2` types and framing. The build compiles the pinned QuickJS-ng reactor with WASI SDK 30. Wasmtime compiles that reactor to native code during the Rust build. The runner embeds those bytes and links the imports at `Init`. The Rust build compiles the Web bootstrap to QuickJS bytecode. At `Init` the runner hashes the app's ESM source with SHA-256, then compiles it in a bounded guest store with no request capability. IPC v2 gives each worker one bundle, so the cache holds one result for that hash and dies with the process. A new bundle starts a new worker. QuickJS-ng can serialize the self-contained ESM Konae accepts. The guest retains the import-refusal policy. At `Invoke` the runner creates a fresh store and instance, loads the Web bootstrap, then decodes, resolves and evaluates the cached ESM bytecode. Top-level code still runs for every invocation. The child preserves the v2 request failure for a bundle that fails to compile at Init. After a successful reply the runner retains that instance until the next invocation or retirement so an optional `shutdown` export can run. Shutdown has its own budget and no request capability.

The linker exposes `op_worker_call`, `op_worker_body_read`, `op_worker_failure`, two clocks via `clock_time_get`, and four libc descriptor stubs that return `BADF`. The runner supplies no WASI filesystem, environment, stdio, network, random source or process APIs to the guest. The native runner needs ordinary process and thread creation, pipes, clocks, executable memory and signal handling. The Kernel still owns authentication, row access, transactions and idempotency.

The host keeps the capability nonce and both sequence numbers. Each response to an operation or body read must match its request. JavaScript calls named actions through a closure. The host attaches the current capability and the executor checks the action. The body starts as a pull stream with zero queued chunks. Each pull requests up to 64 KiB, and the host copies the IPC body bytes into Wasm memory. IPC v2 still returns buffered UTF-8 responses. The runner enforces the response size and the executor validates the reply before the Kernel commits.

The Store caps linear memory at `WorkerLimits.heap_bytes`. The parent also retains its existing process RSS polling. IPC v2 carries no deadline field. The executor sends `Cancel` at its own deadline, and the reader thread advances the Wasmtime epoch when the nonce matches. Wasm code traps at its next epoch check. The reader signals between IPC frames and the body and action imports check for cancellation while they wait. A 65 s local stop handles an unresponsive parent. Cancellation stops the instance. It does not deliver a final JavaScript `abort` event to the handler.

The current Kernel chooses its Linux containment policy before starting the runner. The spike uses `KONAE_FUNCTIONS_DEVELOPMENT=1` and leaves that policy untouched. That mode gives the worker an ordinary process and retains the Kernel's other test-mode behaviors.

## Production work

- Have `konae build` record the selected guest ABI and digest in the server artifact manifest. The QuickJS layout can ship a portable core `.wasm` with its Web layer once per ABI. Define how the Kernel matches it to an installed runner. Keep native `.cwasm` local, authenticated by the installation or built into the runner. Never deserialize native code supplied by an app export.
- Extend export and import to preserve the portable artifact and manifest alongside the ESM, accounts, data and rules. Verify their hashes before activation and retain the current atomic release path.
- Add shared Gate and Kernel refusals for unsupported Wasm ABI, missing or mismatched artifact digest, an import outside the allowed set, missing required exports, and JavaScript or Web APIs that the selected runtime rejects at build time. Use identical codes and wording with shared cases. Runtime-only failures still need their existing limits and error codes.
- Give production the Wasm containment policy without requiring Linux containment to start the executable. Validate the effective host imports and native runtime packaging on both stock kernels. Review the Web layer against the Fetch, URL, encoding and Streams contracts. Address abort events and persistent or cryptographic APIs only where the app contract calls for them.
- IPC v3 will send all bundles at Init, name the function in Invoke and carry a `handler_error` detail. The runner will need a name-to-bundle map, per-function retirement semantics and bounded exception detail. No protocol edit is required for this v2 spike.

## Measured latency and memory

The Mac was an Apple M3 Pro with 18 GiB RAM, macOS 26.6.1, arm64, and `uname -r` reporting `25.6.0`. Docker Desktop 29.9.3 reported 9 vCPUs and 7.75 GiB RAM for its VM. Both arm64 images reported `7.0.14-linuxkit`. The Docker processes ran as UID 65534 with `CapEff=0`, `Seccomp=0`, `NoNewPrivs=1` and no network other than loopback. The container commands use no cgroup delegation. The stock 6.1 and 6.8 kernels ran zero samples.

All figures are milliseconds except RSS, in KiB. Each IPC row contains 30 measured requests after one warm-up. For cold runs, the harness starts a new process, sends Init, waits for Ready and invokes the trivial handler once. A cold reply includes process launch, IPC, mapping the compiled module, Web setup, ESM evaluation and response encoding. The OS can keep binary pages cached. Warm IPC keeps one process, creates a new instance for each invocation and still waits for both pipes. RSS snapshots show the resident pages immediately after Ready and after the series. They omit peak usage and include native code, runtime state and allocator pages. Several builds and test runs shared the machine.

| Environment | Cold first reply p50 / p95 / max | Init through Ready p50 | First Invoke p50 | Warm IPC GET p50 / p95 | Worker RSS before / after |
| --- | ---: | ---: | ---: | ---: | ---: |
| macOS | 52.521 / 88.939 / 102.376 | 17.777 | 34.481 | 81.546 / 168.726 | 14,048 / 12,928 |
| Debian 12 Docker | 52.153 / 61.589 / 67.356 | 18.429 | 32.708 | 30.940 / 36.113 | 16,032 / 18,308 |
| Ubuntu 24.04 Docker | 51.758 / 80.940 / 88.215 | 20.038 | 32.260 | 31.701 / 50.898 | 16,260 / 18,536 |
| macOS, follow-up | 71.724 / 123.350 / 128.708 | 29.394 | 44.008 | 38.770 / 57.461 | 14,080 / 16,688 |

The HTTP harness signs up a disposable app user to stay within the Kernel's rate limits. It discards the first call of each method, then alternates 30 GETs and 30 POSTs on one HTTP connection. Every POST sends a new idempotency key, updates a row with `counter.add`, and reads it with `counter.read` before commit. Each request measures client wall time through receipt of the response body. That includes routing, authentication, worker IPC and scheduling. POST adds SQLite and idempotency. Each export started at 1 and ended at 31.

| Environment | GET p50 / p95 | POST p50 / p95 | Worker RSS |
| --- | ---: | ---: | --- |
| macOS, checkout Kernel | 38.420 / 96.266 | 61.845 / 100.981 | Same two PIDs. 16,320 / 12,960 and 16,416 / 13,088 KiB. |
| Debian 12 Docker, lab Kernel | 75.009 / 131.375 | 105.632 / 142.967 | Zero idle worker children at both snapshots. |
| Ubuntu 24.04 Docker, lab Kernel | 71.255 / 96.983 | 92.288 / 136.460 | Zero idle worker children at both snapshots. |

Only the macOS HTTP series demonstrated worker reuse. The Linux HTTP figures are successive requests through the packaged IPC v2 lab Kernel, which had no worker children at the snapshot points. They may include process startup. The standalone Linux IPC series verified a warm worker and sampled its RSS.

| Artifact | Size in bytes |
| --- | ---: |
| macOS Cargo release runner | 8,332,944 |
| Linux arm64 linked runner including debug information | 20,750,792 |
| Portable QuickJS reactor | 1,342,684 |
| Compiled reactor in the macOS runner | 5,686,160 |
| Compiled reactor in the Linux runner | 5,751,656 |
| Compiled Web bootstrap | 585,487 |
| Per-bundle native artifact | 0 |
| Hono orders and preferences ESM | 816,020 and 62,018 |
| Exported hello and count ESM | 65 and 198 |

The same guest Wasm went into both builds. Its SHA-256 was `5a943a0ce21cae2a3e3c59012f17e089bee7d63223d2f1f57d84f0bcd7797445`. The Linux test binary links the cached release rlibs with `rustc -C opt-level=3` and runs the crate's build script for arm64 Linux. It uses the same guest, configuration and runner source as the macOS Cargo build. The Linux HTTP tests use the already packaged IPC v2 lab Kernel, whose binary hash is `505a40b3003fa9fe297241e49c00a6c3694440d1ace84512b5c6528346926937`.

## Hono 4.13.19 middleware

These are direct IPC observations on macOS. The compiled probe mounted real middleware and sent a signed HS256 test token to the JWT route.

| Middleware | QuickJS in Wasm | deno_core |
| --- | --- | --- |
| JWT | 500, `crypto is not defined` | Same |
| Default ETag | 500, `crypto is not defined` | Same |
| ETag with supplied digest | 200 with `ETag: "010203"` | Same |
| Cache | Route returned 200 and `cacheAvailable: false` | Same |
| Signed cookie | 500, `crypto is not defined` | Same |
| `streamText` with two writes | 200, buffered `onetwo` | Same |
| `streamText` with `sleep(1)` between writes | 200, partial body `one` | No reply within the probe's 10 s limit |

Hono adds `Transfer-Encoding: chunked` to both stream variants. The executor rejects that hop-by-hop header. An application that emits a stream without that header still reaches a buffered UTF-8 IPC reply. Neither runner exposes a timer, WebCrypto or a persistent Cache API in this configuration.

## Correctness and test commands

Run the guest build and the runner's own tests from the repository root:

```sh
crate=kernel-rs/functions-runner-wasm
npm ci --prefix "$crate"
WASI_SDK=/path/to/wasi-sdk-30 "$crate/guest/build.sh"
cargo test --release --locked --manifest-path "$crate/Cargo.toml"
cargo build --release --locked --manifest-path "$crate/Cargo.toml"
```

The new crate passed all 11 tests, including eight adapted from the deno_core isolation tests. The three added tests cover absent host APIs, the 64 MiB linear memory limit, and UTF-8, AbortController and multipart file round trips.

| Command | Result on macOS |
| --- | --- |
| `cargo test -p kernel -p konae` from `kernel-rs/` | 721 passed, 10 ignored. |
| `KONAE_TEST_FUNCTION_WORKER=<Wasm runner> KONAE_TEST_LEGACY_FUNCTION_WORKER=<frozen IPC v1 runner> cargo test -p kernel -- --ignored` from `kernel-rs/` | 5 passed, 5 failed. The export/import test and both real-worker lifetime tests passed. |
| `cargo test --release --locked --manifest-path kernel-rs/functions-runner/Cargo.toml` | 9 passed, 2 failed in the unchanged deno_core runner. Both failures involved watchdog timing or retaining the worker. |
| `bun test --timeout 30000` | 1,536 passed, 115 failed, 14 skipped. The run reported localhost connection errors, timeouts, and a stopped esbuild service among the failures. |
| `KONAE_MCP_TEST_PORT=49373 LITTLEBOX_MCP_TEST_PORT=49373 bun test --timeout 30000 tests/mcp.integration.test.ts` | 6 passed, 48 failed, 4 errors. This isolated run also lost its localhost connection. |
| `bun run build` | Passed. |

The ignored Kernel suite's failures included three copy tests whose MinIO and pilot fixtures were absent. The real authenticated HTTP test received 504 where it expected 413 for an oversized body. The separate 100 MB stream test stopped with a connection reset after the suite ran for 60.56 s. These last two failures count against the runner.

The legacy 200-case suite uses `Verify` from `spikes/functions-runtime/checks/verify.py` and the rollback cases from `run_all.py`. The adapter runs the original Hono bundles with a small export wrapper that maps the old `context.data` methods to the v2 action import. The existing Node fixture still supplies sessions, SQLite, and the rollback transaction. The case timer includes two read-only SQLite snapshots, HTTP, IPC and the handler. The repeat probe restores the selected case's fixture outside the timed interval, makes a warm-up request, and times 30 repeats. It selects one case from the normal fixture and one from the rollback fixture.

| Environment | Full suite pass / fail / skip | Slowest passing case in full suite | Slowest of 30 repeats |
| --- | ---: | --- | ---: |
| macOS | 200 / 0 / 7 | 4,101.45 ms, `a.authorization.alice.POST` | 1,811.30 ms, 0 failures |
| Debian 12 Docker | 200 / 0 / 7 | 1,051.27 ms, `a.body.exact-limit` | 1,269.86 ms, 0 failures |
| Ubuntu 24.04 Docker | 200 / 0 / 7 | 1,023.67 ms, `a.body.exact-limit` | 1,194.59 ms, 0 failures |

An initial macOS run lost synchronization after the HTTP client timed out. I serialized invocations per worker in the suite adapter and reran all 200 cases. The macOS rollback case `b.internal-error.rollback-after-update` also passed 30/30, with a maximum of 1,653.76 ms. The Linux repeats of `b.internal-error.recovery` also passed 30/30 in each image. Their maxima were 1,120.29 ms on Debian and 1,140.04 ms on Ubuntu.

A second warm POST series used one worker and a disposable SQLite database directly behind IPC. The host begins a transaction, handles the two named operations, checks the reply and commits. It measures the same app bundle and checks all 30 counters. This probe has no HTTP routing, app-user lookup or Kernel idempotency receipt.

| Environment | Warm IPC SQLite POST p50 / p95 | Worker RSS before / after, KiB |
| --- | ---: | ---: |
| macOS | 25.950 / 29.633 | 14,032 / 16,720 |
| Debian 12 Docker | 98.321 / 149.782 | 16,028 / 18,324 |
| Ubuntu 24.04 Docker | 102.669 / 139.499 | 16,108 / 18,544 |

Build and run the fixtures with these commands:

```sh
root=$PWD
crate=$root/kernel-rs/functions-runner-wasm
runner=$crate/target/release/konae-functions-runner-wasm
measure=$crate/measure
app=$(mktemp -d)
exported=$(mktemp -d)

npm ci --prefix spikes/functions-runtime/app
npm run build --prefix spikes/functions-runtime/app
npm ci --prefix spikes/functions-runtime/baselines
cargo build --locked --manifest-path kernel-rs/Cargo.toml -p kernel
bun run scripts/konae.ts init --dir "$app" --name 'Wasm probe' --template notes
python3 "$measure/configure-app.py" "$app"
bun run scripts/konae.ts export --dir "$app" --out "$exported"

KONAE_FUNCTIONS_DEVELOPMENT=1 KONAE_FUNCTIONS_RUNNER="$runner" \
  KONAE_KERNEL_BIN="$root/kernel-rs/target/debug/kernel" \
  bun run scripts/konae.ts dev --dir "$app" --port 18796 --events-port 18797
```

The running `konae dev` answered these requests on macOS. The counter already held 17 from a prior run.

```text
GET /api/hello
Host: wasm-probe.localhost:18796

200 OK
wasm-ok

POST /api/count
Host: wasm-probe.localhost:18796
Origin: http://wasm-probe.localhost:18796
Idempotency-Key: wasm-probe-manual-1
Content-Type: application/json

{"amount":2}

200 OK
{"n":19}
```

Run the measurements in a separate shell after the build:

```sh
python3 "$measure/protocol.py" "$runner" --runs 30
python3 "$measure/protocol.py" "$runner" --post "$exported/server/count.mjs" --runs 30
python3 "$measure/suite.py" "$runner" --runs 1 --repeat-slowest 30
PORT=18798 "$measure/run-export.sh" "$root/kernel-rs/target/debug/kernel" "$runner" "$exported"
```

The same exported folder ran its GET and its POST in both Linux images. For the Docker commands, `linux_runner` and `linux_kernel` name the host paths to arm64 binaries. The Dockerfile adds Python, procps and Node to each base image. The measured Linux runner used the same release dependencies and guest as the macOS build. The Linux Kernel came from the prior IPC v2 lab image `sha256:d89bd44eaf0b07449fb7b475a8def7f663e0625fe808c2caa717585a8e8889fb` at `/usr/local/lib/konae/kernel`.

```sh
for base in debian:12 ubuntu:24.04; do
  tag=konae-wasm-${base%%:*}
  docker build --build-arg BASE="$base" -t "$tag" "$measure"
  docker run --rm --init --network none --user 65534:65534 --cap-drop=ALL \
    --security-opt seccomp=unconfined --security-opt no-new-privileges \
    -e PYTHONDONTWRITEBYTECODE=1 -v "$root:/src:ro" -v "$exported:/export:ro" \
    -v "$linux_runner:/runner:ro" -v "$linux_kernel:/kernel:ro" "$tag" sh -ec '
      m=/src/kernel-rs/functions-runner-wasm/measure
      uname -r
      grep -E "^(CapEff|Seccomp|NoNewPrivs):" /proc/self/status
      python3 "$m/protocol.py" /runner --runs 30
      python3 "$m/protocol.py" /runner --post /export/server/count.mjs --runs 30
      "$m/run-export.sh" /kernel /runner /export
      python3 "$m/suite.py" /runner --runs 1 --repeat-slowest 30
    '
done
```

For the middleware probes, bundle `measure/hono.js` with esbuild using `spikes/functions-runtime/app` as the resolution directory, format `esm`, and platform `neutral`. Then run `python3 "$measure/hono.py" "$runner" <bundle>`, and repeat with the deno_core worker path.

## Verdict

| Fixed clause | Verdict | Observation |
| --- | --- | --- |
| The exported app runs on macOS | Pass for the spike fixture | The CLI ran a GET and a transactional POST. The Kernel export/import test also read an imported app with its restored identity. |
| The exported app runs on stock Debian 12 and Ubuntu 24.04 | Fail to demonstrate | The two container userlands ran the same folder. They shared LinuxKit 7.0.14, so the stock 6.1 and 6.8 kernels remain untested. |
| Same isolation on all three without cgroups, Landlock, seccomp or capabilities | Fail as a production replacement | The Wasm guest has the same import list on every build. Both containers had zero capabilities and no seccomp filter. The current Kernel still needs development mode to select the runner without Linux containment. |
| Cold instantiate below 250 ms | Pass | The maximum from two 30-process runs on macOS was 128.708 ms. The two Linux maxima were 67.356 and 88.215 ms. |
| Slowest of 200 cases below 250 ms | Fail | The 30-repeat maxima were 1,811.30 ms on macOS, 1,269.86 ms in Debian, and 1,194.59 ms in Ubuntu. |
| Warm trivial handler p50 below 5 ms | Fail | Median warm IPC exceeded the guard by 33.770 to 76.546 ms on macOS, 25.940 ms in Debian, and 26.701 ms in Ubuntu. |

These verdicts describe the first round, which parsed the app ESM on every request. The follow-up below measures the cached app bytecode and adds two baselines.

## Follow-up with cached app bytecode

On November 22, 2026, I reran the IPC, Hono and transaction probes on the same Apple M3 Pro. The paired macOS runs took place from 21:02:11 to 21:03:05 CET. They ran in sequence, with no other benchmark from this thread running at the same time. The machine was shared. Its one-minute load average ranged from 18.37 to 28.76 at the start of these runs, on 11 logical CPUs. macOS used Rust 1.91.0, Python 3.14.5 and Node 26.0.0. Docker used the same LinuxKit kernel, nine vCPUs, zero effective capabilities and seccomp disabled. It used Node 22.23.5 and Python 3.11.2 on Debian, and Python 3.12.3 on Ubuntu.

The portable reactor SHA-256 for this round was `97165e6a85f0339df2f4f2e218b19b9d2368bf5a2ca9ded27d953adfab365f23`. The macOS build weighed 8,341,488 bytes including a 5,686,336-byte compiled reactor. The Linux build used the crate's locked Cargo release profile and weighed 8,715,344 bytes including a 5,751,824-byte compiled reactor. The prior Linux binary in the size table included debug information. The deno_core baseline used the unchanged macOS release binary, 80,028,256 bytes, with SHA-256 `ea0605d9bffc59cbf64fb41f6b595c05a5492d9ffcf20a8fecc7db2658df2066`.

### Same-harness IPC and Hono

The new IPC cold totals include app compilation before Ready. They still use the trivial bundle. The suite waits for its Hono workers to report Ready before it starts timing cases, so the suite measures the new warm path and excludes one-time Hono compilation. Each fresh store still decodes and evaluates both the Web bootstrap and the app bytecode. Timing includes their work, guest calls, host IPC and scheduling. The Hono case interval also includes two SQLite snapshots. The prior rows came from the earlier measurement period and show the scale of the change. The paired macOS deno_core and cached QuickJS rows are the same-hour comparison.

| Environment and runner | Cold first reply max, ms | Warm IPC GET p50 / p95, ms | Worker RSS before / after, KiB |
| --- | ---: | ---: | ---: |
| macOS QuickJS, source parsed per request | 102.376 / 128.708 across two runs | 81.546 / 168.726 and 38.770 / 57.461 | 14,048 / 12,928 and 14,080 / 16,688 |
| macOS QuickJS, cached bytecode | 62.416 | 21.560 / 22.588 | 14,048 / 16,592 |
| macOS deno_core | 50.049 | 13.634 / 18.654 | 4,192 / 30,208 |
| Debian 12 QuickJS, source parsed per request | 67.356 | 30.940 / 36.113 | 16,032 / 18,308 |
| Debian 12 QuickJS, cached bytecode | 68.204 | 22.181 / 26.529 | 14,164 / 16,356 |
| Ubuntu 24.04 QuickJS, source parsed per request | 88.215 | 31.701 / 50.898 | 16,260 / 18,536 |
| Ubuntu 24.04 QuickJS, cached bytecode | 88.323 | 32.146 / 40.494 | 14,400 / 16,600 |

The macOS IPC runs both started at a one-minute load average of 28.76. The Hono suite started at 27.02 for QuickJS and 19.50 for deno_core. The Debian container ran from 21:03:12 to 21:04:45 CET. Host load rose from 16.70 to 99.70 while the VM reported 1.40 at IPC start and 1.37 at suite start. Ubuntu ran from 21:04:45 to 21:05:19 CET. Host load went from 99.70 to 59.63 and VM load was 1.99 at the start of both probes. The host and VM load averages cover different schedulers.

| Environment and runner | Full suite pass / fail / skip | Slowest passing case in full suite | Highest max of the two 30-repeat cases |
| --- | ---: | --- | --- |
| macOS QuickJS, source parsed per request | 200 / 0 / 7 | 4,101.45 ms, `a.authorization.alice.POST` | 1,811.30 ms |
| macOS QuickJS, cached bytecode | 200 / 0 / 7 | 278.55 ms, `b.body.exact-limit` | 349.70 ms, `b.body.exact-limit` |
| macOS deno_core | 200 / 0 / 7 | 118.69 ms, `a.body.invalid.8` | 143.89 ms, `a.internal-error.rollback-after-update` |
| Debian 12 QuickJS, source parsed per request | 200 / 0 / 7 | 1,051.27 ms, `a.body.exact-limit` | 1,269.86 ms |
| Debian 12 QuickJS, cached bytecode during host load spike | 195 / 5 / 7 | 4,215.34 ms, `b.update.bob.10` | 221.69 ms, `b.update.bob.10` |
| Debian 12 QuickJS, cached bytecode on retry | 200 / 0 / 7 | 326.57 ms, `a.body.exact-limit` | 314.01 ms, `a.body.exact-limit` |
| Ubuntu 24.04 QuickJS, source parsed per request | 200 / 0 / 7 | 1,023.67 ms, `a.body.exact-limit` | 1,194.59 ms |
| Ubuntu 24.04 QuickJS, cached bytecode | 200 / 0 / 7 | 423.36 ms, `a.body.exact-limit` | 291.02 ms, `a.body.exact-limit` |

Each selected case passed all 30 repeats on every follow-up series in this table. Debian's full sweep logged three 5 s client timeouts and two state mismatches. Two late POST commits appeared in the next case's SQLite snapshot, including an extra audit row on a subsequent POST. The passing repeats took place after the host load spike. The repeat max therefore coexists with a slower and partly failing full sweep. A retry using a single-file bind failed to start Node's workers with `spawn ENOTDIR`. Mounting the release directory read-only let the same binary run. The successful retry ran 21:13:18 to 21:13:52 CET, with host load falling from 38.47 to 29.79 and VM load rising from 1.20 to 1.65. It passed all 200 cases. Its second selected case, `b.internal-error.recovery`, passed all 30 repeats with a 176.07 ms maximum.

The macOS transaction series ran from 21:02:57 to 21:03:05 at a one-minute load average of 18.37 to 18.90. Both workers handled 30 verified transactions after warm-up through each driver. The direct IPC POST includes a SQLite transaction in the probe process. The HTTP series routes through the checkout Kernel, signs in an app user, updates the row and commits an idempotency receipt.

| macOS runner | Warm IPC SQLite POST p50 / p95, ms | Worker RSS before / after, KiB | Warm Kernel HTTP GET p50 / p95, ms | Warm Kernel HTTP POST p50 / p95, ms |
| --- | ---: | ---: | ---: | ---: |
| QuickJS, source parsed per request | 25.950 / 29.633 | 14,032 / 16,720 | 38.420 / 96.266 | 61.845 / 100.981 |
| QuickJS, cached bytecode | 23.320 / 32.968 | 14,032 / 16,656 | 26.932 / 35.469 | 33.892 / 46.735 |
| deno_core | 14.230 / 15.679 | 4,192 / 29,696 | 16.404 / 22.705 | 20.134 / 29.966 |

Both follow-up HTTP runs retained the same pair of worker PIDs across the series. Each GET returned `wasm-ok` and the POST counter advanced from 1 to 31 in an independent copy of the export.

### Pooled StarlingMonkey components

I reused the Wizer-initialized probe from the first round and built a second component from the 816,020-byte Hono orders ESM. ComponentizeJS 0.21.0 built the Hono component in 11.004 s wall time in one run. The build included Wizer initialization and writing the Wasm file. Wasmtime CLI 38.0.4 precompiled the probe in 3.89 s wall time and Hono in 6.67 s, one run each. Those builds took place around 20:58 to 21:00 CET, while the machine also ran Rust builds and the isolated MCP test. One-minute load snapshots during that period ranged from 35.96 to 49.98. These are build times.

The harness uses Wasmtime 38.0.4 with a pooling allocator and a 128 MiB maximum per linear memory. It uses `Component::deserialize_file`, the component equivalent of `Module::deserialize_file`. It loads the locally precompiled file and links once, warms up once, then creates a fresh store and instance for each of 30 calls in one process. The instantiate interval includes store creation. The call interval includes canonical argument transfer, JavaScript and post-return cleanup. The round trip adds export lookup and store drop. File mapping and linking are outside the loop. Lazy code or data faults can still occur while a call runs. The first call is shown separately. Both series ran at 21:02:57 CET with a one-minute load average of 18.37. [Wasmtime component deserialization](https://docs.rs/wasmtime/38.0.4/wasmtime/component/struct.Component.html#method.deserialize_file), [ComponentizeJS Wizer initialization](https://github.com/bytecodealliance/ComponentizeJS/blob/main/README.md).

| Component | Wasm / compiled bytes | Deserialize, ms | First round trip, ms | Warm instantiate p50 / p95, ms | Warm call p50 / p95, ms | Warm round trip p50 / p95, ms | RSS before / after, KiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| URL probe | 12,451,589 / 47,090,568 | 7.910 | 46.194 | 0.049 / 0.296 | 1.085 / 6.397 | 1.235 / 7.393 | 6,304 / 9,696 |
| Hono orders GET | 19,924,839 / 54,627,952 | 5.538 | 26.142 | 0.028 / 0.189 | 2.171 / 4.352 | 2.257 / 4.893 | 6,320 / 11,264 |

The original 2.202 s CLI wall time did not isolate instantiation. It included process startup, runtime setup, loading the native artifact and calling the export. The URL probe has zero imports. The Hono GET constructs a real request and checks the handler's returned JSON. Its sole host import returns a fixture row for Alice. The harness has no IPC, SQLite, auth lookup, POST adapter or runtime deadline enforcement. StarlingMonkey is a candidate on the direct instantiate and call measurements. A runner still needs to prove the full IPC path, the correctness suite and the resource and platform constraints.

### Follow-up commands

Run the IPC and suite probes in sequence for both macOS release binaries:

```sh
crate=kernel-rs/functions-runner-wasm
m=$crate/measure
quickjs=$crate/target/release/konae-functions-runner-wasm
deno=/tmp/konae-wasm-deno-target/release/konae-functions-runner
cargo build --release --locked --manifest-path "$crate/Cargo.toml" --bin konae-functions-runner-wasm

python3 "$m/protocol.py" "$quickjs" --runs 30
python3 "$m/protocol.py" "$deno" --runs 30 --skip-engine-size
python3 "$m/suite.py" "$quickjs" --runs 1 --repeat-slowest 30
python3 "$m/suite.py" "$deno" --runs 1 --repeat-slowest 30
```

The deno_core binary was preserved from the first round's locked release build. `--skip-engine-size` bypasses a Wasm-only metadata flag. For the two transaction tables, use `protocol.py RUNNER --post "$exported/server/count.mjs" --runs 30` and `PORT=18798 "$m/run-export.sh" KERNEL RUNNER "$exported"` for each runner. In Docker, rerun the IPC and suite commands with the new Linux binary mounted at `/runner`, using the container flags above. Build that binary with Rust 1.91.0 on the Debian Bookworm builder using `cargo build --locked --release -j 6 --bin konae-functions-runner-wasm`. The Debian retry mounted the release directory with `--mount type=bind,src="$linux_release_dir",dst=/bench-bin,readonly` and set the runner path to `/bench-bin/konae-functions-runner-wasm`.

Build and benchmark the StarlingMonkey components with the pinned ComponentizeJS and Wasmtime tools. Here `assets` is the directory containing the initial `starling.wasm` probe, and `wasmtime` names the 38.0.4 CLI.

```sh
npm install --prefix "$assets" --no-audit --no-fund @bytecodealliance/componentize-js@0.21.0
cp "$m/starling-build.mjs" "$assets/"
node "$assets/starling-build.mjs" spikes/functions-runtime/app/dist/orders.js "$assets/starling-orders.wasm"
cargo build --release --locked --manifest-path "$crate/Cargo.toml" --features starling-bench --bin starling-bench

"$wasmtime" compile -W gc-support=n -W threads=n -W stack-switching=n -O pooling-allocator=y \
  -o "$assets/starling-probe-pool.cwasm" "$assets/starling.wasm"
"$wasmtime" compile -W gc-support=n -W threads=n -W stack-switching=n -O pooling-allocator=y \
  -o "$assets/starling-orders-pool.cwasm" "$assets/starling-orders.wasm"

bench=$crate/target/release/starling-bench
"$bench" "$assets/starling-probe-pool.cwasm" 'http://example.test/path' /path 30
"$bench" "$assets/starling-orders-pool.cwasm" 'http://example.test/a/orders/1' \
  '{"status":200,"body":{"id":1,"ownerId":"alice","quantity":2}}' 30
```

To rebuild the original probe, run this from the assets directory after installing ComponentizeJS:

```sh
node --input-type=module <<'JS'
import { componentize } from '@bytecodealliance/componentize-js';
import { writeFile } from 'node:fs/promises';
const source = 'export function handle(url) { return new URL(url).pathname; }\n';
const witWorld = 'package konae:probe;\nworld runner { export handle: func(url: string) -> string; }\n';
const { component } = await componentize(source, {
  witWorld, disableFeatures: ['stdio', 'random', 'clocks', 'http', 'fetch-event'],
});
await writeFile('starling.wasm', component);
JS
```

The earlier full Bun and isolated MCP runs hit localhost connection failures and a stopped esbuild service while five threads shared the machine. These suites did not select the experimental runner, and the spike changed only its crate and this report, so those failures are not attributable to the spike. The follow-up ran `KONAE_MCP_TEST_PORT=58399 LITTLEBOX_MCP_TEST_PORT=58399 bun test --timeout 30000 tests/mcp.integration.test.ts` on macOS with Bun 1.3.11. All 54 tests passed in 213.65 s. The run took place around 20:56 to 21:00 CET alongside the component build and Rust compilation.

`cargo test --release --locked --manifest-path kernel-rs/functions-runner-wasm/Cargo.toml -- --test-threads=1` passed all 11 tests on macOS. The serial run took 1.70 s with host load at 60.97. An earlier parallel run passed 10 and missed the shutdown watchdog's reply deadline. The fresh-instance test now uses top-level await before each request checks the guest's globals and actor.

I reran `KONAE_TEST_FUNCTION_WORKER=<cached Wasm runner> KONAE_TEST_LEGACY_FUNCTION_WORKER=<frozen IPC v1 runner> cargo test -p kernel -- --ignored` from `kernel-rs/`. The 60.41 s run started at host load 57.05 and passed 4 tests. Six failed. Three still required copy fixtures. The two large-body tests returned 504 in place of 413 and reset a connection, respectively. The worker lifecycle test received `deadline_exceeded` but lost the worker PID after the cancellation. The real export/import test passed. The isolated retry with `cargo test -p kernel real_worker_reuses_actor_input_and_destroys_failed_and_idle_processes -- --ignored --test-threads=1` passed with the same runner at host load 24.76.

### Follow-up verdict

| Fixed clause | Verdict after caching app bytecode |
| --- | --- |
| Cold instantiate under 250 ms | Pass on all measured platforms. New maxima were 62.416 ms on macOS, 68.204 ms in Debian and 88.323 ms in Ubuntu. |
| Warm trivial handler p50 under 5 ms | Fail on all three. Cached QuickJS was 16.560 to 27.146 ms over the guard. The same-hour deno_core IPC baseline was 8.634 ms over it. |
| Slowest Hono case under 250 ms | Fail. Cached QuickJS exceeded the guard in the macOS and Ubuntu full sweeps and 30-repeat series. Debian exceeded it on the successful retry as well as during the host load spike. |
| Stock-platform isolation without Linux containment | The container results show the same guest boundary and zero effective capabilities. The native Debian and Ubuntu kernels and a production Kernel policy remain unverified. |

Cached QuickJS narrows the Hono latency gap and still misses the replacement criterion. The new StarlingMonkey measurements justify a full runner test. They establish that fresh pooled instances can execute the URL probe and one Hono GET within the warm median budget before IPC and Kernel work.
