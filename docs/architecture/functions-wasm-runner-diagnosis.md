# StarlingMonkey runner diagnosis

The portable Function must work when the app moves to a fresh installation with its accounts, data and rules. These runs use IPC/3 at `7d20877b`, ComponentizeJS 0.21.0 and Wasmtime 38.0.4 on macOS arm64. The [runtime contract](functions-wasm-runtime-contract.md) sets the expected behavior. I left the runner implementation unchanged. The three red tests in [worker_isolation.rs](../../kernel-rs/functions-runner-wasm/tests/worker_isolation.rs) reproduce the cache miss, body memory limit and streamed reply defects.

## The request waits for a build before Ready

[Round 2 changes three inputs to the cache key](../../kernel-rs/functions-runner-wasm/src/starling.rs#L235-L253): `dispatch.js`, `starling-runtime.js` and `starling.wit`. Recomputing the hashes at both commits gives `b8094d10…` at `be6fb18a` and `dba7db89…` at `7d20877b`. Even identical published ESM misses the old cache. A cold cache can fail in round 1 as well.

[The Init loop](../../kernel-rs/functions-runner-wasm/src/main.rs#L176-L195) calls `Invocation::pack` for every bundle before it sends Ready. On a miss, [the builder spawn and wait](../../kernel-rs/functions-runner-wasm/src/starling.rs#L275-L325) run ComponentizeJS with Wizer and native Wasmtime compilation under the request's 1,000 ms budget. The first input left in the cold cache during the GET was the count POST bundle. The supervisor was still waiting for Ready when the deadline expired.

I ran the following on a copy of a two-bundle app with a fresh cache. The launcher exports the pinned tool paths and points at the round 2 release runner. The HTTP driver uses the same running dev server and same exact bundle bytes for both paths.

```sh
KONAE_KERNEL_BIN="$PWD/kernel-rs/target/debug/kernel" \
  KONAE_FUNCTIONS_RUNNER="$launcher" KONAE_FUNCTIONS_DEVELOPMENT=1 \
  bun run konae -- dev --dir "$app" --port "$port" --events-port "$events_port"
curl -sS -w '\n%{http_code} %{time_total}\n' "$url/api/hello"
for bundle in "$app"/server/*.mjs; do "$launcher" --prepare "$bundle"; done
# Launch subsequent workers with the builder variable unset.
curl -sS -w '\n%{http_code} %{time_total}\n' "$url/api/hello"
```

| Path | Observed result |
| --- | --- |
| Cold `GET /api/hello` | 504 `deadline_exceeded` in 1.029 s |
| Standalone `--prepare` | Count 7.789 s, hello 7.327 s |
| Prepared `GET /api/hello`, builder disabled | 200 `wasm-ok` in 32.2 ms, including worker start |
| Prepared `POST /api/count` with `{"amount":1}` and an idempotency key | 200 `{"n":162}` in 16.4 ms after mutation and query |
| Prepared `GET /api/hello?large=1` | 500 `function.output-too-large` in 34.3 ms. Direct IPC returned all 2,097,152 bytes. |

Removing build work from Init restores the small GET with the round 2 reply code and process group intact. Move preparation to the build slice's release, import, startup and dev boundaries. Make runtime lookup cache-only and refuse a missing prepared component promptly. Keep the existing IPC/3 handshake and request deadline. The Kernel's [HTTP validation](../../kernel-rs/kernel/src/functions/invocation.rs#L695-L733) caps both the body and its serialized envelope at 1 MiB. Use the 2 MiB case to prove IPC success and HTTP refusal.

## Two guest allocations grow during a body read

The [WIT import](../../kernel-rs/functions-runner-wasm/guest/starling.wit#L5) returns `list<u8>`. Wasmtime lowers Rust's result into the guest by calling guest `cabi_realloc` for the payload. ComponentizeJS also allocates an eight-byte pointer/length record in the native import trampoline. The generated JS binding reads that record and does `new Uint8Array($memory.buffer.slice(ptr, ptr + len))`. The slice makes a second payload allocation inside the guest. The JS array remains safe if the caller retains it. The generated lift releases neither the payload nor the record. `post_call` frees the embedding's tracked export allocations. These import allocations never enter that list. The JS copy is GC-managed and can outlive a pull even after it becomes unreachable. [Pinned embedding](https://github.com/bytecodealliance/ComponentizeJS/blob/5be3a668d3c0612adf93592cbc8b91d4a6190c9d/embedding/embedding.cpp#L128-L149), [native trampoline](https://github.com/bytecodealliance/ComponentizeJS/blob/5be3a668d3c0612adf93592cbc8b91d4a6190c9d/crates/spidermonkey-embedding-splicer/src/splice.rs#L654-L682)

[The stream pull](../../kernel-rs/functions-runner-wasm/guest/dispatch.js#L19-L25) enqueues the existing `Uint8Array` on this path. It does not allocate another byte buffer. The polyfilled stream queues the object and the default reader consumes it. The host IPC byte vector stays outside guest linear memory.

I ran `B1_TRACE=1 python3 b1-ipc.py "$trace_runner" b1-count.mjs count --size 20000000 --cache-only`. The trace runner delegates to the original Wasmtime Store limiter. It records allowed linear-memory growth and samples the last allowed byte size at entry to every 64 KiB BodyRead. The handler counts chunks and bytes and holds only the first chunk to detect aliasing. `linear-refused` records the current size and the requested size when the limiter traps.

| BodyRead, before receiving the next chunk | Baseline linear MiB | Fixed buffer only | Fixed buffer and GC |
| ---: | ---: | ---: | ---: |
| 1 | 9.06 | 9.06 | 9.06 |
| 32 | 13.19 | 11.31 | 11.31 |
| 64 | 19.19 | 15.31 | 11.56 |
| 128 | 31.19 | 23.31 | 11.56 |
| 256 | 55.19 | 39.31 | 11.56 |
| 300 | 63.44 | 44.81 | 11.56 |
| 303 | 64.00 | | 11.56 |
| 1,527, EOF | | | 11.56 |

Baseline trapped after 303 chunks, 19,857,408 bytes, when Wasmtime refused growth from 67,108,864 to 67,174,400 bytes. Sol A2 recorded 19,988,480 bytes with a different fixture. From pulls 64 to 256, baseline grew by about 192 KiB per pull. The fixed allocator saved 64 KiB per pull in that interval. The remaining 128 KiB per pull measures linear growth, including allocator sizing and fragmentation. It does not imply two remaining 64 KiB byte copies.

## Give the canonical ABI transfer one fixed slot

Keep the WIT `list<u8>` and its automatic host copy. Specialize the `canon lower` realloc for this one synchronous import. Allocate one guest-owned 64 KiB slot lazily, validate the allocation arguments and return the same pointer for each read. Use a fixed eight-byte record for this trampoline or free that record after lifting. Keep a fresh JS-owned chunk on every pull, with no scratch view escaping to JavaScript. Leave IPC capability checks, the 64 KiB wire cap and the empty terminal frame in place. A resource handle would still need a transfer operation. A scalar pointer in WIT alone supplies neither access to Wasmtime's private component memory nor a JS allocation.

The diagnostic component changed the allocator with `b1-scratch-wat.py`. Scratch alone moved the trap to 29,687,808 bytes. Replacing the generated JS task wrapper with a direct copy did not move that boundary. With the same scratch allocation, `b1-gc-wat.py` called the engine's existing full-GC sequence before every 32nd native body read. The same driver then returned 200 for 100,000,000 bytes in 754 ms under tracing. It confirmed 1,526 data chunks and first/last marker bytes 0 and 245. Wasm linear memory stayed at 12,124,160 bytes from pull 64 through EOF. Collection can reclaim the JS copies. In this test their lifetime under the default GC schedule was too long for the Wasmtime cap. Retaining the first chunk did not make later reads change its bytes.

```sh
jco print "$portable" > base.wat
python3 b1-scratch-wat.py base.wat scratch.wat
python3 b1-gc-wat.py scratch.wat scratch-gc.wat
jco parse scratch-gc.wat -o scratch-gc.wasm
"$KONAE_STARLING_WASMTIME" compile --target aarch64-apple-darwin \
  -W gc-support=n -W threads=n -W stack-switching=n -W epoch-interruption=y \
  -O pooling-allocator=y -o "$private_native_path" scratch-gc.wasm
KONAE_STARLING_CACHE="$private_cache" B1_TRACE=1 \
  python3 b1-ipc.py "$trace_runner" b1-count.mjs count --cache-only
```

Use a named guest-native hook in the pinned embedding or tune its GC at body-byte checkpoints with equivalent measured bounds. The prototype copies disassembly tied to this exact engine binary. Production needs source-level bindings and a cache fingerprint that includes the allocator and GC changes. Keep short requests below the GC checkpoint. A handler that retains every chunk can still exhaust the heap.

I alternated five runs per variant with one warmed worker and 30 measured requests per GET or 13-byte POST. The driver timed Invoke through verified Reply over IPC and checked every body. Baseline GET p50 was 2.21 ms across runs (2.19 to 2.27). With scratch and GC it was 2.09 ms (2.06 to 2.12). POST moved from 3.01 ms (2.91 to 3.08) to 3.18 ms (2.95 to 3.44). The shared 11-core Mac reported a one-minute load of 49. These timings bound the small-request cost in the probe. They do not establish a speed difference or the Kernel's 1.5× guard.

The existing HTTP fixture's byte iterator also costs time. With the prototype, checksumming 1 MiB of `x` took 2.879 s with `for…of` and 0.421 s with indexed reads. Both returned the correct checksum. Verify throughput separately from memory and keep the 100 MB checksum assertion.

## Use the same stream type for Request and Response

[StarlingMonkey's entry](../../kernel-rs/functions-runner-wasm/guest/starling-entry.js#L5-L15) installs the JS stream polyfill and JS Request but leaves the native Response. The native Fetch constructor expects its own stream type. It coerces a polyfilled stream to text in the four-chunk test, which returned 200 with 23 bytes, `[object ReadableStream]`. That is the failure I reproduced. The earlier report recorded `worker_failed` from its stream probe.

Adding `Response: web.Response` to the guest's globals in a scratch builder made the new reply test pass at 256 KiB, 1 MiB, and 2 MiB with exact byte content. It refused 2 MiB plus one as `response_too_large`. Keep the host accumulator and one IPC Reply. Count before copying on both sides, validate aggregate UTF-8 and strip a leading BOM once to match the prior decoder. The JS Response does not synthesize a length. Pass authored framing headers to validation and retire the heuristic that silently removes a matching length. Test split UTF-8, invalid UTF-8, a configured 1 MiB IPC limit and cancel during transfer. Release response-transfer scratch at each import or reuse a bounded guest slot.

## Keep the builder in the supervised group

I used `Worker::spawn_with_policy` with an 800 ms budget and a builder that recorded its PID and group, inherited stderr and slept five seconds. I toggled only the process-group branch at [starling.rs](../../kernel-rs/functions-runner-wasm/src/starling.rs#L297-L325). With the round 1 branch, the builder was its own group leader. Spawn returned `deadline_exceeded` after 5.786 s. With round 2, the builder shared the worker group. Spawn returned the same code in 0.802 s.

[Worker.stop](../../kernel-rs/functions-executor/src/supervisor.rs#L781-L799) kills the worker group, reaps the worker and joins its pipe readers. The old builder survived that signal and held the stderr pipe open until it exited. This reproduces the old cleanup mechanism with one child. Keep a separately supervised group for standalone preparation. Do not restore a separate builder group under the request worker.

The full ignored Kernel suite finished with five passes and six failures after priming finite bundles, including both auth versions. Export/import and the binary capacity check then passed. The auth test expected version C's 3.5 s top-level loop timeout in execution Usage. Usage showed 3,499.636 ms in initialization and zero in execution. Once preparation rejects nonterminating initialization before publish, keep that refusal test at the build boundary and keep the runtime deadline check on a handler loop. The 100 MB HTTP case lost its TCP connection. The other four failures need MinIO, pilot copies or the frozen IPC/1 binary.

The evidence archive `functions-wasm-runner-diagnosis-evidence.tar.gz`, stored beside the diagnosis report outside the repository, holds the scratch scripts, every per-pull sample, the HTTP transcript and test logs. It holds no component binaries. None of the diagnostic components belongs in a trusted production cache.
