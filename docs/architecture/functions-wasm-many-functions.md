# StarlingMonkey memory with many functions

A worker keeps one linked native component. Init checks and retains the read-only descriptors for the app version. The first request loads the component named by the invocation. Requests to the same object reuse it, including when two names share a device and inode. A switch drops the previous Store and native image before reading the replacement. Each request still gets a fresh Store. The worker retains the latest successful Store for its shutdown hook.

The Kernel verifies the prepared files before handing their descriptors to the worker. The runner checks native compatibility and links the allowed host imports when it first loads each image. It reads the inherited descriptor at offset zero on every load, so eviction and reload work without opening a path or compiling during a request. The app folder, preparation cache format and IPC/3 messages stay the same.

## Baseline

I ran the unmodified `e162cc7f` runner on an Apple M3 Pro with 18 GiB of RAM and macOS 26.6.1. The direct IPC probe sent `InitPrepared` with 1, 3, 6 or 18 distinct inodes. Each contained the same 50,236,240-byte prepared StarlingMonkey image for a handler that returns `ok`. Cloning the file to distinct inodes exercises the baseline's per-component loading and keeps the code and guest workload constant. I sent one GET invocation to each name in sequence.

I sampled RSS with `proc_pidinfo(PROC_PIDTASKINFO)`, the same reading the executor uses on macOS. The probe attempted a sample every 2 ms while waiting for each reply and took a separate snapshot after each reply. I used `vmmap -wide -noCoalesce -interleaved` for the size of anonymous executable mappings and the read/write range within Wasmtime's linear memory pool. The latter measures the guest's accessible linear memory. It excludes virtual address guards and says nothing about how many pages the host currently keeps resident.

| Functions | Ready RSS, MiB | Peak sampled Init RSS, MiB | RSS range after GETs, MiB | Executable mappings at Ready / after each GET, MiB | Guest linear memory at Ready / after each GET, MiB |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 102.19 | 102.19 | 24.48 | 18.16 / 18.16 | 0 / 9.06 |
| 3 | 200.86 | 200.86 | 24.59 to 24.69 | 54.47 / 54.47 | 0 / 9.06 |
| 6 | 196.14 | 258.06 | 24.72 to 25.00 | 108.94 / 108.94 | 0 / 9.06 |
| 18 | 389.66 | 398.56 | 20.09 to 25.09 | 326.81 / 326.81 | 0 / 9.06 |

macOS reported anonymous pages in the `vmmap` SWAP column between snapshots. The executable mappings stayed in place even when the measured RSS fell. Each native image includes 18.16 MiB of executable instructions and other sections that bring the image to 47.91 MiB. The baseline's file read buffer and Wasmtime's copy into anonymous memory can both contribute to Init RSS.

The executor checks RSS in `Worker::receive` while waiting for `Ready`, at least once per 5 ms receive interval while scheduled. Six images crossed 256 MiB in the sampled run and 18 crossed it before there was any guest linear memory. The direct IPC driver leaves that supervisor check out so it can observe every request. All 28 invocations returned 200 with the same 64 MiB guest limit. A replay through `Worker::spawn_prepared_with_policy` confirmed the refusal. The baseline got `worker_memory_limit` at Init with six images after 122 ms and with 18 after 172 ms. The fixed runner reached Ready for all four counts, then returned 200 for every invocation with the default heap and RSS limits and a 1 s budget per request.

## Decision

Use one linked component per worker. The runner already executes requests serially and allocates at most one component instance at a time. One linked image lets the worker reuse it for adjacent requests while keeping the temporary read buffer and its native copy from overlapping with other images. The process RSS cap stays at 256 MiB and the guest limit stays at 64 MiB. Keep native pages in the RSS total, since they occupy memory and the full image plus the read buffer explains the Init failure without a change to either limit. Loading a different function spends part of its request deadline on a read and link.

The first lazy-loading build still needed a median 1,226 ms per switch over 24 direct IPC switches without `vmmap`. A four-second macOS `sample` run attributed 1,345 of 1,560 main-thread samples to `__deregister_frame` under `CodeMemory::drop`. StarlingMonkey on macOS now compiles without native unwind records and includes that setting in the native cache fingerprint. The Kernel builds new native files during preparation. Wasmtime's guest backtraces and JavaScript handler stacks use their existing paths. The Linux compiler keeps its existing native unwind setting.

After recompiling the same portable fixture, the same 24 switches took a median 140 ms and a maximum 394 ms in a run without `vmmap`. All 28 invocations returned 200. The highest sampled RSS was 3.05 MiB at Ready and 107.28 MiB during requests.

The repeat with `vmmap` measured zero component executable mappings at Ready and one 18.16 MiB component executable mapping after every request, for all four function counts. The guest had 9.06 MiB of accessible linear memory after each request. The native file with unwind records disabled weighed 46.57 MiB.

| Functions | Ready RSS, MiB | RSS range after GETs, MiB |
| ---: | ---: | ---: |
| 1 | 3.02 | 105.86 |
| 3 | 3.05 | 105.81 to 106.25 |
| 6 | 3.03 | 105.78 to 105.97 |
| 18 | 3.03 | 105.86 to 106.19 |

## File verification before worker launch

The first `konae dev` run with the fixed worker and all 18 spike functions returned 504 `deadline_exceeded` on every route. Usage recorded zero handler time. The Kernel spent most of the budget before Ready reading and verifying about 846 MiB of native files.

I timed the same 64 KiB file reads and SHA-256 computation as the Kernel against the prepared cache. A single reader took 1,103 ms and 841 ms. Two readers took 487 ms. Four readers took 195 ms and 283 ms. The Kernel now uses up to four readers for apps with more than four functions. It checks each alias and hashes each open file before passing that same read-only descriptor to the worker. Small apps still verify their files in order.

The first two requests in the run with parallel verification timed out before worker initialization. The first request was parsing all JavaScript bundles again. The Kernel now checks their hashes against the metadata it already validated at publish or startup. The release and startup paths still parse every bundle. The request path still checks the manifest, file layout and all bundle digests.

## Node compatibility over HTTP

I built the 18-function Node compatibility spike with `bun spikes/node-compat/build.mjs "$spike"`. The Rust helper prepared the development bundles. The fixed runner's `--compile-component` produced the native files. I primed a private cache and started `bun scripts/konae.ts dev --dir "$spike/cli"` with the fixed Kernel and StarlingMonkey binaries. The Kernel checked the prepared cache and kept the default 1 s deadline.

I sent one GET per route after startup and compared each successful response body with the spike's expected value. The first request had a fresh worker. Each subsequent route used its component for the first time. After `/api/jose` timed out, the Kernel launched a second worker for `/api/bufferEvents`.

| GET path | HTTP | First request, ms |
| --- | ---: | ---: |
| `/api/zod` | 200 | 318.05 |
| `/api/hono` | 200 | 26.83 |
| `/api/dateFns` | 200 | 23.76 |
| `/api/uuid` | 200 | 22.02 |
| `/api/nanoid` | 200 | 28.24 |
| `/api/jose` | 504 | 1,002.45 |
| `/api/bufferEvents` | 200 | 254.20 |
| `/api/stream` | 200 | 26.80 |
| `/api/createHash` | 200 | 25.06 |
| `/api/dependencyMode` | 200 | 24.09 |
| `/api/web` | 200 | 29.53 |
| `/api/modules` | 200 | 22.64 |
| `/api/bareBuiltin` | 200 | 20.19 |
| `/api/npmProcess` | 200 | 21.74 |
| `/api/commonJsEvents` | 200 | 22.97 |
| `/api/dynamicBuiltin` | 200 | 39.59 |
| `/api/reexportBuiltin` | 200 | 36.93 |
| `/api/typeOnlyBuiltin` | 200 | 33.30 |

On separate starts of the same 18-function app, `/api/web` returned 200 as the first function request in 933.42 ms. An earlier cold attempt returned 504 in 1,016.50 ms, with 842 ms recorded in worker initialization. The first request to `/api/jose` on its own fresh start returned 504 in 1,005.21 ms.

`GET /api/jose` returned `{"code":"deadline_exceeded","error":"The function exceeded its execution budget."}`. The Kernel recorded the timeout during the invocation. I ran the exact prepared JOSE component three times on each worker build through direct IPC without a parent deadline. All six replies were 200 with `{"ES256":"konae","RS256":"konae"}`. They used 1.54 to 1.87 seconds of process CPU per call on this Mac, measured with `proc_pid_rusage` and the Mach timebase. A five-second stack sample placed the main thread in guest code under `Invocation::invoke`. The 18-route acceptance check still needs JOSE to finish within 1 s.

## Six resources over HTTP

I opened the same app folder with the Kernel binary on each runner. Six TypeScript handlers read orders, customers, inventory, shipments, invoices and returns. Each uses an anonymous named Server Action with a `LIMIT 10` query against its own SQLite table. The six first requests and 24 further requests returned 200 on each runner. For example, `GET /api/orders` returned `{"resource":"orders","rows":[{"id":1,"label":"orders-1"}]}`.

I took the idle sample one second after the sequential first requests. The load sent four sweeps of the six routes with two concurrent clients. There were two worker processes under load in both runs. The combined peak sums worker RSS within a single sample and can count shared pages for both workers. The timings include HTTP and SQLite.

| Runner | Idle RSS, MiB | Peak RSS per worker, MiB | Peak combined RSS, MiB | Cold first GET, ms | 24 GETs elapsed / slowest GET, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| deno_core | 16.84 | 21.97 | 42.83 | 204.70 | 228.98 / 69.42 |
| StarlingMonkey | 107.52 | 107.75 | 214.88 | 192.59 | 595.67 / 162.85 |

## Streaming before and after the request Web APIs

The shared Mac's trunk gate expired the 60 s budget in `body_chunks_keep_their_bytes_under_the_default_heap` after 29,884,416 bytes. I compared the first parent of the merge, `1ddce22f`, with `e162cc7f`. This includes the runner and preparer changes from `78af201b` and `c3831834`. The N3 tip, `089b6c86`, changes only the Kernel's validation and shared cases. Both release binaries used the same StarlingMonkey engine and Wasmtime 38.0.4. I prepared each component with the helper from its revision and used the same executor to run both.

I timestamped each call to the parent's body reader and measured the interval between successive 64 KiB chunks. That interval includes IPC, delivery to the guest and work the handler performs before asking for more. Each run started a fresh worker, sent 256 chunks and verified the response's length, first and last byte, and checksum. I interleaved five runs per side and mode after the builds and other suites in this worktree finished. The 11-core Mac was shared. Aggregate CPU idle during the checksum runs ranged from 38% to 61%. I used the worker's task CPU counters to distinguish its CPU time from elapsed time.

| Mode | Before, median chunk interval (range), ms | After, median chunk interval (range), ms | Before / after, worker CPU per chunk, ms |
| --- | ---: | ---: | ---: |
| Read and count bytes | 0.212 (0.204 to 0.220) | 0.206 (0.198 to 0.220) | 0.252 / 0.237 |
| Read and checksum every byte | 27.164 (27.058 to 27.645) | 27.163 (26.938 to 29.325) | 27.006 / 26.992 |

The ranges describe the median chunk interval from each run. The bytes-only handler removes the per-byte checksum loop and keeps the same request stream. The checksum loop dominates both builds. The measurements show no per-chunk slowdown from the Web API changes.

With the original fixture and its 60 s budget, both workers checked all 100,000,000 bytes and returned `{"total":100000000,"first":0,"last":245,"checksum":4078523648}`. The run before N3 took 41.40 s elapsed and 40.72 s of worker CPU. The run after N3 took 40.60 s elapsed and 40.16 s of worker CPU. The Mac reported 66% and 72% aggregate idle during those runs.

To isolate the effect of CPU availability, I repeated the full transfer with only the benchmark worker paused for 160 ms after each 40 ms run window. The pre-N3 worker returned `deadline_exceeded` after 23,592,960 bytes at 60.005 s. The post-N3 worker did the same after 21,757,952 bytes at 60.004 s. The two runs reproduce the failure by withholding execution time. The checksum needs about 41 s of worker CPU against the 60 s wall deadline. The comparison supports host load as the cause of the gate failure and calls for no change to the streaming path.
