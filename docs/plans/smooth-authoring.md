# Smooth authoring under the same guardrails

Opened 2026-11-08. The yardstick is the first sentence of AGENTS.md. This plan makes any kind of small app easier to start and harder to get silently wrong, without moving a rule out of the Capsule and without letting the Gate and the Kernel drift.

## Why

A review of the primitives (guard, notify, schedule, signal, http, call) and a look at comparable products found that the size of the glossary is not what makes Littlebox hard. Two numbers are: an author meets about 12 concepts and touches 4 to 6 files before an app with sign-in runs, and an `actions.json` for a realistic app runs 10 to 22 lines per action. Platforms that feel smaller hide identity and rules in the installation, which Littlebox cannot do because the app leaves with its accounts and rules. What Littlebox can do is have those things born with the app, expressed as presets, and refused loudly when written wrong.

Findings this plan acts on, with evidence:

1. A misspelled or misplaced top-level key passes both doors and does nothing. `notify`, `schedule` and `subscribe` on a `kind: call` action pass (`src/kernel/actions.ts` returns before `parseNotify`; `kernel-rs/src/main.rs` `validate_action` returns after `calls::check_kind`), and an unknown key such as `"email"` on a mutation passes. Neither `ActionDefinition` nor `CallKeys` denies unknown fields. 7 of 40 kind-by-key cells are silent.
2. A route on a signal action is refused with "must bind GET for a query and POST for a mutation", which never says a signal action takes no route.
3. A scheduled mutation stays callable by every member its access admits. Probes guard it with `AND ?2 IS NULL` on an actor binding.
4. A GitHub or Stripe webhook needs six signature keys copied from prose, and the Gate refuses them one at a time.
5. Five declarations are refused only at publish (unset secret, malformed `svix` secret, http URL off the installation, editor host, call secret on a route), so "Validated" in the editor does not mean "publishable".
6. A new project starts from Tiny Tasks (`src/lib/starter.ts`), an anonymous shared list. Every app with members, roles, a public form, a webhook or a digest starts by rewriting it. The starter showcase repeats one role predicate in 5 statements.
7. Refusals are sentences, so tools match text (ADR-0020).

## Work packages

Each package owns its files. A package that needs a change in a file another package owns asks for it in its report instead of editing.

### A. No silent keys (findings 1, 2, 3)

Owns `src/kernel/actions.ts`, `src/kernel/calls.ts`, `kernel-rs/src/main.rs` (action parsing and `validate_action`, invoke path), `kernel-rs/src/calls.rs`, `kernel-rs/src/action-cases.json`, `kernel-rs/src/call-cases.json`.

- Each kind has an allowlist of top-level keys. Any other key is refused at both doors with `Action X has a key Y that a <kind> action does not take.` Nested objects already refuse unknown keys; keep them as they are.
- A signal action with `http` is refused with a sentence that says a signal action takes no route.
- A mutation with `schedule` and no `http` answers `Action X runs only on its schedule.` on `actions_invoke` and on the page bridge, beside the routed and `then` branches that exist. The preview and `conform` follow.
- Every new refusal is a case in the shared case files, so both doors run it.
- Update the examples and probes whose `actions.json` now fails, and docs/sdk.md where it lists keys.

### B. Signature presets (finding 4)

Owns `src/kernel/route-declaration.ts`, `src/lib/provider-signature.ts`, `src/lib/route-signature.ts`, `kernel-rs/src/http.rs` (signature parsing only), `kernel-rs/src/provider.rs`, the provider case files, docs/sdk.md section "A provider's scheme".

- `"signature": { "provider": "stripe" | "github" | "svix" | ..., "secret": "<name>" }` expands at both doors from one provider table in `kernel-rs/src/` that the Gate imports. The long form stays for every other sender.
- A preset with a long-form key beside it is refused, so there is one spelling per route.
- When the long form is wrong, the refusal names every wrong key at once, not one per attempt.
- Rewrite `examples/showcase/inbox` routes to the preset and keep its replay green.

### C. A project that is born working (finding 6)

Owns `src/lib/starter.ts`, a new `src/lib/templates/` directory, the new-project UI in `src/components/ProjectDashboard.tsx` (creation flow only), `src/agent/system-prompt.ts` (only the part that tells the agent where a project starts).

- Templates, each a complete Capsule that passes the Gate and runs on `littlebox dev`: shared list (today's Tiny Tasks), members with roles (profiles, a role column, one named role check, promote and demote), public form with an email to the owner, webhook receiver (uses the B preset once B lands; until then the long form), nightly digest (schedule plus notify to the owner), file gallery (`miniapp.files`).
- The agent picks the template that matches the description before its first edit and says which one it picked. A person picks from the same list in the dashboard.
- Each template has a test that runs it through the Gate, and the members template replaces the repeated role predicate with the shortest form the Kernel accepts today.

### D. Validated means publishable (finding 5)

Owns `src/agent/gate.ts`, `src/components/action-readiness.ts`, `src/components/IntegrationsView.tsx`, the readiness part of `src/components/DeployDialog.tsx`, the Kernel's release-plan sentences in `kernel-rs/src/` only if they need a shared table.

- The Gate takes the project's secret names (never values, from `secrets_list`) and the installation's host facts the editor already knows, and refuses the five publish-only declarations with the sentence `release_plan` uses. The footer shows it before Deploy, with a link to Integrations when the owner must act.
- Without secret names (offline, preview) the Gate says nothing about secrets, as now.

### E. Refusal codes (ADR-0020)

Starts after A, B and D land, because it touches every refusal they add. Not dispatched in the first wave.

## Not in this wave

`{changes}` and `{kernel.now}` placeholders and `to` naming a column of account IDs for notices; a "What happened" owner view over notices, calls and scheduled runs; splitting CONTEXT.md and the agent prompt into author and maintainer parts. They are the next wave once A to D land.

## Done when

Each package's tests pass on both doors (`bun test` for the Gate, `cargo test` in `kernel-rs`), the showcase replays still pass (`bun examples/showcase/replay-all.ts`), and the package's report lists every refusal it added with its exact sentence.

## Status, 2026-11-08

Packages A to D landed on the branch `smooth-authoring`. Kernel `cargo test` 264 passed. Editor and Gate `bun test src scripts` 742 passed. The showcase replays pass for booking, club, gallery and inbox.

Open, carried from the work brought into the branch (each also fails at commit 38f2251, before any package):

- `tests/security-review-accounts.integration.test.ts` fails 7 of 11 when run alone.
- The habits and starter replays stop at publish with "The accepted database model does not match the deployed schema. Apply its migration before publishing." (`kernel-rs/src/mcp.rs`). So package A's changes to `notes.surface` and `days.close` are covered by the schedule probe and `cargo test`, not yet by those two replays end to end.

Next wave:

- Rust runs `kernel-rs/src/release-cases.json`. Today only the Gate reads it, so a Kernel rewording would not fail a test. Inject `Rules` in the test instead of reading `LITTLEBOX_MCP_ORIGIN`, and set each case's secrets with `secret_set`.
- Release facts reach the build agent's Gate (`harness.ts`, `tools.ts`, `App.tsx`). Decision for the owner: an unset secret cannot be fixed by the agent, so either it blocks the agent's apply or it shows as a warning in the agent panel and stays an error at Deploy.
- The workshop footer renders a refusal's `detail` and the Integrations button. `ProjectSecret` carries `use`. `.publish-refusal` gets a style.
- `secrets_list` says whether a svix secret's value decodes as `whsec_`, so the Gate can give the fifth publish-time refusal without seeing the value.
- The build agent can read the template files. The design-system gallery shows the seven-card dialog, and the dialog body scrolls.
- Package E, refusal codes (ADR-0020), then the items under "Not in this wave".
