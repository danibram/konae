# Pi Durable 1.5.2 compatibility and storage spike

This is a bounded experiment for Konae PRD #25 gate 1 / issue #26. It does not change Konae production source, dependencies, or configuration. No provider credential is used and no request is sent to a live model endpoint.

## Reproduce

From the repository root, with Bun 1.3.11, Node 22.23.4, and the exact Playwright 1.62.1 Chromium/WebKit browser builds available:

```sh
cd spikes/pi-durable-1.5.2
bun install --frozen-lockfile
bun run test:all
eval "$(fnm env --shell zsh)" && fnm use 22.23.4
node_modules/.bin/tsx browser/bundle.ts > results/bundle.json
node_modules/.bin/tsx browser/run.ts > results/browser-runs.json
bun run bench:raw > results/mobile-raw-replay.json
bun run bench:refs > results/mobile-details-pointers.json
```

The original test output is committed at `results/tests.txt`. The frozen manifest pins `@earendil-works/pi-durable`, `pi-ai`, `chord`, and `pi-schema` to 1.5.2 (published tarball SHA-256 `915648ed07c48ca4a2cf8bde97e9b1693a0127cb1332225b6883bf0d5f6f9082`; lockfile integrity is also pinned). Playwright is 1.62.1. The browser runner intercepts all provider HTTP requests with deterministic SSE fixtures; these results are intercepted compatibility tests, not live smoke.

## Evidence

- `tests/storage.test.ts`: 33 IndexedDbStorage conformance/fault tests pass under Vitest + fake-indexeddb.
- `tests/native-storage.bun.test.ts`: 36 pass, including 32 upstream Storage conformance cases over a SQLite-backed projection of ordered Pi StorageWrite batches; lost acknowledgement/read-back/reopen without duplicate; actual Durable Harness edit-tool result persistence; unsupported task v2 retained and blocked as `task_too_old` by v1 without execution; and storage-write failure closing the Harness before new work is admitted.
- `tests/unsafe-recovery.test.ts`: 1 process-kill test passes. It kills a JSONL-backed Durable Harness during an `replay: "unsafe"` tool after a marker and simulated external side effect are written. Reopen/resume marks the original tool result interrupted, leaves the effect count at one, and does not call the tool again. The post-reopen faux model response is plain text, so a new model-issued call cannot be mistaken for replay. This is a local file side-effect simulation, not a real external service.
- Combined output: 70 tests passed, 0 failed; Bun reports 750 assertions. The browser fixture runs the actual lazy bundle and actual `Harness` + `IndexedDbStorage` in Chromium 151.0.7922.34 desktop and WebKit 26.5 at iPhone 13 emulation 390×664. Both verify zero provider/runtime module requests before the user click, then intercepted Anthropic/OpenAI/OpenRouter streams, custom model IDs, localStorage keys, stream abort, Harness submit, and read-only reopen/history. Provider catalogs were 18/44/403 models. Credential values are never written to results; only presence/header booleans are recorded.
- The Harness tool test stores the full 20,001-character original and structured detail in `ToolResultEntry.model[0].details`, alongside a 20,000-character bounded model-visible `content` and a file delta/base revision in the same durable result batch. It verifies the next faux Models request receives exactly the persisted model message and read-only reopen sees the result without provider calls. This proves separation and atomicity in the fixture; 20,000 characters is only the tested bound, not a production quota.
- Bundle output totals 2,584,602 bytes, with a 488-byte entry. Provider/runtime code is lazy-loaded after the click. Esbuild reports one optional `node:fs` require-call in a bundled dependency; browsers do not execute it because its Bun-only branch is guarded by `process.versions?.bun`. This remains a packaging item to resolve or explicitly accept before shipping.

## Mobile-shaped replay experiment

`bench/` runs a local Bun HTTP server with deterministic 120 ms RTT, 2 Mbps upload, and 10 Mbps download, then materializes the handoff into Pi `MemoryStorage`. It uses 1,000 synthetic entries with 100 large tool records, not a browser or a persistent/server SQLite deployment. Three-run ranges from the committed results:

| Path | Commit (15,634 B) | Latest 50 (14,577 B) | Claim + replay handoff |
| --- | ---: | ---: | ---: |
| Inline original detail | 187–210 ms | 137–140 ms | 5,669,536 B; 4,871–4,922 ms |
| Details behind pointers | 192–242 ms | 139–152 ms | 2,393,825 B; 2,219–2,250 ms |

This supports pointers and bounded pages, not a measured production SLO. Proposed acceptance budgets for a follow-on implementation on this profile are: a typical 15.6 KB commit ≤300 ms; latest 50 ≤250 ms and ≤15 KB; explicit handoff ≤3 s and ≤2.5 MiB. Pointer handoff meets the latter budget in this synthetic run; inline details do not. Re-run against the actual authenticated implementation and a real mobile browser before adopting those budgets.

Quota proposals must fit Konae's existing boundaries: source folders hold up to 20 MiB decoded, serialized source uploads and ordinary owner transfers up to 32 MiB, and built artifacts up to 16 MiB decoded ([`docs/limits.md`](../../docs/limits.md), [`sources.rs`](../../kernel-rs/kernel/src/sources.rs)). Replace the earlier 128 MiB per-owner proposal with a provisional 16 MiB per-owner conversation-metadata budget and a separately bounded, content-addressed file-byte budget. Keep immutable base/scratch files out of the metadata quota and deduplicate identical bytes so two full base/scratch sets can be retained. Each source set may still be up to the existing 20 MiB decoded limit; therefore two non-deduplicating full sets can exceed the ordinary 32 MiB transfer envelope. Export must use bounded component transfers with an explicit larger component quota, or refuse with a named size limit; do not silently prune. Set a provisional 1 MiB maximum for an individual structured detail payload and refuse/pause writes with a visible reason when quota is reached. These proposals require measuring actual serialized snapshots and reviewing export component limits before implementation; they are not observed safe capacities. The fixture does not validate exported-folder portability or a complete owner backup/restore.

## Decision for issue #26

**GO to issue #27 as a prototype of the native Storage adapter, with no production activation. NO-GO to making browser IndexedDB the authoritative portable checkpoint.** The native seam has executable upstream storage conformance plus actual Harness result/recovery cases, and ordered native writes are the smallest path that can put result, files/base revision, receipt, and history under one authoritative server transaction. The existing browser `IndexedDbStorage` runs well in the fixture, but the published public API exercised here has no public whole-store snapshot/export/import checkpoint. Copying IndexedDB's private tables would couple recovery to undocumented representation and would not provide a fenced multi-device authority. This is evidence for the next prototype, not proof that either design satisfies Konae's release constraints.

### Required #27 adapter/protocol contract

1. Implement only the public Pi Durable 1.5.2 `Storage` interface. Treat each `commit` input as ordered batches of native `StorageWrite`; preserve order, sequence, accepted-prefix/refusal behavior, and exactly the `MemoryStorage.prepareCommit` semantics. Do not invent a parallel task/history schema or write browser IndexedDB internals.
2. On the authoritative backend, one transaction persists the accepted native writes, sequence numbers, full private tool/provider context, immutable file blobs or their content-addressed references, base revision, stable request ID/hash, and receipt. Enforce owner/conversation authorization and a fenced writer generation in that same transaction. A stale generation cannot append or receive an acknowledgement.
3. Resolve Pi `receive(index, seq)` only after durable commit. If the response is lost, query the durable receipt by request ID/hash, reconstruct the same immutable outcome, then acknowledge once; on an unknown receipt, fail closed. Idempotently return the original receipt for a duplicate ID with the same hash; reject ID reuse with a different hash. Storage failure closes the Harness and refuses shared work.
4. Allocate globally unique Pi IDs from an authoritative high-water mark or a fenced, bounded reservation per execution. The observed correctness rule is `max(authoritative high-water + 1, projection candidate)`, capped at `Number.MAX_SAFE_INTEGER`; burn unused reservations. Never let a stale writer mint/commit within a newer generation.
5. Keep normal history and model recovery bounded: fetch latest pages first; store full structured detail privately and expose it through explicit detail pointers; pass only a documented bounded model-visible result. Konae must never collect private thinking text into its trace, UI, export, or model-visible bounded result. The spike does not enable reasoning or collect thinking text. If a live provider proves resume requires opaque continuation material, #27 may retain only that opaque, provider/model-scoped material separately, with access controls and no trace projection; it must not decode/export it as a trace. Cross-provider/model changes must follow Pi's documented dropping of provider-specific thinking/signatures. Live continuation of opaque material remains unproven.
6. Reopen/read-only inspection must not resume execution or issue provider requests. Unknown unsafe side-effect outcomes remain interrupted/unknown and are never replayed automatically. The app must reconcile before admitting a conflicting fresh write; this fixture proves no replay of the original unsafe tool but does not establish a production conflict-admission policy.
7. Pass the pinned upstream conformance suite against the adapter and test lost acknowledgements, process death, stale fencing, duplicate/mismatched request IDs, unsupported versions, quotas, read-only/no-provider behavior, atomic files+revision, and export/import before any production decision.

### What remains unproven

The SQLite projection tests use an in-memory Bun SQLite reference, not a production persistent database, authenticated service, HTTP CAS/lease implementation, installation export/import, or multi-device race. The mobile numbers are synthetic loopback results. Browser provider exchanges are deterministic intercepted SSE. No real provider credential, live endpoint, offline-device handoff, encryption-at-rest policy, migration/rollback, or private-reasoning leakage audit was exercised. The crash test exercises one unsafe tool and one JSONL writer only. These gaps are why this is a spike and why #27 must establish the server transaction and fencing before anyone treats this path as durable production storage.
