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
| `littlebox build` drops `about.html` without a word; a two-page app cannot be published from the CLI. Fixed 2026-11-06: every page at the root and under `pages/` publishes, and a missing reference stops the build | gallery | artifact holds `index.html` only |
| Every unknown GET path serves the home page with 200; extensionless paths do not resolve; `404.html` is never read. Fixed 2026-11-06: 404 with `404.html`, `/about` serves `about.html`, `"routing": "spa"` keeps the old fallback | gallery | 200, home page |
| `littlebox publish` sends no `initialSignupPolicy`: a public app starts Open, a private one Closed, and Invitation only needs a second credential with `manage_users`. Fixed: `littlebox.json` names `project.manifest.signupPolicy`, publish sends it on a first release and warns on a later one that differs | inbox, club, gallery, booking | `Unknown tool or insufficient permission.` |
| An app Closed from its first release can admit no one, not even by invitation; the invitation is sent anyway with `acceptable: false`. Fixed: an owner's invitation is acceptable under Closed and a signup stays refused (ADR-0011, note on Closed) | gallery | 403, now 200 |
| Fixed 2026-11-06. A file cannot be made public later or deleted; a draft is re-uploaded and the private copy keeps counting against the quota | gallery | Now `files_update` and `files_delete`, uploader only; a deleted file frees its quota and reads as `File not found.` |
| Fixed 2026-11-06. The SDK's Blob URL cannot be re-read with `fetch` under `connect-src 'self'` | gallery | `violates ... "connect-src 'self'"`. Now `miniapp.files.url(fileId)` returns `/_littlebox/files/<id>`, which `fetch` reads under the same policy |
| A route always answers `{ "result": [rows] }`; no feed shape, no absolute URLs (the statement cannot read the origin), no CORS, `no-store` on everything | gallery | |
| No per-action rate limit: every anonymous action shares 30 calls per 10 seconds per address, about 260,000 contact messages a day from one address | gallery | |
| Inputs have type and length but no allowed values and no integer type; a CHECK is the only guard and its SQLite text reaches the visitor | gallery, booking, inbox | `CHECK constraint failed: ...` |
| The only clock is `now` in UTC milliseconds; no time zone for a local day | habits, booking | |
| The runtime endpoint answers `rate-limited` as HTTP 200 with `isError`; a route answers 429 | gallery | |
| Under required verification the CLI cannot create the first owner in one step. Fixed on 2026-11-07: `littlebox login --verify-from <data folder>/mailbox.jsonl` answers the link from a development mailbox on a loopback Kernel, and without the flag the CLI names both steps | habits | Exit 0 and a saved credential. Was 202 `link-sent` |

### Job 4: take the app to another machine

| Finding | Apps | Exact answer |
| --- | --- | --- |
| The package drops API keys, against what CONTEXT.md and docs/mcp.md say; the destination cannot see which programs had access. Fixed: keys travel revoked at import and `api_keys_list` names them (ADR-0013, note on keys) | club | `{ keys: [] }`, now one key with `revokedReason: "import"` |
| `McpClient.upload` has no `component` argument; the replay calls `transfer_begin` itself. Fixed: `upload(bytes, projectId, component)` | club | |
| The development mailbox writes no subject or body, so the inviter's name cannot be checked without a transport | inbox | |

### Smaller

Fixed 2026-11-06. The SDK stored a private file when `visibility` was anything but `"public"` (club); it now refuses with `File visibility must be private or public.` A visitor's 401 on a private app's file path against 404 on a public one (club) is now the documented rule: 401 before any lookup on a private Deployment, 404 for a file the caller may not read.

A suspended member's session answers as signed out and only the login route says suspended. A publish with two unset secrets names only the first (fixed: one refusal now names every unset secret). A stale timestamp and a wrong secret got the same answer, fixed on 2026-11-06 with `stale-timestamp`. An actor-bound route refuses a visitor with 400 rather than 401. A signup with a password under required verification accepts and ignores the password. A signup for an active address answers 202 and tells no one. HEAD on a route answers 405 with a body HTTP drops, and the Action Contract did not list that 405 (fixed on 2026-11-07: the contract lists it and conform checks its status). The Action Contract's Markdown omitted the Signal (fixed on 2026-11-07).

## What to do first

1. Done on 2026-11-06, see the ADR-0011 update "a route binds its action". Bind a signed route to its action: an action with a signature or a key requirement is not callable through `actions_invoke`, and a route on a private app without a signature requires a key or a session. Include method, path and query in the MAC and add a nonce. This is the only finding that opens data the app meant to close.
2. Done on 2026-11-06, see the ADR-0010 update. Make a refused mutation visible: a declared `refuse` clause, or a convention that `changes: 0` with a declared guard answers `forbidden`, so the page, Usage, the Signal and the contract all see a refusal. Record Signals only when `changes > 0`, and let a Signal name the actor.
3. Done on 2026-11-06, see the ADR-0009 note. One refusal text per rule at both doors, with a test that walks every shared case through the Gate and the Kernel and compares the words, as `http-binding-cases.json` and `signal-cases.json` already do for two groups.
4. Done on 2026-11-06, see the ADR-0005 note on the static publishing contract. The static publishing contract from product-fit: multi-page artifacts in the CLI, 404, extensionless paths, `404.html`.
5. Done on 2026-11-06, see the ADR-0011 note on Closed and `project.manifest.signupPolicy` in docs/mcp.md. Initial signup policy in `littlebox publish` and `littlebox.json`; invitations allowed under Closed when the owner sends them.
6. File lifecycle: publish and delete. Done 2026-11-06: `files_update`, `files_delete` and `miniapp.files.url`.
7. Done on 2026-11-06, see the ADR-0013 note on keys. Keys in the package, and the docs made true either way.

Items 1 to 3 touch `http.rs`, `usage.rs`, `actions.ts` and the Gate together and go as one delivery. Items 4 to 7 are disjoint and can run in parallel.

## What the matrix adds

Date: 2026-11-07. [The capability matrix](./capability-matrix.md) judges ten app shapes against 24 features, 240 cells, with six probes in [examples/probes](../../examples/probes/README.md) for the cells the showcase did not prove. 116 cells work, 77 work with a compromise and 47 are not possible. Of the 87 cells a shape cannot ship without, 16 are not possible, and only the public dataset has none.

Five features account for those 16. Each primitive below is a declaration the Kernel reads from `actions.json` or the Deployment record and runs inside its one process, so no app gets a process of its own and no app runs code of its own, as the product rule in [the architecture](../architecture.md#product-rule) and [the plan](../plans/portable-personal-apps.md) require.

| Feature | Shapes it blocks | Smallest primitive that unblocks it |
| --- | --- | --- |
| Email on an event | Landing page, team tool, community, form collector, integration hub, shop, two-sided app | A `notify` declaration on a mutation, recorded like a Signal when the statement changes a row and queued as an email job in the existing queue ([ADR-0012](../adr/0012-account-email-goes-through-a-persistent-queue.md)). It goes to the Project Owner or to the calling account, never to an address an input supplies, so the app cannot become a relay. The text is a template in `actions.json` with escaped input values, and the per-app limit of 100 a day holds. Sending to followers waits for a second form that takes account IDs from a query |
| Scheduled or delayed work | Personal tracker, team tool, integration hub; degrades the blog and the shop | A `schedule` on a mutation with no input bindings, such as every 15 minutes or daily at a UTC time, that the Kernel's own timer runs under the Deployment's lock, as it already runs copies, with no actor and a Usage count under `scheduled`. With `notify` it sends a reminder: the scheduled statement marks due rows and the notice goes out for each change. A retry of an outbound call stays out, because no action calls out |
| Custom domain | Landing page, blog or docs site, shop | A hostname alias per Deployment that the owner sets with a tool needing `publish`. The Kernel resolves it next to the generated hostname, treats it as the app's origin for the Origin check and the cookies, refuses one another Deployment holds, and carries it in the package and a migration. TLS stays with the operator's proxy |
| Payments | Shop or booking board, two-sided app | The webhook primitive below. A payment then becomes a link to the provider's hosted page, which the page policy already lets through, and a verified webhook that marks the order paid with the guarded mutation the shop probe uses. Checkout inside the page stays out under `script-src 'self'` and `connect-src 'self'` |
| Webhooks from outside services | Integration hub | A declared `scheme` on a route's signature: an HMAC of the body with an optional prefix such as `sha256=`, an HMAC of `<t>.<body>` read from a `t=...,v1=...` header, and Svix, which the Kernel already verifies for Resend. Each input binding may name a JSON pointer, `"from": "/repository/full_name"`, and the route ignores body fields no binding names. The Gate and the Kernel parse both from one case file, as `http-binding-cases.json` does today |

The last two rows share one primitive, so four additions unblock all 16 cells. The next features down block no shape but degrade the most: search, `LIKE` only, in four shapes; realtime, polling only, in three; roles that only the owner's SQL can start and that never see an email, in three.

The probes also found three places where the Gate and the Kernel answered the same declaration differently. Fixed 2026-11-07 by the editor parity review: both doors run one action-shape check from `kernel-rs/src/action-cases.json`, so `{ "source": "actor", "field": "email" }` is refused before publishing with `Invalid binding 1 in members.join.` and `"kind": "email"` with `Action notes.mail has an invalid kind. Use "query", "mutation" or "signal".` A `"schedule"` or `"subscribe"` key still passes both doors and does nothing, which matters once `schedule` becomes a primitive; the platform probe asserts it.

Update, 2026-11-07: scheduled work is a primitive ([ADR-0015](../adr/0015-the-kernel-runs-a-declared-schedule.md)). A mutation with no input declares `"schedule": { "every": "15m" }` or `{ "daily": "02:00" }`, the Kernel's timer runs it once a period with no actor and counts each run under `scheduled`, and both doors refuse a schedule they do not take and any `subscribe` key with the same words. The matrix moves F18 to C for the landing page, the personal tracker, the team tool, the community and the form collector, where the notice, reminder, digest or summary is written into the app and waits for email on an event to reach anyone, and to W for the blog, the dataset, the shop and the two-sided app, where the work is a change to rows. The integration hub's retry of an outbound call stays N. Of the 87 cells a shape cannot ship without, 14 are now not possible.
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
| `littlebox build` drops `about.html` without a word; a two-page app cannot be published from the CLI. Fixed 2026-11-06: every page at the root and under `pages/` publishes, and a missing reference stops the build | gallery | artifact holds `index.html` only |
| Every unknown GET path serves the home page with 200; extensionless paths do not resolve; `404.html` is never read. Fixed 2026-11-06: 404 with `404.html`, `/about` serves `about.html`, `"routing": "spa"` keeps the old fallback | gallery | 200, home page |
| `littlebox publish` sends no `initialSignupPolicy`: a public app starts Open, a private one Closed, and Invitation only needs a second credential with `manage_users`. Fixed: `littlebox.json` names `project.manifest.signupPolicy`, publish sends it on a first release and warns on a later one that differs | inbox, club, gallery, booking | `Unknown tool or insufficient permission.` |
| An app Closed from its first release can admit no one, not even by invitation; the invitation is sent anyway with `acceptable: false`. Fixed: an owner's invitation is acceptable under Closed and a signup stays refused (ADR-0011, note on Closed) | gallery | 403, now 200 |
| Fixed 2026-11-06. A file cannot be made public later or deleted; a draft is re-uploaded and the private copy keeps counting against the quota | gallery | Now `files_update` and `files_delete`, uploader only; a deleted file frees its quota and reads as `File not found.` |
| Fixed 2026-11-06. The SDK's Blob URL cannot be re-read with `fetch` under `connect-src 'self'` | gallery | `violates ... "connect-src 'self'"`. Now `miniapp.files.url(fileId)` returns `/_littlebox/files/<id>`, which `fetch` reads under the same policy |
| A route always answers `{ "result": [rows] }`; no feed shape, no absolute URLs (the statement cannot read the origin), no CORS, `no-store` on everything | gallery | |
| No per-action rate limit: every anonymous action shares 30 calls per 10 seconds per address, about 260,000 contact messages a day from one address | gallery | |
| Inputs have type and length but no allowed values and no integer type; a CHECK is the only guard and its SQLite text reaches the visitor | gallery, booking, inbox | `CHECK constraint failed: ...` |
| The only clock is `now` in UTC milliseconds; no time zone for a local day | habits, booking | |
| The runtime endpoint answers `rate-limited` as HTTP 200 with `isError`; a route answers 429 | gallery | |
| Under required verification the CLI cannot create the first owner in one step. Fixed on 2026-11-07: `littlebox login --verify-from <data folder>/mailbox.jsonl` answers the link from a development mailbox on a loopback Kernel, and without the flag the CLI names both steps | habits | Exit 0 and a saved credential. Was 202 `link-sent` |

### Job 4: take the app to another machine

| Finding | Apps | Exact answer |
| --- | --- | --- |
| The package drops API keys, against what CONTEXT.md and docs/mcp.md say; the destination cannot see which programs had access. Fixed: keys travel revoked at import and `api_keys_list` names them (ADR-0013, note on keys) | club | `{ keys: [] }`, now one key with `revokedReason: "import"` |
| `McpClient.upload` has no `component` argument; the replay calls `transfer_begin` itself. Fixed: `upload(bytes, projectId, component)` | club | |
| The development mailbox writes no subject or body, so the inviter's name cannot be checked without a transport | inbox | |

### Smaller

Fixed 2026-11-06. The SDK stored a private file when `visibility` was anything but `"public"` (club); it now refuses with `File visibility must be private or public.` A visitor's 401 on a private app's file path against 404 on a public one (club) is now the documented rule: 401 before any lookup on a private Deployment, 404 for a file the caller may not read.

A suspended member's session answers as signed out and only the login route says suspended. A publish with two unset secrets names only the first (fixed: one refusal now names every unset secret). A stale timestamp and a wrong secret got the same answer, fixed on 2026-11-06 with `stale-timestamp`. An actor-bound route refuses a visitor with 400 rather than 401. A signup with a password under required verification accepts and ignores the password. A signup for an active address answers 202 and tells no one. HEAD on a route answers 405 with a body HTTP drops, and the Action Contract did not list that 405 (fixed on 2026-11-07: the contract lists it and conform checks its status). The Action Contract's Markdown omitted the Signal (fixed on 2026-11-07).

## What to do first

1. Done on 2026-11-06, see the ADR-0011 update "a route binds its action". Bind a signed route to its action: an action with a signature or a key requirement is not callable through `actions_invoke`, and a route on a private app without a signature requires a key or a session. Include method, path and query in the MAC and add a nonce. This is the only finding that opens data the app meant to close.
2. Done on 2026-11-06, see the ADR-0010 update. Make a refused mutation visible: a declared `refuse` clause, or a convention that `changes: 0` with a declared guard answers `forbidden`, so the page, Usage, the Signal and the contract all see a refusal. Record Signals only when `changes > 0`, and let a Signal name the actor.
3. Done on 2026-11-06, see the ADR-0009 note. One refusal text per rule at both doors, with a test that walks every shared case through the Gate and the Kernel and compares the words, as `http-binding-cases.json` and `signal-cases.json` already do for two groups.
4. Done on 2026-11-06, see the ADR-0005 note on the static publishing contract. The static publishing contract from product-fit: multi-page artifacts in the CLI, 404, extensionless paths, `404.html`.
5. Done on 2026-11-06, see the ADR-0011 note on Closed and `project.manifest.signupPolicy` in docs/mcp.md. Initial signup policy in `littlebox publish` and `littlebox.json`; invitations allowed under Closed when the owner sends them.
6. File lifecycle: publish and delete. Done 2026-11-06: `files_update`, `files_delete` and `miniapp.files.url`.
7. Done on 2026-11-06, see the ADR-0013 note on keys. Keys in the package, and the docs made true either way.

Items 1 to 3 touch `http.rs`, `usage.rs`, `actions.ts` and the Gate together and go as one delivery. Items 4 to 7 are disjoint and can run in parallel.

## What the matrix adds

Date: 2026-11-07. [The capability matrix](./capability-matrix.md) judges ten app shapes against 24 features, 240 cells, with six probes in [examples/probes](../../examples/probes/README.md) for the cells the showcase did not prove. 116 cells work, 77 work with a compromise and 47 are not possible. Of the 87 cells a shape cannot ship without, 16 are not possible, and only the public dataset has none.

Five features account for those 16. Each primitive below is a declaration the Kernel reads from `actions.json` or the Deployment record and runs inside its one process, so no app gets a process of its own and no app runs code of its own, as the product rule in [the architecture](../architecture.md#product-rule) and [the plan](../plans/portable-personal-apps.md) require.

| Feature | Shapes it blocks | Smallest primitive that unblocks it |
| --- | --- | --- |
| Email on an event | Landing page, team tool, community, form collector, integration hub, shop, two-sided app | A `notify` declaration on a mutation, recorded like a Signal when the statement changes a row and queued as an email job in the existing queue ([ADR-0012](../adr/0012-account-email-goes-through-a-persistent-queue.md)). It goes to the Project Owner or to the calling account, never to an address an input supplies, so the app cannot become a relay. The text is a template in `actions.json` with escaped input values, and the per-app limit of 100 a day holds. Sending to followers waits for a second form that takes account IDs from a query |
| Scheduled or delayed work | Personal tracker, team tool, integration hub; degrades the blog and the shop | A `schedule` on a mutation with no input bindings, such as every 15 minutes or daily at a UTC time, that the Kernel's own timer runs under the Deployment's lock, as it already runs copies, with no actor and a Usage count under `scheduled`. With `notify` it sends a reminder: the scheduled statement marks due rows and the notice goes out for each change. A retry of an outbound call stays out, because no action calls out |
| Custom domain | Landing page, blog or docs site, shop | A hostname alias per Deployment that the owner sets with a tool needing `publish`. The Kernel resolves it next to the generated hostname, treats it as the app's origin for the Origin check and the cookies, refuses one another Deployment holds, and carries it in the package and a migration. TLS stays with the operator's proxy |
| Payments | Shop or booking board, two-sided app | The webhook primitive below. A payment then becomes a link to the provider's hosted page, which the page policy already lets through, and a verified webhook that marks the order paid with the guarded mutation the shop probe uses. Checkout inside the page stays out under `script-src 'self'` and `connect-src 'self'` |
| Webhooks from outside services | Integration hub | A declared `scheme` on a route's signature: an HMAC of the body with an optional prefix such as `sha256=`, an HMAC of `<t>.<body>` read from a `t=...,v1=...` header, and Svix, which the Kernel already verifies for Resend. Each input binding may name a JSON pointer, `"from": "/repository/full_name"`, and the route ignores body fields no binding names. The Gate and the Kernel parse both from one case file, as `http-binding-cases.json` does today |

The last two rows share one primitive, so four additions unblock all 16 cells. The next features down block no shape but degrade the most: search, `LIKE` only, in four shapes; realtime, polling only, in three; roles that only the owner's SQL can start and that never see an email, in three.

The probes also found three places where the Gate and the Kernel answer the same declaration differently. Each touches the contract on both sides, so this review reports them for a delivery that changes `src/kernel/actions.ts`, `kernel-rs/src/main.rs`, docs/mcp.md and CONTEXT.md together:

| Declaration | Validation Gate | Kernel |
| --- | --- | --- |
| `{ "source": "actor", "field": "email" }` | Refuses: `Invalid binding 1 in members.join.` | Publishes; every call answers `Unsupported binding source: actor` |
| `"kind": "email"` | `Invalid server action: notes.mail` | `Action notes.mail has an unsupported kind.` |
| `"schedule"` on a mutation, `"subscribe"` on a query | Passes and ignores it | Publishes and ignores it |

Fix the first one first, because a Capsule that the editor refuses publishes from the CLI and fails at its first call. The third matters once `schedule` becomes a primitive, because today an agent can write one and see both doors accept it.
