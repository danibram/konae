# Wasm functions runner spike

The spike asks whether an exported app folder can carry its function code to another machine and keep the same isolation. The HTTP-facing Kernel must be able to run under an ordinary account on macOS, stock Debian 12 with Linux 6.1, and stock Ubuntu 24.04 with Linux 6.8. The first two rounds used 250 ms for a cold instantiate and the slowest case, and 5 ms for the warm median of a trivial GET. The final round compares StarlingMonkey against deno_core in the same harness and keeps the 250 ms cold reply guard.

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

| Middleware | QuickJS in Wasm | deno_core | StarlingMonkey |
| --- | --- | --- | --- |
| JWT | 500, `crypto is not defined` | Same | Same |
| Default ETag | 500, `crypto is not defined` | Same | Same |
| ETag with supplied digest | 200 with `ETag: "010203"` | Same | 500, `res.clone is not a function` |
| Cache | Route returned 200 and `cacheAvailable: false` | Same | Same |
| Signed cookie | 500, `crypto is not defined` | Same | Same |
| `streamText` with two writes | 200, buffered `onetwo` | Same | Same |
| `streamText` with `sleep(1)` between writes | 200, partial body `one` | No reply within the probe's 10 s limit | 200, partial body `one` |

Hono adds `Transfer-Encoding: chunked` to both stream variants in the QuickJS and deno_core probes. The executor rejects that hop-by-hop header. StarlingMonkey's direct replies omitted the header. All three workers use buffered UTF-8 replies. All three lack timers, WebCrypto and a persistent Cache API in this configuration. The StarlingMonkey column came from one request per route through `hono.py` at 21:32:27 CET on the same Mac, at a one-minute host load of 9.71.

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

## StarlingMonkey IPC v2 runner

The `starling-runner` Cargo feature selects StarlingMonkey. The worker still reads one bundle at Init under `konae-functions-ipc/2`. It derives the cache key from the source SHA-256, the pinned StarlingMonkey engine digest, Wasmtime 38.0.4 and the guest adapter and build files. The compiled filename also names the target. The worker loads and links the locally precompiled component once. On a cache miss it can call the configured builder, which runs ComponentizeJS and Wizer, then precompiles the component with Wasmtime. Both build outputs enter the cache through renames. The runtime requires an operator-owned cache and reads native code only from that directory.

Each Invoke uses a fresh Store and a pooled instance of the Wizer snapshot. The snapshot has already evaluated the module, so a request starts from the same top-level state. The previous successful Store lives until the next Invoke or Shutdown so its shutdown hook can run on retirement. The shared bridge checks capability nonces, pulls up to 64 KiB of body data per read, invokes named actions and forwards failure codes. It buffers the UTF-8 response within `WorkerLimits.response_bytes`. StarlingMonkey inserts a length for buffered responses. The adapter removes a length that matches the buffered reply so the executor can set HTTP framing. An incorrect length still reaches the executor's refusal. This also normalizes an explicitly supplied matching length.

Wasmtime limits linear memory with `WorkerLimits.heap_bytes`, 64 MiB in these probes. Epoch interruption handles the executor's Cancel and the shutdown watchdog. IPC v2 carries no deadline value, so the parent must send Cancel when its request budget expires. The component links only the worker imports and one clock function for `Date.now()`. Wizer uses zero for that clock. ComponentizeJS disables its WASI clocks, entropy, networking and stdio. The guest adapter removes timers, fetch, WebCrypto and `performance`. JavaScript still has `Math.random()` with the generator state in the snapshot. `Date.now()` inside a handler uses wall time. General `Date` constructor behavior and randomness across independent invocations need parity tests.

### Build and measurement commands

Run from the repository root. `exported` is the two-function app folder used in the earlier HTTP proof. `tools`, `bundles` and the operator-owned cache are absolute paths outside the app folder. Set `KONAE_STARLING_WASMTIME` to the Wasmtime 38.0.4 CLI.

```sh
crate=kernel-rs/functions-runner-wasm
m=$crate/measure
cargo build --release --locked --manifest-path "$crate/Cargo.toml" \
  --features starling-runner --bin konae-functions-runner-wasm
starling=$PWD/$crate/target/release/konae-functions-runner-wasm

mkdir -p "$tools" "$bundles" "$cache"
npm install --prefix "$tools" --no-audit --no-fund \
  @bytecodealliance/componentize-js@0.21.0 esbuild@0.25.12 es-module-lexer@1.7.0
export KONAE_STARLING_TOOLS="$tools"
export KONAE_STARLING_CACHE="$cache"
export KONAE_STARLING_BUILDER="$PWD/$m/starling-component.sh"
node "$m/legacy-host.mjs" --write-bundles "$bundles"
printf '%s' "export function handle() { return new Response('ok'); }" >"$bundles/trivial.mjs"
cp "$exported/server/count.mjs" "$exported/server/hello.mjs" "$bundles/"
for name in trivial count hello orders preferences middleware; do
  "$starling" --prepare "$bundles/$name.mjs"
done

# Use each portable .wasm path printed by --prepare for the Linux cache.
"$KONAE_STARLING_WASMTIME" compile --target aarch64-unknown-linux-gnu \
  -W gc-support=n -W threads=n -W stack-switching=n -W epoch-interruption=y \
  -O pooling-allocator=y -o "${portable%.wasm}.aarch64-unknown-linux-gnu.cwasm" "$portable"

unset KONAE_STARLING_BUILDER
python3 "$m/hono.py" "$starling" "$bundles/middleware.mjs"
```

The middleware bundle uses the original Hono probe build command above. The Linux StarlingMonkey binary uses the same locked release build with Rust 1.91.0 in `rust:1.91-bookworm`. The deno_core baselines come from `main` at `502ead0e` and the startup snapshot commit `91add6dd`. Both Linux builds use the repository's `Dockerfile.functions-runner` through its build script. The macOS main baseline uses the locked release binary from the previous run. I verified that the worktree and main have identical runner and executor sources. These commands build the new binaries.

```sh
scratch=$(mktemp -d /tmp/konae-deno-comparison.XXXXXX)
mkdir -p "$scratch/main" "$scratch/snapshot"
for tree in main snapshot; do
  git archive 502ead0e Dockerfile.functions-runner scripts/build-functions-runner.sh \
    .dockerignore kernel-rs/Cargo.toml kernel-rs/backup/Cargo.toml \
    kernel-rs/kernel/Cargo.toml kernel-rs/konae/Cargo.toml | tar -x -C "$scratch/$tree"
done
git archive 502ead0e kernel-rs/functions-runner kernel-rs/functions-executor \
  | tar -x -C "$scratch/main"
git archive 91add6dd kernel-rs/functions-runner kernel-rs/functions-executor \
  | tar -x -C "$scratch/snapshot"
"$scratch/main/scripts/build-functions-runner.sh" arm64 "$scratch/linux-main"
"$scratch/snapshot/scripts/build-functions-runner.sh" arm64 "$scratch/linux-snapshot"
CARGO_TARGET_DIR="$scratch/macos-target" cargo build --release --locked -j 6 \
  --manifest-path "$scratch/snapshot/kernel-rs/functions-runner/Cargo.toml" \
  --bin konae-functions-runner
```

For each platform and round, run GET for all three engines, then POST for all three, then Hono for all three. Run each command with `runner` pointing at the corresponding binary. The six rounds use each possible engine order once.

```sh
python3 "$m/protocol.py" "$runner" --runs 30 --skip-engine-size
python3 "$m/protocol.py" "$runner" --post "$bundles/count.mjs" --runs 30
python3 "$m/suite.py" "$runner" --runs 1 --repeat-slowest 30
```

In Docker, the temporary `/bench/run-container.sh` script runs those commands with `m=/src/kernel-rs/functions-runner-wasm/measure` and the counter bundle at `/bench/count.mjs`. The runners are `/starling/konae-functions-runner-wasm`, `/main/konae-functions-runner` and `/snapshot/konae-functions-runner`. Use the `konae-wasm-debian12` and `konae-wasm-ubuntu2404` images.

```sh
docker run --rm --network none --user 65534:65534 --cap-drop=ALL \
  --security-opt seccomp=unconfined --security-opt no-new-privileges \
  --mount "type=bind,src=$PWD,dst=/src,readonly" \
  --mount "type=bind,src=$cache,dst=/cache,readonly" \
  --mount "type=bind,src=$linux_release_dir,dst=/starling,readonly" \
  --mount "type=bind,src=$scratch/linux-main,dst=/main,readonly" \
  --mount "type=bind,src=$scratch/linux-snapshot,dst=/snapshot,readonly" \
  --mount "type=bind,src=$scratch/bench-assets,dst=/bench,readonly" \
  -e KONAE_STARLING_CACHE=/cache -e PYTHONDONTWRITEBYTECODE=1 "$image" sh /bench/run-container.sh
```

For Kernel tests the supervisor clears the child environment. A launcher exports the absolute cache path and runs `exec /absolute/path/to/runner "$@"`. The launcher uses the cache alone. The ignored tests' finite bundles can be extracted with `python3 "$m/starling-kernel-bundles.py" "$bundles"` and prepared as above.

### Paired latency and RSS

The comparison ran on November 22, 2026 on an Apple M3 Pro with 18 GiB RAM and 11 logical CPUs. macOS 26.6.1 build 25G76 reported `uname -r` as `25.6.0`. Docker Desktop 29.9.3 ran both arm64 images on `7.0.14-linuxkit` with nine vCPUs. macOS used Node 26.0.0 and Python 3.14.5. Debian and Ubuntu used Node 22.23.5, with Python 3.11.2 and 3.12.3 respectively. macOS ran 21:48:15 to 21:50:58 CET at a one-minute host load of 14.08 to 47.90. Debian 12 Docker ran 21:50:59 to 21:53:21 CET at a one-minute VM load of 0.08 to 2.56. Ubuntu 24.04 Docker ran 21:53:21 to 21:55:35 CET at a one-minute VM load of 1.21 to 2.59.

Both containers reported UID and GID 65534, all five capability masks zero, `NoNewPrivs=1`, `Seccomp=0` and zero seccomp filters. They used read-only cache mounts, no network beyond loopback, no delegated cgroup and no Landlock setup. The runner needs an ordinary process, threads, pipes, clocks, memory and file mappings, including executable mappings for the native cache. These Linux runs used the Docker VM kernel. Stock Linux 6.1 and 6.8 remain untested.

Each comparison used the same Python driver, IPC v2, bundles and machine for all engines. Main and snapshot deno_core create a fresh isolate per invocation. The snapshot preloads the Web bootstrap at build time. StarlingMonkey creates a fresh Store and pooled instance for each invocation. All three reuse the process in the warm series. The earlier Linux measurements belong to the lab runner.[^lab-runner]

Each of six rounds had 30 cold and 30 warm GETs and 30 warm POSTs, each after one warm-up. Cold GET includes process launch through Init and first reply. For StarlingMonkey that covers loading and linking the precompiled component and making the first instance. The OS can retain mapped file pages between runs. Warm GET includes IPC, guest work and scheduling. POST also includes Python's SQLite transaction, action round trips and commit. The table uses medians of the six per-round p50s and p95s, and the largest measured cold reply. RSS is the median of six worker snapshots after Ready and after the series. It includes shared pages and excludes transient peaks.

| Platform and engine | Cold GET p50 / p95 / max, ms | Warm GET p50 / p95, ms | Warm POST p50 / p95, ms | GET RSS before / after, KiB | POST RSS before / after, KiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| macOS StarlingMonkey | 15.261 / 25.735 / 75.594 | 1.024 / 1.522 | 2.615 / 4.345 | 6,624 / 12,928 | 6,600 / 13,312 |
| macOS deno_core main | 29.430 / 43.933 / 79.302 | 14.843 / 18.483 | 15.566 / 20.297 | 4,192 / 30,152 | 4,192 / 29,936 |
| macOS deno_core with snapshot | 18.553 / 34.668 / 177.148 | 4.702 / 7.072 | 5.325 / 7.837 | 4,240 / 21,872 | 4,240 / 21,944 |
| Debian 12 Docker StarlingMonkey | 18.242 / 23.462 / 32.285 | 1.429 / 2.446 | 4.571 / 5.995 | 4,046 / 13,460 | 4,048 / 13,776 |
| Debian 12 Docker deno_core main | 34.885 / 48.115 / 204.262 | 18.103 / 22.004 | 20.157 / 25.956 | 7,512 / 38,366 | 7,516 / 38,570 |
| Debian 12 Docker deno_core with snapshot | 22.933 / 30.084 / 59.675 | 4.989 / 6.775 | 7.895 / 10.995 | 7,640 / 30,502 | 7,644 / 30,888 |
| Ubuntu 24.04 Docker StarlingMonkey | 17.907 / 22.838 / 29.760 | 1.117 / 1.447 | 3.837 / 5.230 | 5,564 / 14,984 | 5,564 / 15,250 |
| Ubuntu 24.04 Docker deno_core main | 34.668 / 47.085 / 185.129 | 14.747 / 18.900 | 19.438 / 28.507 | 7,572 / 38,540 | 7,576 / 38,594 |
| Ubuntu 24.04 Docker deno_core with snapshot | 20.426 / 24.572 / 60.920 | 4.335 / 5.429 | 6.566 / 10.594 | 7,700 / 30,664 | 7,698 / 31,070 |

Each Hono sweep ran 200 checks and skipped the seven cases marked out of scope. For each round, the verifier restored SQLite state and repeated the slowest case in the main sweep and the slowest in the rollback sweep. Each case had one warm-up and 30 measured repeats. The largest repeat maximum across all rounds counts for the guard. The interval includes local HTTP, routing, IPC, two SQLite snapshots and transactions where applicable. Case selection can differ by engine and round.

| Platform and engine | Passed / failed / skipped | Full sweep max, ms | Per-round repeat maxima, ms | Case of highest repeat |
| --- | ---: | ---: | ---: | --- |
| macOS StarlingMonkey | 1,200 / 0 / 42 | 20.44, `a.internal-error.rollback-after-update` | 9.40 to 36.28 | `a.internal-error.rollback-after-update` |
| macOS deno_core main | 1,200 / 0 / 42 | 219.49, `b.readback.alice.1` | 29.50 to 120.17 | `b.readback.alice.1` |
| macOS deno_core with snapshot | 1,200 / 0 / 42 | 122.40, `b.body.invalid.4` | 23.31 to 167.86 | `a.internal-error.rollback-after-update` |
| Debian 12 Docker StarlingMonkey | 1,200 / 0 / 42 | 157.30, `a.body.invalid.1` | 9.36 to 48.00 | `a.internal-error.rollback-after-update` |
| Debian 12 Docker deno_core main | 1,200 / 0 / 42 | 1080.47, `b.update.alice.1` | 43.11 to 114.80 | `b.update.bob.3` |
| Debian 12 Docker deno_core with snapshot | 1,200 / 0 / 42 | 398.79, `a.internal-error.rollback-after-update` | 34.12 to 119.74 | `a.authorization.alice.GET` |
| Ubuntu 24.04 Docker StarlingMonkey | 1,200 / 0 / 42 | 64.66, `a.authorization.alice.GET` | 9.17 to 26.42 | `a.internal-error.rollback-after-update` |
| Ubuntu 24.04 Docker deno_core main | 1,200 / 0 / 42 | 349.30, `b.id.POST.%20` | 51.17 to 133.85 | `b.body.invalid.0` |
| Ubuntu 24.04 Docker deno_core with snapshot | 1,200 / 0 / 42 | 148.40, `b.body.invalid.5` | 26.05 to 64.19 | `a.authorization.alice.GET` |

Every repeat passed. In the second macOS round, the StarlingMonkey Hono repeat maximum was 36.28 ms and the snapshot maximum was 23.31 ms, a ratio of 1.56×. The guard uses the highest repeat maximum for each engine across all six rounds. Both warm p50 ratios passed against the snapshot in every round on all three platforms. The per-round p50 ranges show the variation.

| Platform and engine | Per-round GET p50 range, ms | Per-round POST p50 range, ms |
| --- | ---: | ---: |
| macOS StarlingMonkey | 0.826 to 2.084 | 2.074 to 7.129 |
| macOS deno_core main | 12.718 to 25.398 | 13.344 to 30.085 |
| macOS deno_core with snapshot | 3.516 to 7.862 | 4.832 to 13.638 |
| Debian 12 Docker StarlingMonkey | 1.176 to 2.333 | 2.890 to 5.546 |
| Debian 12 Docker deno_core main | 14.271 to 23.140 | 16.315 to 30.794 |
| Debian 12 Docker deno_core with snapshot | 4.041 to 6.744 | 5.697 to 9.469 |
| Ubuntu 24.04 Docker StarlingMonkey | 1.071 to 1.254 | 2.956 to 7.305 |
| Ubuntu 24.04 Docker deno_core main | 14.088 to 21.157 | 15.560 to 32.799 |
| Ubuntu 24.04 Docker deno_core with snapshot | 3.820 to 5.163 | 5.410 to 12.222 |

[^lab-runner]: The earlier Docker comparison used the lab image's `/usr/local/lib/konae/functions-runner`, SHA-256 `9c74ed3d77259646477d3b6b1cd16daf40c17f257e00c6902da119d133a409b1`. The lab build record matches that hash to `konae-functions-spike` from base `43decb081494dbfe854327dd1279f9b9c69f17df` plus the frozen patch with SHA-256 `9be74c0a839436e5814df3ce67c25d0c8cf1523234bbabfea814b3dd430f39fd`. Its `spikes/functions-runtime/rust-host/src/worker.rs` creates one runtime before Ready and invokes it for every request. This isolate reuse makes those rows unsuitable for a fresh-isolate comparison.

    Debian 12 recorded cold GET p50/p95/max of 53.266/67.647/73.225 ms, warm GET p50/p95 of 0.425/2.132 ms and warm POST p50/p95 of 2.048/2.964 ms. GET RSS before/after was 37,476/38,196 KiB and POST RSS was 37,456/38,508 KiB. Hono passed 200 with zero failures and seven skips. The rollback case took 16.14 ms in the sweep and reached 7.12 ms over 30 repeats.

    Ubuntu 24.04 recorded cold GET p50/p95/max of 51.836/62.970/64.594 ms, warm GET p50/p95 of 0.401/1.112 ms and warm POST p50/p95 of 1.801/3.188 ms. GET RSS before/after was 37,632/38,336 KiB and POST RSS was 37,592/38,656 KiB. Hono passed 200 with zero failures and seven skips. The rollback case took 13.05 ms in the sweep and reached 5.68 ms over 30 repeats.

### Cache creation and component size

A single actual IPC Init for the trivial bundle in a new empty cache took 5,984.43 ms to Ready and 5,986.81 ms to the first reply. The command used `protocol.Worker(starling, TRIVIAL, init_timeout=70)` followed by an asserted `worker.get()`. It ran at 21:32:33 to 21:32:39 CET with macOS host load 10.13 to 11.08. This includes starting the Node build process, bundling, Wizer, native compilation, then loading and calling the component. The 250 ms criterion uses an existing precompiled artifact. Production must do the slow part before the first request.

The macOS StarlingMonkey release binary was 2,510,608 bytes. The Linux one was 2,946,040 bytes. Main deno_core weighed 80,028,256 bytes on macOS and 90,935,616 bytes on Linux. With the snapshot it weighed 84,677,088 and 95,597,552 bytes respectively. The runners' sizes came from `protocol.py`. The component sizes came from `--prepare` on macOS and `wc -c` after cross-compiling the portable artifacts for Linux.

| Bundle | Portable component bytes | macOS native bytes | Linux native bytes | macOS `--prepare` build, ms |
| --- | ---: | ---: | ---: | ---: |
| Trivial GET | 12,521,534 | 49,962,640 | 50,044,560 | 5,941.15 |
| Counter POST | 12,527,503 | 49,962,640 | 50,044,560 | Cache hit in the recorded batch |
| Exported hello | 12,522,945 | 49,962,640 | 50,044,560 | 4,546.89 |
| Hono orders | 19,930,305 | 57,499,280 | 57,581,200 | 5,363.36 |
| Hono preferences | 12,954,824 | 49,962,640 | 50,044,560 | 6,152.39 |
| Hono middleware | 13,311,171 | 49,962,640 | 50,044,560 | 5,315.58 |

Each build-time cell is one miss. `--prepare` times writing the input, bundling, Wizer, precompiling and installing both files. The finite bundles built at 21:24:14 to 21:25:08 CET at macOS host load 15.22 to 16.86. The separate trivial build finished at 21:22:58. The five Linux native files timed together each took 1.23 to 1.37 s with `time -p` around the cross-target `wasmtime compile` command. That step reused the portable snapshots and took place at 21:27:46 to 21:27:52 with macOS host load 11.26 at start. The later exported hello build used the same cross-target command.

To estimate the memory cost across app versions, I started one macOS IPC worker for the Hono orders component. I sampled `ps -o rss=` after Ready and after 30 successful `protocol.Worker.invoke` calls following a warm-up. The calls used `/a/orders/1` and answered the `readOrder` operation with Alice's fixture row. The series at 21:32:27 to 21:32:30 CET and host load 9.71 went from 8,672 to 17,632 KiB. Under IPC v2's one-worker-per-bundle model, 20 similar distinct warm components would sum to about 344 MiB of worker RSS. This extrapolation can count shared runner pages repeatedly and excludes request peaks and Kernel memory. Twenty Hono orders-sized components would also use about 380 MiB of portable artifacts and 1,097 MiB of macOS native cache. This disk estimate multiplies the file sizes above by 20.

### Export and test results

At 21:32:30 CET on macOS, `PORT=18811 "$m/run-export.sh" "$PWD/kernel-rs/target/debug/kernel" "$launcher" "$exported"` ran the exported app folder under the development Kernel and cache-only StarlingMonkey launcher. Host load was 9.71 at start and 10.13 at finish. The authenticated `GET /api/hello` returned HTTP 200 and `wasm-ok`. The first `POST /api/count` with `{"amount":1}` returned HTTP 200 and `{"n":1}`. The 30 subsequent transactions verified every increment through `{"n":31}`. Both worker PIDs stayed the same. The HTTP harness recorded warm GET p50/p95 of 6.109/10.103 ms and warm POST p50/p95 of 11.995/19.725 ms. This includes routing, authentication, IPC and scheduling. POST also includes the two named actions and the Kernel's idempotency and transaction work. An earlier attempt returned 502 because StarlingMonkey added `Content-Length`. The second run used the response adapter described above.

The two containers ran the same export with the packaged IPC v2 lab Kernel and `"$m/run-export.sh" /assets/kernel /assets/worker /export`. The export run used the same container security flags, mounted the Kernel and StarlingMonkey launcher under `/assets` and the app folder at `/export`, and added `--init`. Both completed the same GET and all 31 POSTs as UID 65534 with zero capabilities and seccomp disabled. Debian ran 21:37:27 to 21:37:30 CET, and Ubuntu 21:37:30 to 21:37:32. VM load was 6.14 to 6.41. Debian's repeated GET and POST p50/p95 were 26.553/67.619 ms and 29.330/84.200 ms. Ubuntu's were 21.621/35.912 ms and 28.383/47.665 ms. The Linux Kernel showed no idle worker children at the snapshot points. These HTTP figures may include worker startup and are outside the warm IPC guard.

| Command | macOS result |
| --- | --- |
| `cargo test --release --locked --manifest-path "$crate/Cargo.toml" --features starling-runner -- --test-threads=1` | 10 passed, 1 failed on the cache-hit rerun in 5.60 s. It finished at 21:34:57 CET and host load 18.59. The multipart file's bytes came back as the ASCII string `0255` in place of bytes 0 and 255. The initial run also failed the import-refusal test because the test builder could not find Node after the supervisor cleared its PATH. The rerun supplied the test builder's PATH and passed. |
| `cargo test --release --locked --manifest-path "$crate/Cargo.toml" -- --test-threads=1` | QuickJS passed 11 in 1.36 s after rebuilding the guest. |
| `KONAE_TEST_FUNCTION_WORKER="$launcher" KONAE_TEST_LEGACY_FUNCTION_WORKER="$legacy" cargo test -p kernel -- --ignored --test-threads=1` from `kernel-rs/` | 5 passed, 5 failed in 73.30 s. It ran 21:32:27 to 21:33:41 CET with host load 9.71 to 19.66. The export/import test, both worker lifecycle tests, the ABI mismatch check and the SQLite benchmark passed. Three copy tests lacked external credentials or fixtures. The oversized-body test returned 504 in place of 413, and the 100 MB stream test lost its TCP connection. The build-time handling of a top-level infinite loop still needs a direct parity test. The HTTP test stopped before it reached that assertion. |
| `cargo test -p kernel -p konae` from `kernel-rs/` | The Kernel passed 623, failed 1 and ignored 10 in 61.88 s. One copy test reported `WouldBlock` on a file lock, and Cargo stopped before running Konae. The isolated `cargo test -p kernel common_cut_releases_partial_inventory_on_space_and_reader_limits -- --test-threads=1` retry passed. `cargo test -p konae` separately passed 96 tests. The follow-ups ran from 21:35:09 to 21:35:36 CET at host load 17.26 to 25.29. |
| `CARGO_TARGET_DIR=/tmp/konae-wasm-deno-target cargo test --release --locked --manifest-path kernel-rs/functions-runner/Cargo.toml -- --test-threads=1` | The three unit tests passed. All eight isolation tests failed to spawn the runner because the cached harness embedded the old binary path. Rebuilding with `cargo rustc --release --locked --manifest-path kernel-rs/functions-runner/Cargo.toml --test worker_isolation -- -C opt-level=3` in the same target directory fixed the path. All eight passed when run serially. The retry took 1.85 s and ended at 21:40:51 CET and host load 6.35. |
| `KONAE_MCP_TEST_PORT=61308 LITTLEBOX_MCP_TEST_PORT=61308 bun test --timeout 30000` | Bun 1.3.11 reported 1,718 passed, 14 skipped and one failure with one import error. The unchanged Node baseline test imports `node:sqlite`, which Bun could not resolve. All 54 MCP integration tests passed in the full run. The suite took 453.98 s, from 21:32:27 to 21:40:01 CET, with host load 9.71 at start and 8.80 at finish. |
| `bun run build` | Passed. It ran immediately after the Bun suite and finished at 21:40:13 CET and host load 8.36. |

A separate IPC boundary test used `protocol.Worker` to return `new Response('x'.repeat(size))` with default worker limits. The runner refused 2,097,153 bytes as `response_too_large`. The two 2,097,152-byte requests both hit `worker_memory_limit` before replying. This was the cache-hit retry at 21:36:26 CET on the Mac at host load 20.31. The wire-size guard works, and this response path can exhaust the 64 MiB guest memory ceiling before the configured reply limit.

### Production gaps

`konae build` would componentize the bundle and ship the portable component and digest in `server/`. The destination would precompile verified Wasm into its own trusted cache. The Gate and Kernel need shared refusals for missing or mismatched artifacts, unsupported versions, unexpected imports or exports, and builds that exceed their limits or require disallowed host effects during initialization. Production still needs a Wasm launch policy, native-kernel tests, a plan for per-version cache cost, and parity for large bodies, response memory, multipart files and `Response.clone`. It also needs a rule for top-level state that Wizer freezes before any request. IPC v3 changes how the worker receives and selects bundles and reports handler errors.

## Final verdict

Warm ratios divide StarlingMonkey's time by the matching baseline. The guard is 1.5× or less. Cold passes if StarlingMonkey's maximum is under 250 ms. The snapshot rows decide the production verdict.

| Platform | deno_core baseline | Cold max, StarlingMonkey / baseline, ms | Warm GET ratio | Warm POST ratio | Slowest repeated Hono ratio |
| --- | --- | --- | --- | --- | --- |
| macOS | deno_core main | 75.594 / 79.302, pass | 0.07×, pass | 0.17×, pass | 0.30×, pass |
| macOS | deno_core with snapshot | 75.594 / 177.148, pass | 0.22×, pass | 0.49×, pass | 0.22×, pass |
| Debian 12 Docker | deno_core main | 32.285 / 204.262, pass | 0.08×, pass | 0.23×, pass | 0.42×, pass |
| Debian 12 Docker | deno_core with snapshot | 32.285 / 59.675, pass | 0.29×, pass | 0.58×, pass | 0.40×, pass |
| Ubuntu 24.04 Docker | deno_core main | 29.760 / 185.129, pass | 0.08×, pass | 0.20×, pass | 0.20×, pass |
| Ubuntu 24.04 Docker | deno_core with snapshot | 29.760 / 60.920, pass | 0.26×, pass | 0.58×, pass | 0.41×, pass |

StarlingMonkey passes the revised timing criterion against deno_core with snapshot on all three measured platforms. Keep deno_core with snapshot for production. StarlingMonkey is the Wasm engine to take forward once the stock-kernel checks, Web API and large-body gaps, and artifact and launch work are resolved. QuickJS remains rejected under its measured guards.
