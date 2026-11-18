# #31 owner history and local-first implementation evidence

This records the #31 slice for #32 integration. It does not activate shared execution or change the rollout gate.

## Origin/main divergence

The validated origin/main baseline for this worktree was `af1a897175ff93a7ce43cb0ffe29fd30a122f541`. Its registered `/` and `/app/<id>` routes enter `PlatformGate` and have no credential-free browser entry, despite the PRD's local-first expectation. #28 independently documented that mismatch in `src/agent/PI-API.md`. #31 adds the explicit `/local` browser-only route and a “Try in this browser” link on the existing owner sign-in form. `/` and `/app/<id>` remain behind `PlatformGate`.

The local entry uses the existing browser key store, agent, Gate, Build, Preview and local Apply seams. It does not inspect owner session, projects, sources or MCP from `/local`; a user action after local work prepares a versioned handoff in session storage. After sign-in, the authenticated workspace asks the owner to choose a saved Project and explicitly attach the transcript and current/candidate file checkpoint. The action creates a new owner conversation and checkpoint; it does not save sources, Apply or Deploy. If task state or opaque pre-attachment details cannot be preserved, the checkpoint is marked inspection-only and continuation refuses with that reason.

## Reader and transport

The authenticated timeline requests at most 50 displayable entry records, pages older records explicitly, and reads raw entries or detail blobs only after expansion. The server projection accepts both string message content and text parts, excludes internal/system and scheduler/document-churn rows, caps visible text at 1,200 characters, and leaves native batches untouched. Optional checkpoint summary failure does not hide a successfully authorized timeline. The summary uses `summary=true` for Rust's typed boolean query.

The reader and continue path use #30's project lease, generation, cooperative stop, effect receipts and documented source read-back. The initial checkpoint view is metadata only. Continue requires a provider key saved in this browser and current permission, then rechecks saved sources and the native high-water sequence. It can reuse the inspected native batch snapshot and rechecks the lightweight conversation envelope before claim, avoiding another full replay.

Versioned downloads include native records, checkpoint IDs/sequences/boundaries, blob bytes and authoritative effect receipts with intent, result, refusal and observation fields. Deletion is confirmed and verified by a fresh owner list read. Owner cache cleanup is limited to that owner's cached workspace and durable recovery/executor identities; provider key storage and unrelated browser storage are retained.

## Measurements and scope

The independent #31 1,000-entry iPhone WebKit reader probe measured a 13,044-byte first page and 232 ms from selection click to paintable content under its synthetic 120 ms plus 10 Mbps profile. It observed zero provider requests, lease claims, detail reads or blob reads on initial open; detail/blob data loaded after explicit expansion. The probe also checked keyboard activation, inert markup, download and second-session read-back after deletion. These synthetic loopback figures compare with #26's proposed ≤250 ms and ≤15 KB latest-50 targets; they are not production SLOs.

The native HTTP projection fixture on this implementation measured 50 items / 13,222 bytes in 94.89 ms locally; applying the same 120 ms and 10 Mbps synthetic profile estimated 225.47 ms. It confirmed the older page and explicit detail/blob fetch, and an independent owner session remained isolated. That server-only estimate excludes real owner login, browser rendering, and mobile-device scheduling. The independent browser measurement above was from the parent draft; #32 should rerun it against the locked commit SHA.

## Declared owner intent contract

An owner can explicitly place `Objective: ...` on the first line of a Durable prompt. The whole prompt, including that line, remains the model-visible user message subject to the existing credential redaction. The browser adds the separately redacted declaration and its client assertion time to that same native `konae.owner-message` entry; `$self` marks its source as that entry. The read-only history projection resolves `$self` to the entry's actual persisted Pi ID. It does not change raw records or create IDs. An ordinary prompt has no intent. The client clock may be wrong or changed, so `declaredAt` is descriptive evidence, not trusted server time. Only owner-authored declarations are captured; assistant private reasoning is never used as a declaration.

This focused write/read contract does not change the independent #32 rollout, rollback, offline, privacy/authentication or in-flight Deploy checks. `SHARED_DURABLE_ACTIVATION` remains false until the integrated gate passes.

## Remaining #32 gate

- Run the complete integrated gate on the exact landed SHA across independent Chromium owner sessions and iPhone WebKit, including delayed A/B selection, stop/release, permissions loss, stale source diffs, partial response, unknown write outcome and source read-back. Do not infer rollout readiness from these isolated fixture measurements.
- Keep aggregate gate and production activation in #32. This slice leaves `SHARED_DURABLE_ACTIVATION` false.
