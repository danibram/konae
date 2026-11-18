# #34 discovery: authorized analysis of stored agent traces

Status: discovery proposal only, based on the #32 validated MVP at `917b8fb582ea61ffa77b58a8a0b3b5786af260db`. No server behavior, collection, store, telemetry, operator feature, or authorization is added here.

## Source and audience

This addresses [GitHub #34](https://github.com/danibram/konae/issues/34), whose parent is the exact [#25 PRD](https://github.com/danibram/konae/issues/25). The audience for the proposed report is Konae maintainers deciding which tool refusal and recovery categories to improve. The operational question is: **Which tool refusal and recovery outcome categories should maintainers prioritize, based on owner-authorized traces, without reading conversation text or tool payloads?**

The default decision is **owner-only inspection and JSON download are sufficient for the current product**. Use existing records for owner support/debugging; do not run cross-owner analysis by default. If a later product decision funds aggregate analysis, use the narrowly scoped, explicit and revocable authorization below. Alternatives considered:

| Choice | Decision | Reason |
| --- | --- | --- |
| Current owner inspection/download only | Chosen now | Already supports the MVP question for an individual owner and preserves the established boundary. |
| One-time owner-authorized, minimized aggregate report | Future option | Could answer category-level maintenance questions without granting raw detail. Requires product, privacy and operational approval before implementation. |
| Installation-wide operator access to traces | Rejected | Operator status is not owner consent; raw transcripts and payloads are unnecessary for the question. |
| External analytics or LLM analysis | Rejected | Adds a recipient, transfer and retention surface with no need established by #34. |

## Existing behavior (supported today)

Owner conversation routes require the signed-in owner browser session and project ownership. The #32 loopback fixture positively checked an owner and refused a second real owner, an app-account cookie, and a delegated `read` token on all ten existing conversation routes. A project-readable role does not imply conversation access. The role string `operator` has no conversation privilege.

The owner can read a bounded 50-entry timeline, page older entries, expand an entry for raw details, fetch blobs on demand, download versioned history, and confirm deletion. The reader does not claim an execution lease or call a provider. Deletion is verified by a fresh list read. App export excludes owner conversation records. These behaviors and their limitations are recorded in [#31 owner history](issue-31-owner-history.md) and [#32 MVP acceptance](issue-32-mvp-acceptance.md).

There is no cross-owner report, operator trace reader, aggregate grant, retention control for such a grant, or guarantee that deleting active history erases older whole-installation copies. This document does not claim otherwise.

## Proposed future authorization semantics

This is a testable permission model, not an implementation request. A future aggregate authorization must be a distinct owner action, scoped to the purpose `category-report`, the named report fields, and an explicit time window. It must name an expiry and a retention deadline, disclose that no raw text or payload will be read, and be revocable by that owner at any time. No default-on, inferred, bundled, role-derived or delegated-read authorization is valid. A role name, account type, operator session, project grant, ordinary read, or past consent cannot substitute for this action.

The report is computed from existing conversation/entry/tool/effect records. It may use tool category, refusal code/category, recovery outcome, stored duration, truncation marker, and declared intent only when an explicit declaration is present and bound to its source message. Missing intent remains missing. Do not infer it from prompts, answers, file names, tool arguments or outcomes. Do not collect/store a second event stream or create an operator transcript. Keep raw transcript, private reasoning, and tool argument/result payloads out of the report and out of external analytics or model-provider requests.

Before release, combine authorized records into categorical counts and coarse duration buckets; suppress categories with fewer than a proposed minimum cohort of five distinct owners, including complementary cells that would reveal a suppressed count. Do not show per-owner rows, timestamps, project names, exact rare refusal strings, or drill-down links from an aggregate. A report should be reproducible from the selected existing records and authorization receipts without retaining source content in a new report store.

### Detail and deletion boundaries

Any future permitted detail link must resolve to an existing owner-authenticated route and carry server-verifiable provenance: authenticated owner, owning project, conversation, source entry/sequence, requested purpose, and current authorization state. Authenticate every link at read time; a URL, opaque ID, export, report row or cached result is not a bearer grant. The report must not link to another owner's detail. In the current behavior, only the owning signed-in owner can open detail/download routes.

Revocation immediately prevents new aggregate computation and detail resolution under that authorization. Each owner may delete their conversation using the current confirmed delete and verify it via a fresh owner list read. Aggregate recomputation excludes deleted records. If a future export/report has already been produced, its declared retention deadline bounds its lifetime; deleting a source cannot claw back a copy already downloaded by an authorized recipient. Installation snapshots/backups may retain older records until their own expiry/rotation; the current MVP does not claim retroactive erasure from such copies. A future grant must state this limit plainly and must not extend backup retention.

### Audit separation and timing

Operational audit answers who invoked an owner/operator tool and whether authorization succeeded. Trace assertions answer what the browser claims happened (for example client elapsed time, truncation, or declared intent). Keep them separate in storage, display and interpretation. Do not treat client timing as trusted server time or an SLO; browser suspension, clock changes and network/provider latency affect it. Use server receipt/order for persisted sequencing. A report may show coarse client-reported duration only as such, alongside unknown/missing when absent.

## Synthetic sample report

Question: **Which refusal and recovery categories should maintainers prioritize?** The following values are synthetic and intentionally categorical. They are not live traces or measured Konae usage.

| Category | Synthetic records | Owners | Proposed report |
| --- | ---: | ---: | --- |
| stale-source refusal | 6 | 5 | visible; review source-conflict explanation |
| permission-denied refusal | 4 | 3 | suppressed (low cohort) |
| interrupted-recovery | 7 | 5 | visible; review continuation recovery path |
| response-truncated | 2 | 2 | suppressed (low cohort) |
| unknown-write-outcome | 1 | 1 | suppressed; never expose rare detail |

Visible duration buckets (client-reported, descriptive only): `<1s`, `1–5s`, `>5s`, `unknown`. Declared intent appears only as an owner-authored categorical label if present and source-bound; this sample contains no intent value. Absence is not rendered as “unknown intent” and is never inferred.

## Synthetic permission fixture

Run `bun tests/durable-mvp/authorized-analysis-fixture.mjs`. The fixture defines two primary synthetic owners, three additional cohort-only synthetic owners, synthetic conversation text/payload markers and an operator actor. It verifies current owner-only semantics and proposed aggregate grant semantics without starting Konae, connecting to GitHub, or reading any real conversation. The positive aggregate is visible only to the explicitly named synthetic report recipient after all five owners grant the scoped purpose, fields and window; the `operator` actor and an unapproved report recipient receive no result.

| Check | Current supported policy | Proposed aggregate policy |
| --- | --- | --- |
| Owner A reads A detail | allow | detail remains owner-only |
| Owner A reads B text/payload | refuse | refuse |
| Operator role / ordinary project read | refuse | refuse |
| Explicit, scoped grants from all five cohort owners | no aggregate API exists | named report recipient sees one categorical count |
| Operator role or unapproved recipient | no aggregate API exists | no report result |
| Owner B revokes after visible report | no aggregate API exists | cohort falls below five; report disappears |
| Owner A's source record is deleted | confirmed owner deletion | deleted row excluded; category falls below five |
| Owner B revokes | no aggregate API exists | deny next report and every detail resolution |
| Cohort under five owners | no aggregate API exists | suppress, including complementary disclosure |
| Retention deadline elapsed | no aggregate API exists | expire derived report authorization/output |
| Detail link not matching stored owner/project/conversation/entry or retention | refuse | refuse |
| Client duration / intent provenance | existing record claims only | descriptive; intent requires source-bound declaration |

The fixture is a policy demonstration, not evidence that this proposed authorization exists in Konae. Existing live access evidence remains the #32 route fixture: positive signed-in owner; refusal for independent owner, app-account cookie and ordinary delegated `read` across ten routes.

## Validation and limits

The accepted validation target is a synthetic actor/record fixture plus the existing #32 owner route test. Do not extend the route contract, add roles or exercise operator tools against real conversations for this discovery. Existing owner-history read-back contracts: after writes through the owner API, observe through the same door before claiming success; deletion is confirmed and then verified by a fresh owner list; download is a versioned owner-authenticated export. `session_inspect.can` governs tool capability, not login flags or role labels.

Reproduce the synthetic semantics:

```sh
bun tests/durable-mvp/authorized-analysis-fixture.mjs
```

Reproduce the existing real loopback owner-policy fixture (requires the already-built local test binary and uses temporary synthetic installation data only):

```sh
bun run test:durable-mvp:access
```

Documentation link validation:

```sh
bun test scripts/docs-site.test.ts --timeout 30000
```

This deliverable is discovery only. It does not alter collection, storage, owner permissions, operator capabilities, deployment, release, external forwarding, or the validated #32 execution contract.
