# What five showcase apps found

Date: 2026-11-06. Five apps were built in one afternoon as product probes, each forcing one group of primitives, and each with a replay that publishes it on its own Kernel, runs every flow and refusal, and runs the Action Contract's conformance checks. All five replays pass. Their sources, replays and per-app tables are in [examples/showcase](../../examples/showcase/README.md). This document consolidates the tables and orders what they found by the job in AGENTS.md it hurts.

The yardstick: a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.

## What held

- Two accounts never saw or changed each other's rows, in every app, through actions and through routes.
- Private files stayed private across members, visitors, API keys and an import; public files loaded under the Content Security Policy with no violation in any app.
- Required verification, recovery, suspension, Invitation only and Allowlist by domain refused what they should, with one answer for every refused signup.
- A signed route refused a bad signature, a stale timestamp and a replay; a key scoped to nothing could not be created; a revoked key was refused.
- One multi-row booking was atomic, and under 20 races and one burst of ten, exactly one booking won each time, because the Kernel serializes actions per Deployment and a partial unique index backs it.
- Eject kept sessions and rows; a package import kept IDs, passwords, rows, files, roles and the policy; a restore kept data, keys and Signals and signed everyone out.
- The Action Contract named every column in four apps and `conform` passed 67 to 84 checks per app against a real Kernel. After the fixes of 2026-11-06 it passed 42 in inbox, whose two routed actions are now probed on their routes only, 70 in club, 76 in booking and 87 in habits, with none skipped. After the review of 2026-11-07 it passes 42 in inbox, 70 in club with 2 skipped, 76 in booking and 71 in habits with 2 skipped. Since conform sends a HEAD to each GET route, also on 2026-11-07, it passes 43 in inbox, 77 in booking and 72 in habits; club has no GET route. Habits fell because its routes declare `"callers": ["key"]` and answer only there. Each skip is a guard on an INSERT that conform's second identical call cannot reach, and each app's replay proves that guard itself.

## What hurt, by job

### Job 5: who may read a row stays the app's decision

| Finding | Apps | Exact answer |
| --- | --- | --- |
| An unsigned GET route on a private app hands its rows to anyone. No declaration says "key required". Fixed on 2026-11-06: `"callers"` names who may call a route, and a private app's unsigned route defaults to a key or a session | inbox | 401 `authentication-required`. Was 200 with rows, no session or key |
| A signed route's action is callable without the signature through `actions_invoke`: a member stored a webhook event, a visitor of a public app checked in a booking. Fixed on 2026-11-06: an action whose route does not answer anyone answers only there | inbox, booking | `Action events.receive answers only on its HTTP route, POST /hooks/events.` Was `{ "changes": 1 }` |
| The signature covers the timestamp and the body, not the method, path or query string; two signed GETs in one second collide and a captured signature works for any query. Fixed on 2026-11-06: the MAC covers the method, the path, the canonical query string and a nonce | inbox | 401 `invalid-signature` for another query; 200 for a second request in the same second. Was 409 `replayed-signature` |
| A refused role check answers like a missing row. Usage counts it as a success and the contract has no refusal for it, so `conform` would pass a backend that dropped the check. Fixed on 2026-11-06: a mutation may declare a `guard` | club, booking | `Only an organizer can remove a book.`, counted under Usage's `refused` and probed by `conform`. Was `{ changes: 0, lastInsertId: 0 }` |
| The runtime endpoint ignores an API key instead of refusing it. Fixed on 2026-11-06 | booking | 403 `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.` Was visitor answers |
| A role inside the app has no bridge to administration: an organizer cannot invite; the owner promotes by pasting a user ID into a `database_execute` string with no bound parameters. Partly fixed on 2026-11-07: the Project Owner's own app account has the role `owner`, from Open as owner or `app_owner_enter`, and `{ "source": "actor", "field": "role" }` lets a guarded action promote from the app's own admin area, as the starter does; an organizer still cannot invite | club, gallery | `Unknown tool or insufficient permission.` |

### Job 3: the same checks at both doors

| Finding | Apps | Exact answer |
| --- | --- | --- |
| Gate and Kernel refuse the same thing with different words: a leading `WITH`, a trigger, a view, a second statement. Fixed on 2026-11-06: both run `kernel-rs/src/sql-rule-cases.json` | habits, booking | `Action habits.streaks is a query, so it must start with SELECT. Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).` at both doors. Was Gate `Runtime query only allows: SELECT. ...`, Kernel `... contains disallowed SQL.` |
| The Gate's hint for a CTE points at `schema.sql`, which is not where a CTE belongs. Fixed on 2026-11-06 | booking | `... Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.` Was `... Use schema.sql for structural changes.` |
| A Signal is recorded when its mutation changed no row: 12 of 8, 61 of 24, and a `signal` action records for an event that does not exist. Fixed on 2026-11-06: a mutation records its Signal only when it changed a row, and the inbox moved its Signal onto the guarded mutation | inbox, habits, booking | 8 of 8, 24 of 24. Was `{ "recorded": true }` |
| A Signal cannot carry the actor, so the owner cannot count per person. Fixed on 2026-11-06: `actor` among a Signal's properties records the caller's ID | inbox, habits | `{ ..., "actor": "<user or key ID>" }`. Was `names signal property user, which is not one of its input bindings.` |
| The `actor` binding yields an ID and no email or name, so apps store a self-declared email, and one member stored another's | inbox, booking | `{ "changes": 1 }` |
| `conform` skips actor-bound routes even with `--key`, and fails signed probes that land in the same second. Fixed on 2026-11-06: it sends the key or the session to such routes, and each signed probe carries its own nonce | habits, inbox | `87 passed, 0 failed, 0 skipped.` Was `SKIP ... Binds the caller's ID` |
| Seven aliased expression columns are typed `unknown` with no warning | booking | |

### Job 1 and 2: both people make the same app

| Finding | Apps | Exact answer |
| --- | --- | --- |
| `konae build` drops `about.html` without a word; a two-page app cannot be published from the CLI. Fixed 2026-11-06: every page at the root and under `pages/` publishes, and a missing reference stops the build | gallery | artifact holds `index.html` only |
| Every unknown GET path serves the home page with 200; extensionless paths do not resolve; `404.html` is never read. Fixed 2026-11-06: 404 with `404.html`, `/about` serves `about.html`, `"routing": "spa"` keeps the old fallback | gallery | 200, home page |
| `konae publish` sends no `initialSignupPolicy`: a public app starts Open, a private one Closed, and Invitation only needs a second credential with `manage_users`. Fixed: `konae.json` names `project.manifest.signupPolicy`, publish sends it on a first release and warns on a later one that differs | inbox, club, gallery, booking | `Unknown tool or insufficient permission.` |
| An app Closed from its first release can admit no one, not even by invitation; the invitation is sent anyway with `acceptable: false`. Fixed: an owner's invitation is acceptable under Closed and a signup stays refused (ADR-0003) | gallery | 403, now 200 |
| Fixed 2026-11-06. A file cannot be made public later or deleted; a draft is re-uploaded and the private copy keeps counting against the quota | gallery | Now `files_update` and `files_delete`, uploader only; a deleted file frees its quota and reads as `File not found.` |
| Fixed 2026-11-06. The SDK's Blob URL cannot be re-read with `fetch` under `connect-src 'self'` | gallery | `violates ... "connect-src 'self'"`. Now `miniapp.files.url(fileId)` returns `/_konae/files/<id>`, which `fetch` reads under the same policy |
| A route always answers `{ "result": [rows] }`; no feed shape, no absolute URLs (the statement cannot read the origin), no CORS, `no-store` on everything | gallery | |
| No per-action rate limit: every anonymous action shares 30 calls per 10 seconds per address, about 260,000 contact messages a day from one address | gallery | |
| Inputs have type and length but no allowed values and no integer type; a CHECK is the only guard and its SQLite text reaches the visitor | gallery, booking, inbox | `CHECK constraint failed: ...` |
| The only clock is `now` in UTC milliseconds; no time zone for a local day | habits, booking | |
| The runtime endpoint answers `rate-limited` as HTTP 200 with `isError`; a route answers 429 | gallery | |
| Under required verification the CLI cannot create the first owner in one step. Fixed on 2026-11-07: `konae login --verify-from <data folder>/mailbox.jsonl` answers the link from a development mailbox on a loopback Kernel, and without the flag the CLI names both steps | habits | Exit 0 and a saved credential. Was 202 `link-sent` |

### Job 4: take the app to another machine

| Finding | Apps | Exact answer |
| --- | --- | --- |
| The package drops API keys, against what CONTEXT.md and docs/mcp.md say; the destination cannot see which programs had access. Fixed: keys travel revoked at import and `api_keys_list` names them (ADR-0008) | club | `{ keys: [] }`, now one key with `revokedReason: "import"` |
| `McpClient.upload` has no `component` argument; the replay calls `transfer_begin` itself. Fixed: `upload(bytes, projectId, component)` | club | |
| The development mailbox writes no subject or body, so the inviter's name cannot be checked without a transport | inbox | |

### Smaller

Fixed 2026-11-06. The SDK stored a private file when `visibility` was anything but `"public"` (club); it now refuses with `File visibility must be private or public.` A visitor's 401 on a private app's file path against 404 on a public one (club) is now the documented rule: 401 before any lookup on a private Deployment, 404 for a file the caller may not read.

A suspended member's session answers as signed out and only the login route says suspended. A publish with two unset secrets names only the first (fixed: one refusal now names every unset secret). A stale timestamp and a wrong secret got the same answer, fixed on 2026-11-06 with `stale-timestamp`. An actor-bound route refuses a visitor with 400 rather than 401. A signup with a password under required verification accepts and ignores the password. A signup for an active address answers 202 and tells no one. HEAD on a route answers 405 with a body HTTP drops, and the Action Contract did not list that 405 (fixed on 2026-11-07: the contract lists it and conform checks its status). The Action Contract's Markdown omitted the Signal (fixed on 2026-11-07).

## What to do first

1. Done on 2026-11-06, see ADR-0013. Bind a signed route to its action: an action with a signature or a key requirement is not callable through `actions_invoke`, and a route on a private app without a signature requires a key or a session. Include method, path and query in the MAC and add a nonce. This is the only finding that opens data the app meant to close.
2. Done on 2026-11-06, see ADR-0010. Make a refused mutation visible: a declared `refuse` clause, or a convention that `changes: 0` with a declared guard answers `forbidden`, so the page, Usage, the Signal and the contract all see a refusal. Record Signals only when `changes > 0`, and let a Signal name the actor.
3. Done on 2026-11-06, see ADR-0004. One refusal text per rule at both doors, with a test that walks every shared case through the Gate and the Kernel and compares the words, as `http-binding-cases.json` and `signal-cases.json` already do for two groups.
4. Done on 2026-11-06, see ADR-0006. The static publishing contract from product-fit: multi-page artifacts in the CLI, 404, extensionless paths, `404.html`.
5. Done on 2026-11-06, see ADR-0003 and `project.manifest.signupPolicy` in docs/mcp.md. Initial signup policy in `konae publish` and `konae.json`; invitations allowed under Closed when the owner sends them.
6. File lifecycle: publish and delete. Done 2026-11-06: `files_update`, `files_delete` and `miniapp.files.url`.
7. Done on 2026-11-06, see ADR-0008. Keys in the package, and the docs made true either way.

Items 1 to 3 touch `http.rs`, `usage.rs`, `actions.ts` and the Gate together and go as one delivery. Items 4 to 7 are disjoint and can run in parallel.

## What the matrix adds

The [capability matrix](./capability-matrix.md) compares ten app shapes with 25 features. After the five additions of 2026-11-07, 146 cells work, 94 work with a compromise and 10 are not possible. All 88 required cells work or have a stated compromise, so none of the ten shapes has a required cell marked impossible.

| Addition | What it closes | Evidence |
| --- | --- | --- |
| Custom domain, ADR-0014 | A Deployment answers on a hostname whose TXT record proves control | Domain probe and gallery replay |
| Notices, ADR-0012 | A changed mutation emails the owner or the verified caller | Notify probe, booking and inbox replays |
| Scheduled mutations, ADR-0011 | The Kernel changes due rows without a person calling | Schedule probe, booking and habits replays |
| Provider signatures, ADR-0013 | Routes verify provider signatures and read nested JSON fields | Hub probe and inbox replay |
| Outbound calls, ADR-0013 | The Kernel sends signed or keyed requests and retries failures with bounded backoff | Calls probe, starter replay and `cargo test calls::tests` |

The integration hub's retries use the call worker's fixed calendar, with at most five attempts after waits of 30 seconds, 1, 2 and 4 minutes. An app cannot declare its own retry calendar. Changed scheduled mutations and call answers now queue their declared notices. A schedule can send the owner a generic reminder, but cannot fill a digest from changed rows or address those rows' account IDs. Notices reach the owner or the verified caller, rather than an assignee or followers.

The three declaration differences found by the original probes are closed. Both doors refuse an actor email binding and an email action kind, parse supported schedules, and refuse subscriptions. Shared cases and the platform probe check those answers. The owner's app account now has role `owner`; starter checks `actor.role` when appointing other roles. Invitations still require an owner tool.

No replay drives the editor's build agent. F24 remains a compromise even though the managed publish path, CLI and shared parser cases pass.
