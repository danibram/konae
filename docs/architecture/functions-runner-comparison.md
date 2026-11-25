# Function runner comparison

This comparison uses the same debug Kernel and the same Apple Silicon Mac for both runners. Timed requests ran during 2026-11-24 13:00-13:59 UTC. StarlingMonkey used Wasmtime 38.0.4, ComponentizeJS 0.21.0, and a fresh private cache for each cold preparation. The node compatibility fixture has separate CLI and editor bundles. Its two StarlingMonkey lanes each use 18 functions.

## Method

`measure_build.mjs` times the JS builder for each function. The StarlingMonkey preparation wrapper times `konae-functions-prepare` per function. That time includes component preparation and excludes the later native Wasmtime compilation. The app-level helper totals and native cache sizes are in `spikes/functions-comparison/*-preparation.json`. Deno has no native component cache.

`measure_http.py` and `measure_node_http.py` measure local HTTP request time through response-body read. Each warm route has 200 sequential requests. The clinic app uses 110 ms spacing. The public node fixture uses 500 ms spacing and an 11 second cooldown after its cold route requests to stay below its rate limit. One first request was measured for every function. For StarlingMonkey, the Kernel prepares components before it starts listening, so cold request time measures the first invocation after preparation. The node fixture has GET routes only, so it has no write-route measurement.

RSS comes from `ps` every 25 ms. Rest values were sampled after first requests reached all functions and the process had no active request. Load values are the highest sampled runner-worker RSS during the sequential requests. Kernel RSS is shown separately. These are process RSS samples, not memory charged to the whole installation.

## HTTP results

Times are milliseconds, shown as p50/p95. Cold values list first-request latency by function. Worker RSS is KiB at rest and peak under load.

| App and runner | Cold first requests by function | Warm read | Warm write | Worker RSS, rest/peak | Native cache |
| --- | --- | ---: | ---: | ---: | ---: |
| Kaminando, deno_core | profiles 127.4, dashboard 16.1, patients 26.5, files 15.2 | 21.14/23.12 | 22.59/25.27 | 32,176/33,152 KiB | none |
| Kaminando, StarlingMonkey | profiles 319.1, dashboard 21.1, patients 50.5, files 21.5 | 41.37/44.60 | 42.39/46.82 | 163,952/164,112 KiB | 197,484,992 bytes |
| Node compatibility, deno_core | zod 110.2, hono 8.5, dateFns 7.4, uuid 6.6, nanoid 5.1, jose 14.4, bufferEvents 6.5, stream 16.0, createHash 11.8, dependencyMode 5.1, web 12.9, modules 6.9, bareBuiltin 5.1, npmProcess 5.1, commonJsEvents 4.9, dynamicBuiltin 5.1, reexportBuiltin 5.2, typeOnlyBuiltin 4.8 | 12.57/13.96 | none | 36,592/40,704 KiB | none |
| Node compatibility, StarlingMonkey | zod 358.9, hono 16.6, dateFns 15.4, uuid 14.6, nanoid 14.2, jose 45.7, bufferEvents 14.4, stream 15.7, createHash 17.0, dependencyMode 14.0, web 19.1, modules 14.2, bareBuiltin 14.1, npmProcess 14.0, commonJsEvents 14.1, dynamicBuiltin 14.1, reexportBuiltin 13.9, typeOnlyBuiltin 14.0 | 5.55/6.32 | none | 117,376/125,824 KiB | 887,830,496 bytes per lane |

The node compatibility editor lane has a separate native cache of 886,388,704 bytes. Both lanes together use 1,774,219,200 bytes. Kernel RSS at rest and peak was 56,608/51,632 KiB and 72,432/72,528 KiB for Kaminando on deno_core and StarlingMonkey. It was 49,328/46,912 KiB and 48,240/48,240 KiB for the node fixture.

The node fixture's warm read was 2.3 times faster at p50 on StarlingMonkey. Kaminando's p50 read and write were about 2 times faster on deno_core. The node fixture's cold zod route was 3.3 times faster on deno_core. Its StarlingMonkey component preparation took 145.069 seconds for the CLI lane and 141.458 seconds for the editor lane, or about 7.8-8.2 seconds per function. Kaminando preparation took 31.255 seconds across four functions. JS bundle builds took 70.8 ms for Kaminando, 229.9 ms for the node CLI lane and 218.4 ms for the editor lane. Per-function values are in the build and preparation JSON files.

## Usage traces

`konae events --json` retains the latest 100 function events in these captures. Clinic captures include the read actions after the 200-request runs. The `konae.action` span for `patients.archive` had p50/p95 of 124/159 microseconds on deno_core (80 samples) and 125/159 microseconds on StarlingMonkey (60 samples). `patients.get` measured 1,058/1,341 microseconds on deno_core (20 samples) and 940/1,203 microseconds on StarlingMonkey (40 samples). The `konae.function.worker` warm span measured 15.5/18.1 ms on deno_core and 16.0/35.1 ms on StarlingMonkey. `konae.function.prepare`, `konae.sqlite`, and `konae.transaction` spans are also present in the trace files.

The node compatibility fixture does not call a Server Action, so its traces have no `konae.action` spans. Its `konae.function.worker` warm span measured 10.3/11.4 ms on deno_core and 3.44/3.86 ms on StarlingMonkey. Captured traces are in `spikes/functions-comparison/*-events.jsonl`.

## Recommendation

Keep StarlingMonkey as the default runner, consistent with the owner's production choice. The warm node compatibility read and worker spans were faster on StarlingMonkey, and all 18 expected route bodies passed in both CLI and editor lanes. Use deno_core when an app needs lower cold latency, lower worker RSS, or less preparation and cache storage. In this run, deno_core cut Kaminando warm request latency roughly in half and used about 132 MiB less worker RSS at rest. The node compatibility fixture contains no write route, so this comparison cannot establish a runner advantage for node-fixture writes.

## Artifacts

`spikes/functions-comparison/` contains the benchmark drivers, per-function bundle and preparation timings, HTTP samples, and sanitized function trace events. `node_http_long.mjs` is the node compatibility HTTP harness with a longer startup wait for cold StarlingMonkey preparation.
