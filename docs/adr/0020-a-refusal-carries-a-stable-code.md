# A refusal carries a stable code

Status: accepted, 2026-11-08. Builds on [ADR-0009](0009-the-gate-and-the-kernel-share-one-query-bound.md), which made the Validation Gate and the Kernel refuse a statement with the same words, and extends that rule from one sentence to every refusal of a Capsule.

Today a refusal is a sentence. The Gate and the Kernel agree on the sentence because both read the shape case files (`kernel-rs/src/action-cases.json`, `call-cases.json` and six more, 261 refusal cases), and that agreement is what lets a check that passes in the editor pass on the machine an app leaves on. A sentence is the right thing to show a person. It is the wrong thing to branch on. The build agent, the editor's readiness footer and `conform` all match on text, so rewording a refusal breaks them, and nothing tells an agent which refusals it can fix by editing `actions.json` and which need the owner (a missing secret, an unverified sender). MCP tools answer a refused call with prose and only `domain.rs` and `migration.rs` return a `next`.

Other platforms that agents build on settled this the same way: Postgres returns SQLSTATE `42501`, Supabase's advisors return `0013 rls_disabled_in_public`, Lakebed returns `lakebed_quota_exceeded` with a `bucket`. Each keeps the human text free to change and the identifier fixed.

## Decision

Every refusal of a Capsule, from the Gate or the Kernel, is `{ code, message, hint? }`.

- `code` is a lowercase dotted identifier naming the rule, such as `query.limit-missing`, `action.key-unknown`, `route.signature-secret-unset`. It never changes once released. A rule that changes meaning gets a new code.
- `message` is the sentence both doors already share. The same-words rule of ADR-0009 still holds for it.
- `hint` names who can fix it and the next step: `"fix": "capsule"` when an edit to the Capsule fixes it, `"fix": "owner"` when the owner must act outside the Capsule (set a secret, verify a sender), `"fix": "operator"` when only the installation can. It may name a tool or a field.
- The codes live in one table, `kernel-rs/src/refusal-codes.json`, with each code's `fix` and a one-line remedy. The Gate imports it, the Kernel embeds it, and every refusal case in the shape case files carries the `code` it expects. A case without a code fails the case runner on both sides.
- An MCP tool that refuses answers `{ error: { code, message, hint } }`. A tool that succeeds and leaves work for the caller answers `next`, as `domain.rs` and `migration.rs` do now.
- `platform_guide` gains a `refusals` topic rendered from the table, so an agent reads the remedy for a code through the same door that refused it.

## Considered options

- **Keep sentences and let tools match text.** Rejected because every rewording is a breaking change for the agent and for `conform`, and a sentence cannot say whether the Capsule or the owner fixes it.
- **Numeric codes.** Rejected because a number tells a person reading a log nothing, and the dotted name groups refusals by the part of the Capsule they concern.
- **Codes only on the Kernel.** Rejected because the editor would then branch on text while the Kernel branched on codes, and the two doors would drift in exactly the way ADR-0009 forbids.

## Consequences

The case files grow one field per refusal case, and adding a refusal means adding a code to the table first. The editor footer and the build agent can say "the owner must set the secret `stripe` in Integrations" without parsing a sentence. Codes travel with nothing in a package, because they describe the installation's rules, not the app's data, but an exported Kernel answers the same codes because it embeds the same table.

## Update, 2026-11-08: built

- `kernel-rs/src/refusal-codes.json` holds 112 codes in 14 groups: action, call, connects, domain, guard, mutation, notify, query, route, schedule, schema, signal, signature and statement. Each row has `fix` and `remedy`, and an owner row names the `tool` that clears it (`secret_set`, `domain_set`). No code needs the operator yet.
- The site that refuses names the code: `refuse(code, sentence)` in the Kernel (`kernel-rs/src/refusal.rs`, a `Refusal` error that prints only its sentence) and `refusal(code, sentence)` in the Gate (`src/kernel/refusal.ts`, a `RefusalError`). No sentence changed, so ADR-0009's same-words rule and every replay hold as before.
- A refusal that names several wrong keys at once, as a route's signature does, joins the sentences under the first one's code on both doors.
- All 323 refusal cases in the 11 shape case files carry `code`. Both case runners compare the code and the sentence, and a case without a code fails both. Tests on both sides fail when a table code has no case, and when a source names a code the table does not hold. One case was added, `call-cases.json` "headers written as a list", so `call.headers-invalid` has a case.
- `domain-cases.json` holds `domain_set` hostname rules, which belong to the owner, not the Capsule. They carry `domain.*` codes with `fix: owner`, so `domain_set` answers a code too.
- `google-client-cases.json` holds the owner's refusals for an app's Google client and sign-in methods: `secret_set` and `secret_delete` answer `secret.*` codes and `sign_in_methods_set` answers `signin.*` codes, all with `fix: owner`. The editor's Auth > Users card shows the Kernel's sentence instead of checking the values itself.
- An MCP tool that refuses keeps the sentence in `content[0].text`, which the CLI, `deploy.mjs` and the preview read, and adds `structuredContent.error` with `{ code, message, hint }`. The HTTP deploy route keeps `error` a string and adds `code` and `hint` beside it.
- The Gate's `ValidationError` and `ValidationWarning` carry `code` and `hint`; the build agent sees `Code <code>: <remedy>` under the sentence. `refusalNeedsSecret`, which matched three sentences with a regular expression, is gone: the Gate, the workshop footer and the Deploy dialog ask `ownerFixes(code)`, which reads `fix` from the table. The editor's `McpError` carries the refusal a tool answered, so the Deploy dialog offers Auth > Integrations from the code.
- `platform_guide` topic `refusals` renders the table grouped by part.

Two kinds of refusal have no code. The parse errors that come before a Capsule rule (`actions.json must contain valid JSON.`, a missing bindings array) have no code. The Kernel answers them with serde's text, so no shared case can hold one sentence for both doors. Runtime refusals of `actions_invoke` (a missing input, a value of the wrong type) are not Capsule refusals and have no code either.
