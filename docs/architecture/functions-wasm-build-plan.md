# Split the StarlingMonkey build step

An app leaves with its published ESM, hashes, accounts, data and rules. The receiving Kernel builds a snapshot for each function before it serves, including when the saved draft is ahead of the release. [ADR 0020](../adr/0020-wasm-functions-runner.md) and the [production design](functions-wasm-production.md) set the folder and build policy. This plan uses their destination-built snapshots and eleven refusals where the earlier runtime contract proposed shipping Wasm and one initialization code. A pending promise with no work can remain if module evaluation and its initialization jobs completed. Use a fixed initialization clock on both installations. Invocation-time Web API parity and performance acceptance are separate work.

## Branches and file ownership

Branch all three slices from the commit that adds this plan on local `functions/wasm-build-plan`. Its parent is `origin/functions/wasm-build-step` at `a2a18d0b`. The commit supplies two compile-time boundaries. The component verifier stub fails closed. The candidate check stub calls A4's existing preparation so B2b can exercise the owner path before B2c replaces the backend.

| Slice | Owns | Test input available at the base |
| --- | --- | --- |
| `functions/wasm-prepare-helper` (B2a) | New standalone `kernel-rs/functions-prepare/`, including its lockfile, build-only adapter, pinned asset recipe and tests. Reads the existing guest WIT and runtime adapter. | The A1 archive has splicing and assembly probes. Build against this branch's WIT and call the snapshot through the current StarlingMonkey host. The archive's IPC/2 component omits the second argument to `op-worker-failure`. Pin the current `web-streams-polyfill` guest dependency in the adapter build. |
| `functions/wasm-shared-refusals` (B2b) | `kernel-rs/kernel/{Cargo.toml,src/refusal-codes.json,src/functions-artifact-cases.json,src/functions-wasm-cases.json,src/functions/{manifest_artifact,wasm_validation}.rs}`, `kernel-rs/Cargo.lock`, `kernel-rs/konae/src/{mcp/mod.rs,mcp/function_candidate.rs,guide.rs}`. `src/kernel/functions.ts` and its artifact tests, `src/agent/{gate,contracts,tools,harness,durable-engine}.ts` and their tests, `src/App.tsx`, `src/mcp/function-candidate.ts`, `tests/mcp.integration.test.ts`. | Feed the real Gate a fake MCP response from the shared cases and test the owner handler with an injected preparation function. Generate components from WIT locally for the Kernel verifier. Run the existing runner to prove the positive route on this branch. |
| `functions/wasm-cache-and-proof` (B2c) | `kernel-rs/kernel/src/{functions/{mod.rs,preparation.rs,preparation/,runtime.rs,http.rs,http_tests.rs},deployments/release.rs,app_folders/{folder,import,package}.rs,app_data/{admin,backups}.rs,server/{startup,operator}.rs}`, `kernel-rs/konae/src/projects/release.rs`, `kernel-rs/functions-{executor,runner,runner-wasm}/` except `functions-runner-wasm/guest/`, `scripts/konae{,.test}.ts`, `docs/operate.md` and new proof files under `tests/functions-wasm/`. | Use a test-only helper executable that emits the agreed manifest for a real IPC/3 portable component. Generate it with `measure/starling-component.sh` using the tools already installed. In a scratch copy of `starling-component.mjs`, set esbuild's `mainFields: ['module', 'main']` for `web-streams-polyfill`. Inject a test verifier that accepts the fixture's exact input and output digests and refuses all other bytes. Exercise the real compiler, cache, IPC and HTTP in a Kernel test. |

The helper owns a separate build-only entry and can import the published ESM as one module under a fixed filename. It reads the unchanged invocation adapter and WIT from `functions-runner-wasm/guest/`. The cache owns the runtime entry points and worker transport. Neither task needs to edit the same guest file. B2b owns root workspace dependency changes. B2c can use the executor, libc, SHA-256 and Tokio already in the Kernel. B2b adds the MCP dispatch while B2c owns release and backup paths, including the preflight before `app_data/admin.rs` takes its project, publish and database locks. Current cache pins reconcile against the served catalogue, so project deletion needs no cache call in MCP.

## B2a to B2c through the helper process

The Kernel selects an absolute operator-installed `KONAE_FUNCTIONS_PREPARER` path. On strict Linux B2c launches it outside the Kernel identity, with a cleared environment, a read-only staged source, and bounded scratch and output mounts. The helper owns the Wizer subprocess and its deterministic WASI inputs. Only generated initialization variables reach that subprocess. Expose no network, secrets, app data, or cache to either build process. Strict setup failures stop preparation. On macOS use explicit loopback development mode. The parent uses a bounded queue and a 90 second per-bundle outer deadline covering helper and native compilation. On expiry it kills and reaps the preparation group. The helper uses a shorter internal deadline and reaps Wizer. Keep Wizer in the outer preparation process group. Use hard limits for file size and child memory and bound scratch by total bytes. Test the limits with actual process exits and scratch cleanup.

```text
konae-functions-prepare identity
  -> {"abi":"konae-functions-wasm/1","engineSha256":"<hex>","builderFingerprint":"<hex>"}
konae-functions-prepare prepare --input <absolute app.mjs> --output <absolute component.wasm>
  -> {"status":"ok","manifest":{"abi":"konae-functions-wasm/1","inputSha256":"<hex>","componentSha256":"<hex>"}}
  -> {"status":"<PrepareStatus>","diagnostic":"<optional private detail>"}
```

Each command prints one JSON object to stdout and exits zero for a well-formed reply, including typed refusals. Any other exit or malformed reply is an operator failure, except a limit enforced by the supervisor. Allow at most 8 KiB of stdout, 4 KiB of private diagnostics and 64 MiB of portable output. Drain both pipes under those bounds so a verbose child cannot deadlock the parent. Hash the raw UTF-8 input and exact output bytes with SHA-256 in lowercase hex. The builder fingerprint changes with the engine, splicer, Wizer, WIT, adapter, build code or initialization policy. The helper uses ComponentizeJS 0.21.0's splicer and pinned engine, Wizer 10.0.0 and the encoder pins in the A1 probe. It disables semver import merging and validates encoded bytes. Node remains a way to generate a baseline fixture. The shipped build uses Rust.

The status values and Rust member names are in `kernel-rs/kernel/src/functions/wasm_validation.rs`. Observe the trusted completion marker, settled initialization jobs and callable `handle` before reporting success. Record timer and fetch attempts even when code catches the exception. Use structured engine signals for unsupported APIs, throws and build limits. Leave no usable output after a refusal. Keep the completion check out of the production WIT exports.

## B2b to B2c inside the Kernel

```rust
pub async fn check_preparation(state: &AppState, artifact: &ServerArtifact) -> anyhow::Result<()>;
pub(crate) fn helper_refusal(status: PrepareStatus, function: &str) -> anyhow::Error;
pub(crate) fn verify_component(
    manifest: &WasmManifest,
    input_sha256: &str,
    component: &[u8],
) -> anyhow::Result<()>;
```

B2c implements `check_preparation` by taking a temporary pin from the same preparation path as publish and dropping it on return. Deno and the disabled runner follow their existing release policy. In the Wasm path B2c parses the helper reply, calls `helper_refusal` on a typed refusal, and calls `verify_component` on the bounded bytes before compiling them. B2b returns a downcastable `Refusal` from both functions for every named failure. Map `bundle_contract` to existing `function.bundle-contract` and pass its declared name. Use the code and exact words from section 3 of the production design. Check ABI, input and component digests, encoding, typed imports, then typed exports. A wrong helper input digest also uses `function.wasm-digest`; it breaks the association between source and output. Treat an absent ABI as an empty ABI and let the ABI check refuse it. Once the ABI matches, a missing or malformed digest refuses at the digest step. Validate the exact `guest/starling.wit` interfaces, including `option<string>` on the failure import. Use operator remedies for fresh component format and provenance refusals, and Capsule remedies for the bundle and initialization refusals. Add the published bundle digest case to the existing artifact cases and the ten Wasm codes to `functions-wasm-cases.json`. Startup, restore, direct release and owner MCP keep the same `Refusal` through their existing adapters. Missing tools, cache I/O, queue saturation and unrecognized helper replies use operator errors.

Offline the Gate checks declarations, normalized manifests, names, sizes, and the self-contained ESM export and import rules. Where it has a published manifest it also checks the declared bundle hashes through `verifyServerFolder`. The candidate transfer has its own digest. The browser gets component and initialization results from the owner MCP.

```ts
type CandidateFunctionCheck = (input: {
  artifact: ServerArtifact;
  scratchRevision: string;
  buildFingerprint: string;
}) => Promise<"accepted" | "pending">;

// Reject a refusal with the existing MCP error code, message and hint.
function_candidate_check({
  projectId, transferId, scratchRevision, buildFingerprint
}): { scratchRevision: string; buildFingerprint: string };
```

The browser uploads the UTF-8 JSON `ServerArtifact` and sets `buildFingerprint` to the SHA-256 of those exact bytes. Bind that build and scratch revision to the proposal in the editor and ignore answers for superseded snapshots. The acknowledged scratch revision is a client correlation value. MCP checks project write permission, ownership, scoped completed transfer and matching digest before reading bounded bytes and preparing. Consult `session_inspect.can.write` and use the existing transfer read-backs. The check changes no project, release or data. `validate_capsule` builds candidate ESM through the editor builder. Apply checks the approved build before checkpoint disposition or save. Disconnection sets `functionPreparation: "pending"` on the Gate result and leaves the proposal available with Apply blocked. Non-function proposals retain their existing flow. Test both normal and durable agent paths.

## B2c from preparation to Invoke

```rust
async fn prepare_version(state: &AppState, artifact: Option<&ServerArtifact>)
    -> anyhow::Result<PreparedVersion>;
```

`PreparedVersion` is an opaque RAII pin on the validated manifest, ordered bundle hashes and name-to-native-key map. The cache keeps a per-bundle input alias of source hash, ABI and builder fingerprint. The version map also binds the ordered names and hashes and the exact release reference. The native key is the component digest, engine digest, Wasmtime version and target compatibility fingerprint. Include CPU features and engine configuration in that fingerprint.

The operator-only runner commands are `--native-target`, which prints `{"wasmtime":"38.0.4","targetFingerprint":"<hex>"}`, and `--compile-component <absolute staging-output>`, which reads the bounded portable bytes from stdin and writes only the staged native object. Use the same Wasmtime configuration for this compiler and the worker. None of the worker's Init or Invoke code can call the build or compile path. Once verified, send that exact buffer to the compiler. Record its digest and discard the portable bytes. Share one build per key across concurrent misses, publish by sync and atomic rename, and budget actual file bytes and in-flight reservations. Evict unpinned entries oldest first. Use `KONAE_FUNCTIONS_WASM_CACHE_BYTES`, default 4 GiB, for native storage and its in-flight reservations. A bad stored entry is a miss during preparation. A runtime miss fails the invocation. Cache files live under the installation's private volume and never enter an app folder, package or backup.

Plan pins last through apply or expiry. Attach the pin to the plan's exact transfer digest, project and source revisions. Persist the expiry and reference with the cache pin so a restart can recheck it. Apply rechecks identity before switching and promotes the pin with the Deployment. Rebuild a missing or incompatible plan entry outside locks, then recheck under lock. Current Deployments retain pins with zero workers. An admitted request or a draining worker keeps its lease until it can no longer use the object. Sweep only after startup reconciles current, staged and plan references. Recheck the catalogue during later sweeps so a removed Deployment releases its current pin. A failed build or exhausted cache leaves the previous Deployment and its rows available.

Select the Wasm Linux launch policy before spawn with `KONAE_FUNCTIONS_RUNNER_KIND=wasm`. Keep the existing deno default. IPC/3 keeps the deno `Init` with ESM. Add `InitPrepared { abi: String, generation: String, limits: WorkerLimits, components: BTreeMap<String, u32> }` for Wasm. The map names read-only inherited file descriptors the parent opened and checked for ownership, regular-file type and write protection. Do not reopen a user-supplied path or share unrelated descriptors. The worker loads only the component named by Invoke. Verify ABI, generation and `Ready { abi: String, generation: String, engine: EngineKind }` before Invoke. Serialize engine kinds as `deno`, `starlingmonkey` or the test-only `quickjs`. B2c supplies the engine check and descriptor transport in explicit loopback development mode. Until the production launch policy lands, strict Wasm worker startup fails closed. Keep the existing strict deno containment.

## Keep A4's release entry points

- Keep its pre-serve boundary at planning, deploy/apply/import and standalone folder startup. Extend it to source backup restore and the catalogue loaded by both binaries on restart. A catalogue record enters the serving map only after its Wasm preparation succeeds. A data-only move needs no build.
- Keep private staging, bounded blocking execution and process cleanup. Move expensive work before project, publish and database locks. Recheck source revision, release digest or backup identity under the existing locks before the first live change. Staging and preparation must cover the same bytes that will replace `server/`.
- Replace the delegated Node `--prepare`, child environment forwarding and stderr-only errors with the Rust helper, typed refusals and version pins. Replace build-on-miss in the worker with protected cache handles. Allowlist the Wasm kind, preparer and cache options in `konae dev` and update the operator guide for manual helper setup. The deno option and exports without functions keep their existing policy.

## Merge and verify

Merge this plan first, then B2a, B2b and B2c in that order. The helper can be built independently, and the refusals and owner check can land while the A4 backend still exists. Keep fixture overrides in tests or disposable worktrees. After merging all three, run the full suites and repeat the editor publish and CLI publish with the Rust helper and verifier. Export the published folder, then import it into a separate Kernel with an empty cache and networking disabled during preparation. Sign back in using the moved app account. Record an authenticated GET, a POST with an idempotency key, and a read of the persisted row under the moved account's rules. Check that the cache is ready before the first request and that no native object traveled. Record separate stock Linux containment results if available. macOS development or Docker Desktop cannot establish the stock-kernel result.

The separate launch-policy task starts after B2c. It owns strict worker identity and resource limits, the images, entrypoints and generated Compose. It consumes the prepared-descriptor transport and engine handshake. Runtime parity can keep developing with the existing harness. Carry its guest and WIT changes into the helper and verifier at integration, with a new builder fingerprint and a component ABI change for incompatible WIT.
