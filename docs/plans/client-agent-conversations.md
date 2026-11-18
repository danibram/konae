# Shared build-agent conversations, with execution in the browser

Status: proposed PRD, 2026-11-17. Design and issue planning only; no migration has been implemented. The delivery index is [client-agent-issues.md](./client-agent-issues.md). ADR-0006 and ADR-0007 remain the accepted description of the running product. Implementation must record the eventual harness decision without rewriting history as though it already shipped.

## Problem and outcome

An owner describes an app on a laptop, sees a proposed change, and opens the same project on a phone. Today the installation has the saved files, but the conversation, tool details and scratch copy lived in the laptop's agent. A transcript alone would preserve the words while losing the files those words refer to.

The owner should read the conversation on either device, understand what each check or tool answered, and continue from a coherent saved state. The build agent, provider requests, build and preview continue to run in the active browser. Anthropic, OpenAI and OpenRouter keys stay in that browser. Konae stores owner conversations and recovery state; it never uses the owner's provider key to run an agent.

This supports jobs 1 and 2: describing and hand-editing the same app. Job 3 remains the boundary: recovery never turns an old validation result into permission to Apply. Jobs 4 and 5 remain unchanged: the exported app carries its accounts, data and rules, without the owner's private conversation. Conversation persistence is a Konae facility, not a new app capability or a Kernel resource.

## Goals and exclusions

The first release must preserve a useful conversation, a complete structured tool record, the model-visible context and the corresponding scratch copy. It must support reading without a provider key, deliberate continuation on another device, source-conflict detection, clear interruption and one admitted executor per project.

It does not move execution, bundling or preview to the server; remove Apply; publish automatically; add a shell, code-mode execution, Node worker, server agent or background mobile service; share conversations between owners; or introduce operator analytics. The existing [server preview proposal, #20](https://github.com/danibram/konae/issues/20), is independent and not a dependency. The [expected-value setters, #18](https://github.com/danibram/konae/issues/18), may strengthen individual operations but do not replace this plan's execution fencing or read-back rules.

## What exists, and what Pi Durable supplies

`src/agent/harness.ts` runs `pi-agent-core` 0.84.2 with an in-memory scratch copy. `contracts.ts` exposes a small editor interface, though some types come from that package. `platform-tools.ts` pins the project, uses an allowlist and confirmation dialogs, and opens the Deploy dialog for publish. It limits model text to 20,000 characters and retains the original result in `details` only in memory. `useAgentThread.ts` and `agent-thread-state.ts` retain transient messages and chips; each new prompt clears the chips. `App.tsx` creates, retargets and disposes the agent. Provider authentication is inserted at request time by `providers.ts`, using owner/provider keys in `key-store.ts`.

Konae already authenticates owners, checks project ownership, stores installation records in SQLite and exposes the owner's MCP. None of that makes the Kernel the owner of conversations.

The candidate is **`@earendil-works/pi-durable` 1.5.2**, with the matching `pi-ai`, `chord` and `pi-schema` family. This is a reviewed candidate, not an approved dependency update. Its published README calls the API experimental. The package exports a platform-neutral root and an IndexedDB adapter; its Node adapters are separate. Published types include `Harness.open`, read-only sessions, transactional documents, task records, `ToolRegistration.replay`, `ToolExecutionResult.output`, `structuredOutput`, `details` and diagnostics. The default tool replay policy is unsafe. An interrupted unsafe tool gets an interruption result rather than automatic execution; a safe tool can rerun. Opening/resuming work can restart pending tasks, so opening history must never accidentally activate the scheduler.

Pi persistence is not cross-device coordination. Upstream requires one writer per storage and supplies no cross-process lock. Its custom `Storage.commit` contract includes ordered atomic batches, durable receipts and unknown-outcome failures, with a conformance suite. A storage error fails and closes the harness. Native recovery may retry an interrupted provider request; that can cost another request and is not an exactly-once guarantee.

Primary sources inspected on 2026-11-17:

- [Upstream Durable package and README](https://github.com/earendil-works/pi/tree/main/packages/durable).
- [Published 1.5.2 metadata](https://registry.npmjs.org/@earendil-works%2fpi-durable/1.5.2) and [immutable package tarball](https://registry.npmjs.org/@earendil-works/pi-durable/-/pi-durable-1.5.2.tgz): `README.md`, `dist/index.js`, `dist/index.d.ts`, `dist/types.d.ts`, `dist/harness/types.d.ts` and storage declarations. The tarball SHA-1 is `ba2029d62e2a4a31cf30a0a9e45b6fb35e58b099`; use npm integrity verification when installing.
- Local contracts: [ADR-0006](../adr/0006-dist-is-the-publishing-contract-and-the-editor-builds-in-the-browser.md), [ADR-0007](../adr/0007-the-build-agent-edits-a-scratch-copy-and-operates-through-the-owners-mcp.md), [architecture](../architecture.md) and [language](../../CONTEXT.md).

Reading exports is not a browser compatibility test. The spike must demonstrate actual bundling, provider adapters, recovery and storage behavior before choosing the implementation. `src/agent/PI-API.md` describes 0.84.2 and cannot serve as the Durable reference.

## Owner flows

1. **Start on desktop.** Open a project, configure a local model key if needed, and send a request. Persist its admission before starting the provider request. The conversation shows streaming text, tool calls and the candidate preview. Each acknowledged boundary says it is saved; streaming fragments may be shown as still saving. Apply retains its existing Gate, build and preview requirements. Its result is not called saved until the source revision is read back.
2. **Read on mobile.** Sign in as the same owner and open the project. Load recent conversation entries first, with older entries and large tool details on demand. No provider key is needed, and no harness task starts. A tool expands to arguments, original structured response, model-visible response, timing, refusal and observation. Show the device currently running and the last confirmed save.
3. **Continue on mobile.** Configure a key locally; the laptop's key is neither retrieved nor copied. Choose Continue. If the laptop still owns execution, ask it to stop/release through the coordination state; do not seize its live lease. Once released, or expired and reconciled, claim a new execution generation, check current permissions and sources, restore a coherent checkpoint, revalidate and start only after explicit continuation. A key need not be configured to inspect a recovered proposal; a new provider request does.
4. **Source conflict.** A hand edit, import, release or another tab changed `sourcesRevision`. Preserve the old proposal and history. Show the changed revision and a diff. The MVP offers starting a new attempt from current sources, linked to the old request; it does not silently merge or apply the stale candidate. Manual reuse of a diff is an explicit new edit.
5. **Interrupt.** Stop requests cancellation and persists the outcome. Closing or suspending the browser prevents further browser execution; an already dispatched provider request or MCP write may still finish remotely. Show interrupted or outcome unknown until observed. Reopening history never resumes automatically. A partial answer is visibly partial.
6. **No owner session.** Preserve today's local first-run path: a browser may describe and preview an app with its local provider key, without backend history. Label the conversation local-only. Signing in can offer explicit attachment of a coherent checkpoint to the owner's project; never silently upload earlier local text. Cross-device continuation requires an owner session and saved project.

## Persistent model and invariants

Names below describe logical records, not a second workflow engine or a prescribed SQL layout. Reuse Pi's conversation, entry, document and task records where possible. Add only Konae's project binding, coordination, source checkpoint and tool evidence.

| Record | Required information |
| --- | --- |
| Conversation | Stable ID, owner and project binding, created/updated times, title, format and harness versions, status, latest acknowledged checkpoint, optional parent conversation/checkpoint |
| Execution | Stable ID, conversation, initiating message/request ID, executor ID, fencing generation, provider/model/settings without credentials, prompt/tool-registry version or digest, start/end and outcome |
| Model turn / attempt | Execution and parent IDs, ordered input references and context version, submitted user message, generated message, attempt number, provider request ID when supplied, timing and available usage; partial/failed/superseded state |
| Tool call | Execution, turn, task and provider call IDs; tool/schema version; model arguments and effective arguments including the pinned project; confirmation state; start/end; original response, model-visible response, diagnostics, refusal and read-back links |
| Declared intent | Optional brief objective, origin `owner-message` or `assistant-declaration`, exact source entry and optional execution/call link, declared time. No source means absent. Pi's persisted tool-call intent means an execution record; it is not proof of a human-readable objective |
| Checkpoint | Immutable ID; conversation/context boundary; source revision and base files/digests; scratch files with encodings and asset references; proposal status; model-view files; local accepted edits if not yet saved; settings affecting validation; historical Gate result; lifecycle event and parent checkpoint |
| Observation | Call being checked, read tool and arguments, response/refusal, time, and resolved or unresolved outcome. Observing that a setting now has a value does not prove which caller changed it |

The continuation boundary atomically associates model context, scratch copy and base revision. File tools must commit their mutation and corresponding result coherently, so replay cannot apply an edit twice or present a result against different files. Binary assets may be referenced by immutable, retained content; a path to mutable current project files is insufficient.

Preserve the actual model-visible bounded tool content, separately from the original response. Record truncation strategy, original length, retained length and redactions. Never reconstruct the original from clipped text. Do not invent start times, provider IDs, usage or intent; use absent/unknown. Server receipt time determines order across devices; client elapsed timing is descriptive, not a trusted clock. Keep original refusals (`code`, message, detail and hint) and distinguish denied permission, owner decline, validation refusal, transport failure and unknown outcome.

Persist the model-visible conversation and context transformations, including compaction provenance and summaries, so continuation does not secretly substitute the UI transcript for the model context. Do not request, infer or expose private chain-of-thought. If a provider requires opaque continuation/signature material, the spike must document its minimal representation and whether that provider can resume without it; it is not an intent field or an analysis UI feature.

A recovered Gate result is historical evidence only. On recovery, rebuild the preview and revalidate current files, source revision, permissions and release facts. The preview's rows, uploaded test files, actor sessions, iframe and compiled artifact are not portable checkpoints in the MVP: recreate browser preview state and explicitly label the reset. Never claim that restoring files undid a published data change.

## Persistence with the smallest adequate mechanism

The backend is authoritative for acknowledged shared state. Keep owner records in Konae's installation storage, outside app folders and outside the bare Kernel. Prefer a focused Konae module beside `mcp.rs`, with SQLite transactions and the existing owner transport. Do not scatter a new store through the large MCP dispatcher or add a Node service.

The spike compares two concrete options:

| Option | Benefit | Cost / required proof |
| --- | --- | --- |
| Pi's commit model through a narrow backend storage adapter | One durable boundary for tasks, context and scratch documents; safe handoff can read the same committed state | Rust must honor Pi's storage contract; network latency and experimental schema are real costs. Run upstream storage conformance, failure injection and a mobile-latency measurement |
| IndexedDB plus explicit coherent checkpoint upload | Uses upstream browser storage; can keep streaming local | No assumed public export/import API. Prove complete task/document/entry reconstruction and atomic snapshots, and acknowledge the unsynced window. Two independent authoritative logs or copying raw IndexedDB tables is not acceptable |

Prefer the first only if a narrow adapter passes these checks without reproducing Pi's scheduler. It is justified by atomic cross-device recovery, not by a desire for a proprietary event log. Do not decide new tool names, wire format or database file in this PRD. Record the chosen seam, measured costs and upgrade contract at the spike exit. If neither is small and correct, return a reduced boundary-recovery design for review before implementation; do not quietly ship chat-only recovery.

Regardless of transport: admit submissions and writes with unique request IDs; compare expected state/fencing generation; acknowledge only after storage commits; read back an uncertain write by its ID; deduplicate identical retries and reject reused IDs with different content. Page history and details. Reuse transfer infrastructure for bounded large records if needed, without exposing generic SQL or a model-callable history upload tool. Polling while a viewer is visible is sufficient initially; WebSockets, a separate search index and a second event bus are unnecessary.

## One executor and external effects

Use one admitted agent executor per project, even if the project has several past conversations. That matches one scratch proposal and avoids two conversations racing to Apply. A lease has a server-clock expiry and monotonically increasing generation. A starting point is a 60-second lease renewed every 15 seconds, tuned in the spike for mobile suspension; the protocol cannot depend on accurate client clocks. Same-browser Web Locks may avoid competing local adapters, but cannot coordinate devices.

Every task dispatch, continuation commit and agent-originated MCP effect must be tied to the current generation. The backend, not merely the UI, refuses a stale generation for those effects. Claiming an expired lease requires an explicit Continue action and reading the last committed state. A live lease is never stolen. A cooperative stop closes the old harness and releases only after terminal results are persisted or recorded as unknown. Deletion revokes its generation too.

An old browser can wake after expiry and an old direct provider request can finish after handoff. It must stop on a lost lease, publish no accepted continuation, and dispatch no tools. A provider outside Konae cannot be fenced remotely; stale responses are discarded, and overlapping billing for an already dispatched request remains possible. This does not permit two admitted executors or a second application of a tool effect.

Fencing must cover admission of agent-originated MCP writes and their in-flight state. A new executor cannot race a previously admitted write that is still running. Persist call admission before dispatch, track completion/unknown state, and drain or reconcile it before admitting conflicting effects. Keep the existing MCP tool implementations and permissions; add execution metadata around their invocation, not a second set of operational tools. A backend receipt can reduce ambiguity but cannot make mail or external services transactional with the trace.

On each resumed execution, read `session_inspect.can` and discover current tools. Before an external effect, check the current session, pinned project and any confirmation against the actual call arguments and generation. Saved permission snapshots and a confirmation from another execution are evidence, never authority. Publish still opens the Deploy dialog and requires its current decisions. Capability loss stops the affected work without hiding history.

Classify reads and file mutations individually. A repeatable read can be retried with fresh permissions and recorded as another attempt. A scratch edit is safe only with an atomic file/result commit or a checked idempotency key; textual edits are not inherently replay-safe. All external writes and publish default to unsafe replay. After a write, follow `platform_guide` topic `rules` to observe it through its existing read-back tool before claiming success. A lost response means outcome unknown, not “nothing changed.” Never automatically repeat an invitation, SQL mutation, publish or other ambiguous write. If observation cannot establish an outcome, retain the unresolved status and require an informed new action; a generic Retry button must not resend it.

## Failure behavior

| Condition | Observable behavior |
| --- | --- |
| Offline or storage write fails | Stop starting shared work and external effects; show last server-acknowledged boundary and any unsaved local fragment. Keep a recoverable local draft where possible. No disconnected shared-agent branch in the MVP |
| Commit response is lost | Read by submission/commit ID before retry; no duplicate message, checkpoint or tool admission |
| Browser dies during generation | Preserve acknowledged partial state, label interrupted, and require Continue. A new provider attempt is visible and may incur cost |
| Browser dies during a write | Show unknown until receipt/read-back resolves it; never infer failure or replay from a missing response |
| Two tabs/devices press Send | Exactly one admission wins; the other remains a reader and retains its unsent draft |
| Source revision changes | Preserve proposal as stale; no Apply or continuation on a silently replaced base |
| Old harness/schema/registry version | History remains readable; resume only through a tested migration. Otherwise offer a new attempt from current sources with a link to the old history |
| Missing key, denied browser storage or expired session | Reading shared history needs only an active owner session. Continuation explains the precise missing prerequisite; no credentials copied from another device |
| Quota or oversized response | Refuse before dispatch when predictable; otherwise preserve explicit capture failure/length and pause. Never label a clipped or missing response complete |

## Privacy, access and lifecycle

MVP history endpoints require the owning browser session and current ownership of the project. App sessions, another owner and ordinary delegated `read` credentials do not gain transcript access: tool responses may contain rows or earlier privileged results. Delegated history access is future work with an explicit capability; it must not piggyback on existing `read`. Operational MCP calls continue to use their existing scopes and current `can` map. All server operations enforce ownership independently of supplied IDs and inherit the owner origin/CSRF checks.

Provider keys, owner tokens, cookies and authorization headers never enter conversation records, logs, exports or backups. Extract only allowed fields from provider/tool objects; scrub known credential material and sensitive URL parameters before persistence, and mark redactions. This applies to error strings as well as success responses. Free text can still contain a secret pasted by a person; warn against entering credentials in chat, support deletion, and do not claim arbitrary secret detection. Show that tool records may contain app data. Trace UI renders untrusted text as inert text, including arguments and tool responses.

The default is to retain conversations until the owner deletes them or deletes the project. Archive only removes a conversation from the main list. Set explicit bounded record and owner storage quotas, reported with actual limits; the spike must choose initial values against existing Capsule/transfer limits and representative tool responses. No silent pruning of resumable context, proposals or original responses. When full, pause new writes and offer export/delete; cleanup must preserve referenced checkpoints and blobs.

Provide an owner download of one conversation as versioned JSON plus referenced files, including timeline, trace, redaction markers and checkpoint metadata. This is a separate **conversation download**, not the app's Export and not a promise to import runnable Pi tasks on another installation. Project/app exports stay unchanged. A Konae installation copy includes the new store and blobs; restore invalidates execution leases and pending confirmations and never starts tasks. Test this integration whichever store location wins.

Deleting a conversation removes its live records, payloads and checkpoints after revoking execution; no stale client may resurrect it. A second-device read confirms deletion. Project deletion cascades; owner deletion/suspension follows existing account policy, immediately revoking access. Clear owner-specific local caches on sign-out/account switch without clearing unrelated browser storage. Existing installation copies can retain deleted records until their normal expiry; document that limit rather than promising retroactive erasure from backups. Conversation deletion does not delete the independent operational audit log or undo app data.

## Checkpoints, restore and redo

MVP checkpoints exist for recovery: admitted input, each coherent file/tool boundary, completed proposal, Apply/read-back and Discard. Apply and Discard update model context as well as UI state; the next model request must know whether its proposed files were accepted, rejected or replaced. Unsaved accepted editor changes are distinguishable from the installation's source revision. An interrupted Apply is reconciled through source read-back before it is reported as accepted or offered again.

The MVP can recover the latest coherent candidate, revalidate it, and start a new attempt after a conflict. It does not provide arbitrary timeline rewind. A later phase may let an owner select an earlier checkpoint and fork a new conversation/context boundary with those files as a proposal against the current saved revision. Rebuild preview, show the diff, revalidate and require Apply. Redo is another deliberate branch; later messages and external tool records remain visible in their original history.

Neither phase reverses published rows, invitations, mail, external calls, accounts, signing secrets or deployments. Any real reverse operation is a separate current operation with its own authorization, confirmation and observation. Restoring preview files is never described as “undo everything since this message.”

## Compatibility and delivery

Ship the backend contract first without changing the default agent. Introduce a versioned adapter behind the existing `KonaeAgent` boundary; remove dependencies on legacy Pi types where they leak through `contracts.ts`. Gate activation until the persistence, recovery and coordination slices work together. Keep the old path available during staged verification, but never run two harnesses over one conversation or silently downgrade persisted Durable state into the old agent.

There is no historical store to backfill. Old tabs can offer an explicit snapshot of the messages and proposal still in memory, marked incomplete and without fabricated tool details or intent. Do not reconstruct missing original responses from chips. Existing keys stay in their owner/provider namespace; do not migrate them to the backend. New local-only sessions continue to work. Unknown record versions must remain inspectable/exportable and refuse execution with an actionable explanation.

Pin the selected Pi versions exactly after the spike. Maintain a fixture from the first shipped record format, test upgrades, and distinguish Konae's envelope version from Pi's schema/registry versions. On rollout rollback, preserve new records for reading/download; the old engine can start a separate conversation from current saved sources, not continue an incompatible Durable task. Update ADR-0007 only when the final implementation and decision are reviewed.

## Release acceptance

1. On a desktop, ask for a source edit and a tool read with a response over 20,000 characters. A phone with no provider key reads the request, full permitted original response, exact model-visible response, timings, errors and linked proposal. No provider request occurs just by viewing.
2. After configuring a separate key on the phone, deliberately continue from the same acknowledged boundary. The model sees the matching scratch files and context. No backend request/log/export contains either key.
3. Kill the writer before and after a file/result commit, during streaming, during MCP dispatch, after a write but before its response, and during Apply/autosave. Reopen without automatic execution. Every recovered boundary is coherent; ambiguous effects stay unknown until observed and are never automatically repeated.
4. Race two devices, expire a lease, wake the old tab, and delay an admitted write until after expiry. One generation can commit; stale calls are refused; conflicting effects wait for reconciliation. Repeated submission IDs never duplicate admission.
5. Change sources by hand or import between proposal and recovery. The stale proposal remains readable; Apply is blocked. Rebuilt preview states its test-data reset. Gate/Kernel fixture refusals retain their code and words.
6. Decline a write, revoke a permission, switch project and sign out while confirmations are open. Nothing executes from stale approval. A successful permitted write is observed through its documented read-back before the UI claims success.
7. Another owner, app session and delegated ordinary `read` token cannot read history, details, blobs or downloads. Crafted tool HTML remains inert. Credentials in injected error fixtures are redacted before persistence.
8. Download and delete a conversation, then read from a second device. The download is complete within documented redactions, deletion cannot be resurrected, and app exports contain no owner conversation. Restore an installation copy: records survive, active execution does not.
9. Desktop Chromium and mobile WebKit complete the read/continue/interrupt flows. With 1,000 entries, load only the latest page (initially 50) and no large payload until expanded; measure first readable content and continuation latency under a documented mobile network profile. Establish the release budget in the spike rather than inventing a benchmark result.
10. A fresh local session still reaches a running preview without an owner credential; build, Apply and Deploy retain their existing boundaries. Do not enable the migration until all preceding release checks pass.

## Decisions to settle in the spike

The product boundary is settled: browser execution and keys, backend owner history, coherent proposals, explicit continuation, no ambiguous write replay. The implementation uncertainties are browser/provider compatibility across 0.84.2 → 1.5.2; the smallest conforming storage seam; atomic file/result capture; storing the exact post-limit model output; required provider continuation material; size/latency budgets; and migration of versioned tasks. A failed spike is a design finding, not permission to move execution or keys to the server.
