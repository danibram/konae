# StarlingMonkey memory with many functions

A worker keeps one linked native component. Init checks and retains the read-only descriptors for the app version. The first request loads the component named by the invocation. Requests to the same object reuse it, including when two names share a device and inode. A switch drops the previous Store and native image before reading the replacement. Each request still gets a fresh Store. The worker retains the latest successful Store for its shutdown hook.

The Kernel verifies the prepared files before handing their descriptors to the worker. The runner checks native compatibility and links the allowed host imports when it first loads each image. It reads the inherited descriptor at offset zero on every load, so eviction and reload work without opening a path or compiling during a request. The app folder, preparation cache format and IPC/3 messages stay the same.

## Baseline

I ran the unmodified `e162cc7f` runner on an Apple M3 Pro with 18 GiB of RAM and macOS 26.6.1. The direct IPC probe sent `InitPrepared` with 1, 3, 6 or 18 distinct inodes. Each contained the same 50,236,240-byte prepared StarlingMonkey image for a handler that returns `ok`. Cloning the file to distinct inodes exercises the baseline's per-component loading and keeps the code and guest workload constant. I sent one GET invocation to each name in sequence.

I sampled RSS with `proc_pidinfo(PROC_PIDTASKINFO)`, the same reading the executor uses on macOS. The probe attempted a sample every 2 ms while waiting for each reply and took a separate snapshot after each reply. I used `vmmap -wide -noCoalesce -interleaved` for the size of anonymous executable mappings and the read/write range within Wasmtime's linear memory pool. The latter measures the guest's accessible linear memory. It excludes virtual address guards and says nothing about how many pages the host currently keeps resident.

| Functions | Ready RSS, MiB | Peak sampled Init RSS, MiB | RSS after last GET, MiB | Executable mappings at Ready / after last GET, MiB | Guest linear memory at Ready / after each GET, MiB |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 102.19 | 102.19 | 24.48 | 18.16 / 18.16 | 0 / 9.06 |
| 3 | 200.86 | 200.86 | 24.69 | 54.47 / 54.47 | 0 / 9.06 |
| 6 | 196.14 | 258.06 | 24.92 | 108.94 / 108.94 | 0 / 9.06 |
| 18 | 389.66 | 398.56 | 24.59 | 326.81 / 326.81 | 0 / 9.06 |

macOS reported anonymous pages in the `vmmap` SWAP column between snapshots. The executable mappings stayed in place even when the measured RSS fell. Each native image includes 18.16 MiB of executable instructions and other sections that bring the image to 47.91 MiB. The baseline's file read buffer and Wasmtime's copy into anonymous memory can both contribute to Init RSS.

The executor checks RSS in `Worker::receive` while waiting for `Ready`, at least once per 5 ms receive interval while scheduled. Six images crossed 256 MiB in the sampled run and 18 crossed it before there was any guest linear memory. The direct IPC driver leaves that supervisor check out so it can observe every request. All 28 invocations returned 200 with the same 64 MiB guest limit. A replay through `Worker::spawn_prepared_with_policy` confirmed the refusal. The baseline got `worker_memory_limit` at Init with six images after 122 ms and with 18 after 172 ms. The fixed runner reached Ready for all four counts, then returned 200 for every invocation with the default heap and RSS limits and a 1 s budget per request.

## Decision

Use one linked component per worker. The runner already executes requests serially and allocates at most one component instance at a time. One linked image lets the worker reuse it for adjacent requests while keeping the temporary read buffer and its native copy from overlapping with other images. The process RSS cap stays at 256 MiB and the guest limit stays at 64 MiB. Keep native pages in the RSS total, since they occupy memory and the full image plus the read buffer explains the Init failure without a change to either limit. Loading a different function spends part of its request deadline on a read and link.

The first lazy-loading build still needed a median 1,226 ms per switch over 24 direct IPC switches without `vmmap`. A four-second macOS `sample` run attributed 1,345 of 1,560 main-thread samples to `__deregister_frame` under `CodeMemory::drop`. StarlingMonkey on macOS now compiles without native unwind records and includes that setting in the native cache fingerprint. The Kernel builds new native files during preparation. Wasmtime's guest backtraces and JavaScript handler stacks use their existing paths. The Linux compiler keeps its existing native unwind setting.

After recompiling the same portable fixture, the same 24 switches took a median 140 ms and a maximum 394 ms in a run without `vmmap`. All 28 invocations returned 200. The highest sampled RSS was 3.05 MiB at Ready and 107.28 MiB during requests.

The repeat with `vmmap` measured zero component executable mappings at Ready and one 18.16 MiB component executable mapping after every request, for all four function counts. The guest had 9.06 MiB of accessible linear memory after each request. The native file with unwind records disabled weighed 46.57 MiB.
