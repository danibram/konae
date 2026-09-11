# Build the real agent on pi-agent-core, in the browser, behind a validation gate

Status: accepted — built in `src/agent/*` (capsule-env, gate, tools, harness, providers, system-prompt, key-store, catalog, contracts). The versioned, one-click-undo Apply described below is the one part still outstanding: Apply writes the changed files into the Project without a version bump. ADR-0004 supersedes the schema-file detail for the next Managed Capsule: the agent will edit `schema.ts` rather than `schema.sql`; the scratch workspace, loop and Validation Gate are unchanged.

The current agent is a keyword-matching demo that proves the edit/apply/preview loop. The real agent will be built on `@earendil-works/pi-agent-core` + `@earendil-works/pi-ai` (MIT), running the actual pi agent loop entirely in the browser over an `ExecutionEnv` backed by the Capsule's in-memory files, rather than reimplementing one. Verified against the published packages: the core imports no `node:` builtins, the only platform seam is `ExecutionEnv` (`FileSystem + Shell`), and `pi-ai` ships native Anthropic-style prompt caching and a multi-provider model layer — so the loop runs browser-clean when given browser-native adapters and tools.

> Implementation note (0.84.2): the driving class is pi's `Agent`, not `AgentHarness` — in the shipped 0.84.x package `AgentHarness` is an unimplemented skeleton. The substance of this decision (real loop, streaming, interruption, incremental edits, injectable env/tools/model) is unchanged; see `src/agent/PI-API.md` for the verified API surface.

## Decision

- **Harness, not a single structured call.** `AgentHarness` gives us the loop, streaming, steering, interruption, sessions and compaction for free, and incremental `edit` diffs instead of whole-file rewrites. A single structured call was the earlier design; it loses streaming UX, multi-turn repair, and dies the day a Capsule outgrows one prompt.
- **The agent edits a scratch workspace, never the Project.** A `CapsuleEnv` implements `ExecutionEnv` over a copy of the Capsule's five files and emits mutation events for the UI (live diff). `Shell.exec` is not implemented — no bash tool: for a five-file Capsule it is prompt surface and attack surface without payoff.
- **Nothing applies without passing the Validation Gate.** The sql.js validator runs entirely in the browser: `schema.sql` executes in a disposable database; the migration runs against a clone of the real database and must converge (`sqlite_master` ≡ new schema); every `actions.json` statement must `prepare()` against the migrated clone. It is exposed twice: as a `validate_capsule` tool so the model self-repairs, and as the hard gate on Apply so the invariant does not depend on model behavior. Apply serializes the workspace back into the Project with a version bump (one-click undo). This extends ADR-0001: the agent is browser code, so it gets no path to data that bypasses validation.
- **Provider independence via the model layer, key custody via the Kernel.** `pi-ai` models are swappable (Anthropic direct, OpenRouter, local endpoints). Self-hosted single-user mode may use BYOK from the browser; any multi-user installation must route through a Kernel Admin Interface proxy so provider keys, quotas and audit stay server-side — consistent with ADR-0002.

## Performance plan (the ranking is the decision)

1. **Minimize round trips, not tokens.** The whole Capsule (~few KB) is seeded into the first user message so a typical edit completes in one model turn (two with a validation round) instead of a read-heavy loop. `read` tools remain for future larger Capsules.
2. **Prompt caching from day one** (free in `pi-ai`): stable system prompt carrying the Capsule contract; nothing volatile before the last cache breakpoint.
3. **`edit` emits diffs, not rewrites** — output tokens dominate latency.
4. **The repair loop is local**: sql.js validation costs milliseconds and zero infrastructure; a failed validation costs exactly one extra model turn.
5. **Minimal tool set** (read/write/edit/validate_capsule/preview_errors): smaller schemas, less tool-choice deliberation.
6. **Model and thinking level scale with risk**: fast/low-thinking defaults for UI-only edits; escalate when the change touches `schema.sql`/`actions.json` (the gate already knows).
7. **Lazy-loaded agent chunk**; no just-bash, no Monaco — editor cold start unchanged.

Metrics to hold ourselves to: time-to-first-token, model turns per accepted edit, cache read ratio, validation-pass rate on first attempt.

## Considered options

- **Single structured-output call (provider SDK):** simplest, but no streaming-of-work UX, no incremental diffs, no growth path past a one-prompt Capsule; the validation loop must be hand-rolled around it anyway.
- **A larger IDE-style framework around the harness:** more capability than a five-file Capsule editor needs; Littlebox keeps its own thin presentation (chat + preview + diff) and takes only the harness.
- **Hand-rolled loop:** rebuilds streaming, sessions, compaction and abort for no benefit over a 4-dependency MIT package.
- **In-browser local models (WebLLM/Phi):** rejected as the primary path — multi-file schema/actions coherence is where small models degrade, and gigabyte downloads contradict a product whose rule is zero marginal weight. Remains a future adapter behind the same model layer.

## Consequences

Two external runtime dependencies (`pi-agent-core`, `pi-ai`), pinned; both are small tree-shakeable ESM and forkable if abandoned. A loop spends more tokens than a single call — accepted, bounded by levers 1–3. The scratch-workspace + gate design means agent capability can grow (more tools, bigger capsules, stronger models) without ever widening its authority over user data.

Build order: (1) `CapsuleEnv` + harness + edit tools with caching — proof of life on a CSS edit in one turn; (2) Validation Gate + `validate_capsule` before the agent may touch `schema.sql`; (3) measured latency tuning against the metrics above.
