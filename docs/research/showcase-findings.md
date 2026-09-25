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
- The Action Contract named every column in four apps and `conform` passed 67 to 84 checks per app against a real Kernel.

## What hurt, by job

### Job 5: who may read a row stays the app's decision

| Finding | Apps | Exact answer |
| --- | --- | --- |
| An unsigned GET route on a private app hands its rows to anyone. No declaration says "key required" | inbox | 200 with rows, no session or key |
| A signed route's action is callable without the signature through `actions_invoke`: a member stored a webhook event, a visitor of a public app checked in a booking | inbox, booking | `{ "changes": 1 }` |
| The signature covers the timestamp and the body, not the method, path or query string; two signed GETs in one second collide and a captured signature works for any query | inbox | 409 `replayed-signature` |
| A refused role check answers like a missing row. Usage counts it as a success and the contract has no refusal for it, so `conform` would pass a backend that dropped the check | club, booking | `{ changes: 0, lastInsertId: 0 }` |
| The runtime endpoint ignores an API key instead of refusing it | booking | anonymous actions answer as for a visitor |
| A role inside the app has no bridge to administration: an organizer cannot invite; the owner promotes by pasting a user ID into a `database_execute` string with no bound parameters | club, gallery | `Unknown tool or insufficient permission.` |

### Job 3: the same checks at both doors

| Finding | Apps | Exact answer |
| --- | --- | --- |
| Gate and Kernel refuse the same thing with different words: a leading `WITH`, a trigger, a view, a second statement | habits, booking | Gate: `Runtime query only allows: SELECT. Use schema.sql for structural changes.` Kernel: `Action habits.streaks contains disallowed SQL.` |
| The Gate's hint for a CTE points at `schema.sql`, which is not where a CTE belongs | booking | `Runtime execute only allows: DELETE, INSERT, REPLACE, UPDATE. Use schema.sql for structural changes.` |
| A Signal is recorded when its mutation changed no row: 12 of 8, 61 of 24, and a `signal` action records for an event that does not exist | inbox, habits, booking | `{ "recorded": true }` |
| A Signal cannot carry the actor, so the owner cannot count per person | inbox, habits | `names signal property user, which is not one of its input bindings.` |
| The `actor` binding yields an ID and no email or name, so apps store a self-declared email, and one member stored another's | inbox, booking | `{ "changes": 1 }` |
| `conform` skips actor-bound routes even with `--key`, and fails signed probes that land in the same second | habits, inbox | `SKIP ... Binds the caller's ID` |
| Seven aliased expression columns are typed `unknown` with no warning | booking | |

### Job 1 and 2: both people make the same app

| Finding | Apps | Exact answer |
| --- | --- | --- |
| `littlebox build` drops `about.html` without a word; a two-page app cannot be published from the CLI | gallery | artifact holds `index.html` only |
| Every unknown GET path serves the home page with 200; extensionless paths do not resolve; `404.html` is never read | gallery | 200, home page |
| `littlebox publish` sends no `initialSignupPolicy`: a public app starts Open, a private one Closed, and Invitation only needs a second credential with `manage_users` | inbox, club, gallery, booking | `Unknown tool or insufficient permission.` |
| An app Closed from its first release can admit no one, not even by invitation; the invitation is sent anyway with `acceptable: false` | gallery | 403 |
| A file cannot be made public later or deleted; a draft is re-uploaded and the private copy keeps counting against the quota | gallery | no primitive |
| The SDK's Blob URL cannot be re-read with `fetch` under `connect-src 'self'` | gallery | `violates ... "connect-src 'self'"` |
| A route always answers `{ "result": [rows] }`; no feed shape, no absolute URLs (the statement cannot read the origin), no CORS, `no-store` on everything | gallery | |
| No per-action rate limit: every anonymous action shares 30 calls per 10 seconds per address, about 260,000 contact messages a day from one address | gallery | |
| Inputs have type and length but no allowed values and no integer type; a CHECK is the only guard and its SQLite text reaches the visitor | gallery, booking, inbox | `CHECK constraint failed: ...` |
| The only clock is `now` in UTC milliseconds; no time zone for a local day | habits, booking | |
| The runtime endpoint answers `rate-limited` as HTTP 200 with `isError`; a route answers 429 | gallery | |
| Under required verification the CLI cannot create the first owner in one step | habits | 202 `link-sent` |

### Job 4: take the app to another machine

| Finding | Apps | Exact answer |
| --- | --- | --- |
| The package drops API keys, against what CONTEXT.md and docs/mcp.md say; the destination cannot see which programs had access | club | `{ keys: [] }` |
| `McpClient.upload` has no `component` argument; the replay calls `transfer_begin` itself | club | |
| The development mailbox writes no subject or body, so the inviter's name cannot be checked without a transport | inbox | |

### Smaller

A suspended member's session answers as signed out and only the login route says suspended. A publish with two unset secrets names only the first. A stale timestamp and a wrong secret get the same answer. An actor-bound route refuses a visitor with 400 rather than 401. A signup with a password under required verification accepts and ignores the password. A signup for an active address answers 202 and tells no one. HEAD on a route answers 405 with a body HTTP drops. The Action Contract's Markdown omits the Signal.

## What to do first

1. Bind a signed route to its action: an action with a signature or a key requirement is not callable through `actions_invoke`, and a route on a private app without a signature requires a key or a session. Include method, path and query in the MAC and add a nonce. This is the only finding that opens data the app meant to close.
2. Make a refused mutation visible: a declared `refuse` clause, or a convention that `changes: 0` with a declared guard answers `forbidden`, so the page, Usage, the Signal and the contract all see a refusal. Record Signals only when `changes > 0`, and let a Signal name the actor.
3. One refusal text per rule at both doors, with a test that walks every shared case through the Gate and the Kernel and compares the words, as `http-binding-cases.json` and `signal-cases.json` already do for two groups.
4. The static publishing contract from product-fit: multi-page artifacts in the CLI, 404, extensionless paths, `404.html`.
5. Initial signup policy in `littlebox publish` and `littlebox.json`; invitations allowed under Closed when the owner sends them.
6. File lifecycle: publish and delete.
7. Keys in the package, and the docs made true either way.

Items 1 to 3 touch `http.rs`, `usage.rs`, `actions.ts` and the Gate together and go as one delivery. Items 4 to 7 are disjoint and can run in parallel.
