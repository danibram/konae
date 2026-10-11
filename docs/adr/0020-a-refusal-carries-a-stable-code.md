# A refusal carries a stable code

Status: proposed, 2026-11-08. Builds on [ADR-0009](0009-the-gate-and-the-kernel-share-one-query-bound.md), which made the Validation Gate and the Kernel refuse a statement with the same words, and extends that rule from one sentence to every refusal of a Capsule.

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
