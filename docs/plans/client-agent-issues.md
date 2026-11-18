# Shared build-agent conversations: delivery issues

Status: proposed implementation plan, 2026-11-17. No migration is implemented. [Local PRD](./client-agent-conversations.md); [self-contained GitHub PRD #25](https://github.com/danibram/konae/issues/25). GitHub issue bodies do not depend on these files being published.

## Delivery order

| Order | Issue | Depends on | Phase |
| --- | --- | --- | --- |
| 1 | [#26: Spike: prove Pi Durable browser execution, storage and recovery](https://github.com/danibram/konae/issues/26) | None | MVP — first |
| 2 | [#27: Persist owner conversations and recovery records in Konae](https://github.com/danibram/konae/issues/27) | [#26](https://github.com/danibram/konae/issues/26) | MVP |
| 3 | [#28: Run the browser build agent on Pi Durable and preserve complete tool traces](https://github.com/danibram/konae/issues/28) | [#26](https://github.com/danibram/konae/issues/26), [#27](https://github.com/danibram/konae/issues/27) | MVP |
| 4 | [#29: Recover scratch copies with their model context and source revision](https://github.com/danibram/konae/issues/29) | [#27](https://github.com/danibram/konae/issues/27), [#28](https://github.com/danibram/konae/issues/28) | MVP |
| 5 | [#30: Fence device handoff and reconcile interrupted agent operations](https://github.com/danibram/konae/issues/30) | [#27](https://github.com/danibram/konae/issues/27), [#28](https://github.com/danibram/konae/issues/28), [#29](https://github.com/danibram/konae/issues/29) | MVP |
| 6 | [#31: Read shared conversations and continue deliberately on desktop and mobile](https://github.com/danibram/konae/issues/31) | [#28](https://github.com/danibram/konae/issues/28), [#29](https://github.com/danibram/konae/issues/29), [#30](https://github.com/danibram/konae/issues/30) | MVP |
| 7 | [#32: Prove conversation recovery, privacy and upgrade safety before rollout](https://github.com/danibram/konae/issues/32) | [#27](https://github.com/danibram/konae/issues/27), [#28](https://github.com/danibram/konae/issues/28), [#29](https://github.com/danibram/konae/issues/29), [#30](https://github.com/danibram/konae/issues/30), [#31](https://github.com/danibram/konae/issues/31) | MVP — release gate |
| 8 | [#33: Later: fork an earlier chat checkpoint into a new file proposal](https://github.com/danibram/konae/issues/33) | [#32](https://github.com/danibram/konae/issues/32) | Later — not required for MVP |
| 9 | [#34: Later: design authorized analysis of stored agent traces](https://github.com/danibram/konae/issues/34) | [#32](https://github.com/danibram/konae/issues/32) | Later — discovery before implementation |

Deliver #26 → #27 → #28 → #29 → #30 → #31 → #32. The backend may ship dormant; enable the new agent only after the release gate. #33 and #34 are later, independent follow-ups after MVP. The spike is a decision gate: failed browser/storage/recovery proof returns to design, not server execution.

## Existing work checked

Searched all existing open and closed issues through the GitHub connector and authenticated `gh issue list --state all` before creation. No existing issue covered shared conversation persistence or Pi Durable migration.

- [#20: server preview](https://github.com/danibram/konae/issues/20) is a separate proposal. Do not fold it into this migration or treat it as a prerequisite.
- [#18: expected-value setters](https://github.com/danibram/konae/issues/18) may help individual writes; the handoff still needs fencing and observation.
- [#9: module tools and UI](https://github.com/danibram/konae/issues/9) remains the owner of module-specific agent additions. Preserve its allowlist/confirmation work if it lands meanwhile.

## Decisions and unresolved implementation details

Browser execution and browser-local keys are fixed. Reading needs no provider key; continuing does. The backend retains owner history, complete permitted tool responses, bounded model responses and coherent scratch/context/base-revision checkpoints. The bare Kernel and app Export remain independent. One admitted executor per project; stale generations and ambiguous writes cannot silently replay. Apply, current permissions and the Gate remain required. Preview test data is recreated on recovery.

The spike resolves the storage seam, exact package versions, atomic file/result capture, provider continuation material, record limits and measured mobile latency budget. It compares a narrow adapter for Pi's own commit model with supported checkpoint transfer; neither an extra scheduler nor raw IndexedDB copying is presumed. The initial candidate is Pi Durable 1.5.2, whose API is experimental.

## Issue scopes

The following text mirrors the created issue bodies, apart from headings used to nest them in this index. GitHub is the tracker for state changes.

### #26: Spike: prove Pi Durable browser execution, storage and recovery

[Open issue](https://github.com/danibram/konae/issues/26).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP — first**.

Dependencies: none; this is the first delivery step.

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
Choose a small, tested seam for migrating the browser build agent from pi-agent-core 0.84.2 to Pi Durable. Candidate: @earendil-works/pi-durable 1.5.2 and its matching pi-ai/chord/pi-schema family. This issue is a bounded compatibility experiment, not permission to ship the migration.

#### Scope
- In an isolated experiment, bundle the actual package for the browser; exercise Anthropic, OpenAI and OpenRouter adapters, local credential injection, streaming, interruption, custom model IDs and lazy loading. Check Chromium and mobile WebKit. No Node/browser polyfills that move execution to a server.
- Prove file mutation + tool result + model context can form one coherent committed boundary, and identify the exact post-truncation output the model receives. Verify original structured details survive independently.
- Compare a narrow backend adapter for Pi's native Storage contract with IndexedDB plus a supported coherent checkpoint transfer. Do not assume a public export/import API. Run upstream registerStorageConformance on any custom backend. Do not invent a second scheduler/event log, mirror private IndexedDB tables or add a Node service.
- Test lost commit acknowledgements, read-only opens, unsafe tool interruption, session failure, task/version changes and resume activation. Identify provider continuation/signature material and how to exclude private reasoning from product traces.
- Measure commit round trips, payload sizes, first history page and handoff latency under a documented mobile network profile. Set initial record/owner quotas and measurable release budgets against existing Capsule and transfer limits.
- Record the selected version, public API imports, storage seam, protocol/fencing obligations and upgrade contract in a short decision note. Treat upstream main as moving; cite the published 1.5.2 tarball.

#### Acceptance / validation
A reproducible fixture can close and reopen with matching conversation context, scratch files and base revision; viewing starts no task; an interrupted unsafe external tool never reruns. Publish test output and measurements, including failed cases. If neither storage design is proportionate, stop implementation and propose a reduced recovery boundary for review. Browser execution/local keys are fixed constraints.

#### Not included
Production dependency changes, a shipped backend schema, server agents, server build/preview, shells, code mode or automatic publish. ADR-0007 stays accepted until implementation is reviewed.

### #27: Persist owner conversations and recovery records in Konae

[Open issue](https://github.com/danibram/konae/issues/27).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP**.

Dependencies: [#26](https://github.com/danibram/konae/issues/26).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
The owning browser can save and read shared conversation state with an acknowledged, atomic recovery boundary. The bare Kernel and exported app know nothing about it.

#### Scope
Implement the storage seam selected by the spike in a focused Konae module beside the owner's MCP. Reuse SQLite and existing owner transport/transfer infrastructure where appropriate; no generic SQL API. Reuse Pi records instead of a second execution log. Associate owner/project, envelope and Pi versions, context, task state, scratch checkpoint references, original tool payloads and bounded model payloads.

Enforce owner browser session and project ownership on every record/blob operation; ordinary delegated read credentials and app sessions cannot access transcripts. Authenticate origin/CSRF as other owner surfaces do. Persistence tools are editor infrastructure, not additions to the model allowlist.

Admit with stable request IDs, expected state and execution generation. Identical retries return the same receipt; reused IDs with different content refuse. Commit acknowledgement means durable storage; uncertain outcomes are queried by ID. Return paginated history and lazy details. Enforce measured size/storage limits without silently truncating full responses or pruning resumable state.

Provide versioned conversation download, explicit delete (including payloads/checkpoints), project deletion cleanup and tombstones/generation revocation against stale reuploads. Integrate installation copy/restore; restored leases and confirmations are invalid. Keep the normal app Export unchanged. Retention defaults to explicit deletion; document copies' existing retention.

#### Acceptance / validation
Run selected storage conformance and transaction crash/failure tests. Two owners with guessed IDs cannot read each other's list, details, checkpoint, blobs or download. Repeated submissions and lost acknowledgements cannot duplicate state. Oversized writes name actual limits. Delete/read-back from a second client is empty and stale writers cannot resurrect records. Copy/restore preserves records without resuming work; an app export contains none.

#### Not included
Running Pi on the server, operator analysis, delegated history grants, full-text search infrastructure or a second event bus. This backend slice may ship dormant before the editor migration.

### #28: Run the browser build agent on Pi Durable and preserve complete tool traces

[Open issue](https://github.com/danibram/konae/issues/28).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP**.

Dependencies: [#26](https://github.com/danibram/konae/issues/26), [#27](https://github.com/danibram/konae/issues/27).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
Replace the current in-memory Agent behind the editor's KonaeAgent boundary with the selected Pi Durable version, preserving the existing file tools and operational policy.

#### Scope
Pin validated package versions, isolate upstream types behind the adapter and update PI-API.md for the selected version. Keep lazy loading and Anthropic/OpenAI/OpenRouter requests in the browser. Read keys locally at dispatch; persist no keys, auth headers, cookies or owner tokens. Keep local-only sessions available without an owner credential.

Persist stable conversation/execution/turn/attempt/task/call links, provider/model settings, prompt and tool versions, partial responses, timing, available usage, errors and refusal structures. Capture model arguments and effective pinned arguments; preserve original permitted structured responses separately from the exact bounded content delivered to the model, including truncation and redaction metadata. Capture at the result boundary, not from UI chips. Render refusal words unchanged.

Optional declared objective has origin, declaration time and source entry/call link; absent stays absent. Do not infer intent or collect private reasoning. Preserve context transformations and compaction provenance needed for coherent continuation. Honor spike findings on opaque provider material.

Preserve allowlist, project pinning, no shell/code mode, confirmation rules and publish-as-Deploy-dialog. External writes/publish are replay-unsafe. No automatic scheduler start on history open. Activation remains gated until checkpoint and coordination dependencies of the umbrella are complete.

#### Acceptance / validation
Use deterministic provider fixtures and a documented provider smoke run: streaming, abort, tool validation/refusal, 20,001+ character output, structured response, repeated tool names with distinct calls, error serialization and compaction. Verify the actual next model request receives the stored bounded content. Inspect backend request/log/export fixtures for injected credentials. Local-only first-run still produces a preview.

#### Not included
Changing Gate/Kernel rules, adding operational tools, broadening access or server execution. A successful harness unit test alone is insufficient to enable rollout.

### #29: Recover scratch copies with their model context and source revision

[Open issue](https://github.com/danibram/konae/issues/29).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP**.

Dependencies: [#27](https://github.com/danibram/konae/issues/27), [#28](https://github.com/danibram/konae/issues/28).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
A recovered conversation proposes the same files the model saw and cannot overwrite newer saved sources.

#### Scope
Use coherent immutable checkpoints for admitted input, file/tool boundaries, complete proposals, Apply/read-back and Discard. Store base sourcesRevision and immutable base files/digests, scratch files/encodings/assets, model-view files, context boundary, proposal state and settings affecting validation. Retain referenced asset bytes; do not refer only to mutable project paths.

Make each scratch mutation and its tool result atomic or provably idempotent. Apply and Discard must update model context as well as UI. Distinguish accepted-but-unsaved editor files from backend sources. Reconcile interrupted Apply/autosave through project source read-back before saying applied/saved.

Recovery checks current source revision. Preserve a stale proposal with a diff and offer a new linked attempt from current sources; no automatic merge. Rebuild browser preview, state that test rows/files/actor were reset, and rerun the Gate against current facts. Historical validation never enables Apply; build and ready preview remain required. Concurrent manual source edits retain the existing source revision checks.

#### Acceptance / validation
Kill before and after each file/result checkpoint and verify matching files/context, including binary asset references. Race hand edit/import/source save with recovery and Apply; no stale write lands and the old proposal stays inspectable. Recover Apply after source save succeeded but acknowledgement was lost. Continue after Discard and confirm the model knows those files were rejected. Shared Gate/Kernel refusal fixtures keep the same words and PREVIEW_BEHIND behavior.

#### Not included
Arbitrary chat rewind, automatic merging, carrying preview databases between devices, undoing published rows or external actions, removing Apply or automatic publish.

### #30: Fence device handoff and reconcile interrupted agent operations

[Open issue](https://github.com/danibram/konae/issues/30).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP**.

Dependencies: [#27](https://github.com/danibram/konae/issues/27), [#28](https://github.com/danibram/konae/issues/28), [#29](https://github.com/danibram/konae/issues/29).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
Only one admitted browser executor per project can advance a conversation or dispatch agent effects, including when a suspended device wakes.

#### Scope
Implement a server-clock lease with monotonically increasing fencing generation and stable executor ID. Initial candidate: 60-second expiry / 15-second renewal, subject to spike measurements. A live lease is not stolen. Cooperative handoff waits for persistence and release; expiry permits only explicit Continue after reading/reconciling committed state. Local Web Locks supplement this only within one browser.

Check generation at task admission/commit and on the server when admitting agent-originated MCP writes. Track already admitted in-flight calls so a new executor cannot race them; drain or mark/reconcile unknown outcomes before conflicting effects. Use the existing tools/permissions with execution metadata, not duplicate operational tools. Stale provider responses cannot commit or dispatch tools; already sent provider requests may still incur cost.

Before continuation read session_inspect.can and refresh tools; before effects recheck session, project and confirmation binding. Old confirmations never authorize a new execution. External writes and publish are unsafe to replay. Read back writes through platform_guide rules before claiming success. A lost response is unknown, never “nothing changed.” Unresolved observations require an informed new action; generic Retry must not repeat a write.

Stop dispatch on lost lease, storage failure, sign-out or owner revocation. Viewing and reconnecting never starts tasks. An offline reader can retain a labelled local draft but cannot create an offline shared execution branch.

#### Acceptance / validation
Race two devices/tabs; delay renewal and commits; suspend/wake old writer; lose write responses; and hold a backend write across lease expiry. Exactly one generation advances, stale effects refuse, no ambiguous invitation/SQL/publish repeats, and new conflicting work waits for reconciliation. Change permissions/project while confirmation is open and verify no stale approval executes. Check cooperative handoff, cancellation and source read-back.

#### Not included
Exactly-once provider billing, automatic lease theft, background server execution or a universal transaction spanning MCP writes, mail and traces.

### #31: Read shared conversations and continue deliberately on desktop and mobile

[Open issue](https://github.com/danibram/konae/issues/31).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP**.

Dependencies: [#28](https://github.com/danibram/konae/issues/28), [#29](https://github.com/danibram/konae/issues/29), [#30](https://github.com/danibram/konae/issues/30).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
An owner opens a project on a phone, reads its conversation without a model key, and can deliberately continue after configuring a key on that phone.

#### Scope
Replace transient message/chip state with a paged persistent view. Show recent conversations, messages, linked calls, proposal/checkpoint state and expandable arguments/original response/model-visible response/errors/refusals/timing/declared intent. Show missing or redacted fields honestly. Render untrusted content inertly; load large details only on expansion.

Reading must not construct a writable recovering harness or call a provider. Display last acknowledged save, running device, interruption, unknown outcome, source conflict and preview reset. Continue obtains the lease and current state; an active writer offers a cooperative stop/handoff path, never force takeover. Missing key blocks execution only, not history.

Keep Apply/Discard and Deploy decisions recognizable. Link an observation to the call it checks. Distinguish published-state effects from file proposals. Include conversation download/delete and explicit attachment of a local-only session after sign-in; never silently upload prior local text. Clear only this owner's caches on sign-out/account switch.

#### Acceptance / validation
Desktop Chromium and mobile WebKit: desktop proposal → phone history with no key and no provider traffic → local key setup → deliberate safe handoff → revalidated proposal → Apply/read-back. Exercise active lease, stale proposal, partial response, permission loss and unknown external outcome. A 1,000-entry fixture initially loads at most 50 entries, then pages; large responses load on demand. Record latency against the spike budget and inspect keyboard/small-screen access.

#### Not included
Operator analytics, arbitrary rewind, background mobile progress promises, server preview or a redesign of the whole editor.

### #32: Prove conversation recovery, privacy and upgrade safety before rollout

[Open issue](https://github.com/danibram/konae/issues/32).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **MVP — release gate**.

Dependencies: [#27](https://github.com/danibram/konae/issues/27), [#28](https://github.com/danibram/konae/issues/28), [#29](https://github.com/danibram/konae/issues/29), [#30](https://github.com/danibram/konae/issues/30), [#31](https://github.com/danibram/konae/issues/31).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
Enable the migration only after cross-device recovery and the existing product boundaries pass observable end-to-end checks.

#### Scope
Run and automate the umbrella PRD's ten release acceptance scenarios with deterministic model fixtures and selected live provider smoke checks. Cover crash windows, lease races with in-flight writes, source conflicts, auth loss, no-key readers, full/limited tool responses, redaction, deletion/download, quotas, offline behavior and copy/restore.

Keep a fixture from the first persisted format. Test exact-version upgrades, unknown schema/registry versions and rollback: history remains readable/exportable; unsupported tasks do not run. No silent migration of old chips into complete traces. If an old active tab offers import, label missing details and require explicit attachment. Keys remain in existing owner/provider browser storage.

Roll out behind the integration gate with the backend available first. Never run old and new harnesses on the same conversation. Rollback starts a separate old-engine conversation from current sources and preserves Durable history. Document retained-copy deletion limits, preview reset, interruption costs and local-only first run. Record the eventual ADR decision only after implementation is reviewed.

#### Acceptance / validation
Attach the test matrix and results to this issue. Run appropriate TypeScript/build, Rust persistence/auth integration, shared Gate/Kernel cases and browser suites; verify docs links. Show one complete desktop-to-mobile flow and one ambiguous-write recovery. Inspect provider/backend traffic and stored/exported fixtures for credential leaks. Confirm app Export remains portable without conversations and no bare Kernel owner store was introduced.

#### Not included
Expanding migration scope to fix unrelated uncommitted work, server builds/previews, operator analytics or advanced undo. Close the umbrella only when every MVP acceptance scenario passes.

### #33: Later: fork an earlier chat checkpoint into a new file proposal

[Open issue](https://github.com/danibram/konae/issues/33).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **Later — not required for MVP**.

Dependencies: [#32](https://github.com/danibram/konae/issues/32).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
Let an owner choose an earlier coherent checkpoint and retry from those files without erasing history or suggesting that external effects were undone.

#### Scope
Design and implement explicit restore-as-new-proposal: parent checkpoint link, preserved old conversation, model context matching the selected files, diff against current saved sources, fresh preview/Gate checks and normal Apply. Redo is another deliberate branch. If source revision changed, follow the same conflict rules as recovery; do not overwrite current work silently.

List external effects after the selected point and state that their records and real outcomes remain. Restoring files does not reverse rows, invitations, emails, accounts, calls or deployments. Any real reversing operation is a separately authorized current tool call with read-back.

#### Acceptance / validation
Restore a point before a schema proposal and before a completed invitation; only candidate files change, invitation remains and the UI says so. Old history stays readable. Revalidate against current rows and stale sources; Apply cannot reuse an old Gate pass. Redo references the correct parent and sends matching model context.

#### Not included
Universal undo, automatic compensation, data backup restore from chat, or editing historical evidence. Schedule after the MVP has demonstrated reliable recovery; this issue does not block its launch.

### #34: Later: design authorized analysis of stored agent traces

[Open issue](https://github.com/danibram/konae/issues/34).

Part of [PRD #25](https://github.com/danibram/konae/issues/25). Phase: **Later — discovery before implementation**.

Dependencies: [#32](https://github.com/danibram/konae/issues/32).

The parent issue contains the full PRD; no unpublished repository file is required to understand this issue.

#### Outcome
Make collected traces useful for finding tool refusals and recovery failures without turning private owner conversations into unrestricted operator data.

#### Scope
First agree a concrete audience and question: owner inspection already ships in MVP; cross-owner operator analysis needs its own explicit authorization and retention design. Propose the smallest report from existing trace IDs, outcomes, durations, truncation markers and optional declared intent. Prefer aggregation and redacted evidence before any raw-text access. Use the backend records already collected; do not add another telemetry store by default.

Keep declared objectives tied to their original messages; absence remains absence. Do not retrospectively infer intent, collect private chain-of-thought or forward conversations to another model/service without a separate product decision. Document limits of client-reported timing and distinguish operational audit from client-supplied trace assertions.

#### Acceptance / validation
Publish an access/data-flow proposal and sample report using synthetic traces, then validate permissions with two owners and an operator role. For any later implementation, test that aggregate views cannot expose another owner's text or tool payloads, that deletions/retention apply, and that every detail link has authorized provenance.

#### Not included
Automatic raw transcript access for operators, new delegated history grants by implication, external analytics export or a prerequisite for the MVP. The discovery outcome may be that owner-only JSON download and inspection are sufficient.

## Verification

All ten created issues were read back after creation; titles, complete bodies and dependency links were checked. The umbrella checklist was then updated and read back again. Every created issue was linked to this BB thread with `focus_board_link_issue`. No code, dependencies, lockfiles or accepted ADRs were changed; no commit, push or pull request was made.
