# Can the typical apps be built?

Date: 2026-11-07, against `main` at `e9a0711`. This matrix takes ten app shapes people ask for and 24 features they expect, and gives every cell one verdict with the primitive behind it and the replay that proves it. Where no showcase replay proved a cell, a probe under [examples/probes](../../examples/probes/README.md) does, on its own local Kernel. Run them all with `bun run examples/probes/replay-all.ts`. Update of 2026-11-07: F22 Custom domain moved from N to W when the custom domain primitive landed ([ADR-0015](../adr/0015-a-deployment-answers-on-one-custom-domain.md)), proved by the domain probe and the gallery replay.

Updated 2026-11-07 when the custom domain ([ADR-0015](../adr/0015-a-deployment-answers-on-one-custom-domain.md)), notices ([ADR-0016](../adr/0016-a-mutation-may-notify-the-owner-or-the-actor.md)) and scheduled work ([ADR-0017](../adr/0017-the-kernel-runs-a-declared-schedule.md)) landed: F17, F18 and F22, their cells, the counts, the shapes they unblock and the probes that prove them.

The yardstick is the sentence at the top of AGENTS.md: a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.

## Verdicts

- **W, works.** A primitive does it today and a replay asserts it.
- **C, works with a compromise.** The app ships, but something is given up. The legend names what, with the exact answer the Kernel gives.
- **N, not possible.** No primitive does it. The legend says why and what the nearest honest alternative is.

A bold cell is one the shape cannot ship without. Every shape needs F23 and F24, because jobs 2 and 4 in AGENTS.md apply to every app.

## App shapes

| | Shape | What it needs at minimum |
| --- | --- | --- |
| A | Landing page with a waitlist | Anonymous signup to a list, a public count, an email when someone joins or at launch, its own domain |
| B | Personal tracker | One account, private rows, history and totals, reminders |
| C | Private team tool with roles | Invited members, roles, shared files, who changed what, notice of an assignment |
| D | Members-only community with content and files | Invited members, moderators, posts, files and images, search, notice of new posts |
| E | Public blog or docs site with an admin area | Public pages, an admin account, drafts, images, search, a feed, scheduled publishing, its own domain |
| F | Form or survey collector | Anonymous submissions, attachments, tallies, notice of a submission |
| G | Public dataset with a JSON API and a private write API | Open reads with search and paging, keyed writes |
| H | Integration hub | Webhooks from outside services, an API for a program, retries and outbound calls |
| I | Small shop or booking board with inventory rules | Stock that never oversells, holds that expire, payment, a receipt, its own domain |
| J | Two-sided app | One group publishes, another follows and consumes, notices to followers, maybe payment |

## Features

Each row gives the verdict most shapes get, the primitive, what is given up or why it cannot be done, and the proof. A replay's check is cited by its name, as it prints after `PASS`.

| | Feature | Verdict | Primitive, compromise or reason | Proof |
| --- | --- | --- | --- | --- |
| F1 | Open signup | W | Signup Policy Open | habits "Verified signup"; probes community, shop and survey sign people up under Open |
| F2 | Invited signup | W | Invitation only, or Closed with the owner's invitation | inbox "Team by invitation", club "Invitations", gallery "Closed signup, one account" |
| F3 | Allowlisted signup | W | Allowlist with exact emails and `@domain` | booking "Allowlist signup" |
| F4 | Verified signup | W | `LITTLEBOX_ACCOUNT_VERIFICATION=required`. It is a switch for the whole installation, not one app | habits "Verified signup" |
| F5 | Roles inside the app | C | Roles are rows that each action's `WHERE` checks, refused with a guard. The first role comes from the owner's `database_execute`. The `actor` binding yields an ID and never an email, so an admin promotes by ID or by a name the person typed, and an in-app role cannot invite, because invitations are owner tools that need `manage_users` | club "Roles", community "Roles inside the app" (`Only an editor can make someone a publisher.`), community "No email from the actor" |
| F6 | Public and private rows in one table | W | Each action projects and filters its own rows | booking "Public and staff views", gallery "Visitor view", community "Publishers and readers" |
| F7 | Public read API | C | An unsigned GET route on a public app. It always answers `{ "result": [rows] }` as JSON with `Cache-Control: no-store` and no CORS headers, so a server reads it and a page on another origin cannot; an `OPTIONS` preflight answers 405. There is no XML or RSS. A private app's routes take a key, a signature or a session, never a caller with none, so a public API means a public app | gallery "/feed.json", dataset "Read API from another origin" |
| F8 | Private write API with a key | W | A route with `"callers": ["key"]` and an API key that names the action | dataset "Private write API", habits "API key on an actor-bound route", booking "Check-in route" |
| F9 | Signed webhooks in | C | A signed route in Littlebox's scheme only. A provider's own scheme answers 401 `invalid-signature`; a provider's body answers 400 `Unexpected action input: pusher` for a field the action does not declare, `ref must be a non-empty string.` for a nested object and `The request body must be a JSON object.` for a form body. A sender that cannot sign or send `Authorization: Bearer` lands only on an unsigned route of a public app, where anyone can post | inbox "Signed webhook on a private app", booking "Check-in route", hub "Provider signature", "Undeclared fields", "Nested value", "Form-encoded body", "Unsigned route on a public app" |
| F10 | Files: public, private, per user; upload, publish, delete | W | `files_upload`, `files_update`, `files_delete` and `GET /_littlebox/files/<id>`; 2 MiB a file, 20 MiB a Deployment. A file is its uploader's or public, so no rule shares it with a role (`File not found.` to an editor), and a visitor cannot upload | gallery "Curator uploads", "Publish a draft", club "Files", community "Files by role" |
| F11 | Images embedded | W | `<img src="/_littlebox/files/<id>">` under `img-src 'self' blob: data:`. Images from other origins and embedded players do not load, because the policy names no other origin and `frame-src` falls back to `default-src 'self'`. No resizing | gallery "Chrome", club "Browser", shop "Payment provider in the page" |
| F12 | Search | C | `LIKE` with a nullable filter, a scan bounded by the 2-second statement deadline. Full-text search needs a virtual table, and both doors refuse it with `schema.sql may only create tables and indexes, but it contains CREATE VIRTUAL.` | dataset "Search", "Full-text search refused" |
| F13 | Pagination and sorting | W | `LIMIT min(coalesce(?, 20), 100) OFFSET coalesce(?, 0)`, a keyset `id > ?`, and a sort key that picks among `CASE` branches the statement names | dataset "Pagination and sorting", "Keyset pagination", inbox "Keyed read route" |
| F14 | Counters and aggregates | W | `count`, `sum`, `avg`, `max` with `GROUP BY` and a `LIMIT`; a total with no `GROUP BY` needs none | dataset "Aggregates", survey "Multi-answer form", inbox "In-app list" |
| F15 | Audit of who did what | C | A Signal that lists `actor` gives the owner who did what, and only for calls that changed a row. The app's members see only what the rows keep, such as `author_id`, because one statement writes one table, so there is no history table written with the change | community "Audit", inbox "Usage and Signals", booking "Validation Gate and Kernel" (two statements refused) |
| F16 | Signals and Usage for the owner | W | `signals_read`, `usage_read`, guard refusals under `refused` | inbox "Usage and Signals", habits "Signals and Usage", booking "Signals" |
| F17 | Email on an event | W | Since 2026-11-07, `"notify": { "to": "owner" \| "actor", "subject", "body" }` on a mutation ([ADR-0015](../adr/0016-a-mutation-may-notify-the-owner-or-the-actor.md)) queues one email when the statement changes a row: to the Project Owner, or to the caller once the account answered a link. A local signup proves no address, so the call succeeds and Usage counts `unverified-email`. Templates take `{input.<key>}` and `{actor.id}` only, and no address from the input is ever a recipient, so a third person such as an assignee or a follower gets nothing. Notices count apart from account email: 200 a day per app, 500 per creator, 50 per recipient from one app. The runtime door still has no mail tool | notify "Owner notice", "Actor notice needs a proven email", "Owner reads the outcome", "Refusals at both doors"; booking "Booking confirmation"; inbox "Critical page by email" |
| F18 | Scheduled or delayed work | C | A mutation with no input declares `"schedule": { "every": "15m" }`, from 5 minutes to 24 hours, or `{ "daily": "02:00" }` in UTC, and the Kernel's timer runs it once a period with nobody calling, records each run under Usage's `scheduled`, retries a failure at the next tick and skips an app in maintenance. The compromise: a run is one SQL statement, so a reminder, a digest or a summary is written into the app's tables and reaches nobody who has not opened the app, because nothing sends email on an event (F17). A run can be up to a minute late, and a period is never shorter than 5 minutes. A retry of an outbound call is N, because no action calls out | schedule "A run with nobody asking", "Only the timer", "A failing run is retried", "Maintenance"; booking "Release unclaimed bookings"; habits "Close the day"; platform "Declare a schedule on a mutation" |
| F19 | Realtime updates | C | The page polls a query with an `after` cursor, within 30 calls per 10 seconds per visitor address and 120 per signed-in user. Nothing is pushed: a `"subscribe"` key passes both doors and does nothing | community "Realtime by polling", platform "Declare a subscription on a query" |
| F20 | Payments | N | The page policy refuses a provider's script, API calls, checkout frame and a form that posts to it, and a provider's signed webhook answers 401 `invalid-signature`. The nearest alternative is a plain link to a hosted payment page, which publishes and navigates, and a program the owner runs that confirms the payment with an API key | shop "Payment provider in the page", "Holds that lapse" (`payments.confirm` by key), hub "Payment provider signature" |
| F21 | Third-party login | N | App accounts are email and password; the runtime tools are `auth_signup`, `auth_verify`, `auth_recover` and `auth_reset`. Nothing inside Littlebox comes close, because an outside login does not produce a Runtime Actor | platform "Runtime tools" |
| F22 | Custom domain | W | `domain_set` with `publish` gives a Deployment one hostname alias next to its generated hostname, which keeps answering. The owner proves control with a TXT record `_littlebox.<hostname>` holding a token, which the Kernel looks up itself; then the alias is the app's own origin for the runtime's `Origin` check, cookies, the policy, routes, the file path and the maintenance page. A name under the suffix, a name another Deployment serves and a record holding another value are refused. The operator's proxy terminates TLS and forwards `Host` unchanged ([ADR-0015](../adr/0015-a-deployment-answers-on-one-custom-domain.md)). Until 2026-11-07 another `Host` answered 404 and this cell was N | domain "TXT proof", "Serve from the alias", "A name another Deployment holds", "Package", "Maintenance page"; gallery "Custom domain"; platform "Unclaimed hostname" |
| F23 | Export and migration | W | `package_export` and an import into another Kernel, Eject, scheduled copies and restore | club "Export", "Import", habits "Eject and publish from outside", "Copy and restore" |
| F24 | Same app from the editor's agent and an external agent | C | The editor saves Managed sources and publishes through the Gate; an external agent publishes from the CLI. No replay drives the build agent itself: habits sends the editor's save and publish over MCP and runs the Gate on sql.js. The two doors also disagree on three declarations, listed under [Where the doors disagree](#where-the-doors-disagree) | habits "Managed publish", "Streak SQL within the LIMIT rules"; community "No email from the actor"; platform "Declare ..." |

## The matrix

| Feature | A | B | C | D | E | F | G | H | I | J |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| F1 Open signup | W | **W** | W | W | W | W | W | W | **W** | **W** |
| F2 Invited signup | W | W | **W** | **W** | **W** | W | W | W | W | W |
| F3 Allowlisted signup | W | W | W | W | W | W | W | W | W | W |
| F4 Verified signup | W | **W** | W | W | W | W | W | W | W | W |
| F5 Roles inside the app | C | C | **C** | **C** | C | C | C | C | C | **C** |
| F6 Public and private rows in one table | W | **W** | W | W | **W** | W | W | W | **W** | **W** |
| F7 Public read API | C | C | C | C | **C** | C | **C** | C | C | C |
| F8 Private write API with a key | W | W | W | W | W | W | **W** | **W** | W | W |
| F9 Signed webhooks in | C | C | C | C | C | C | C | **N** | C | C |
| F10 Files | W | W | **C** | **W** | W | **C** | W | W | W | **C** |
| F11 Images embedded | **W** | W | W | **W** | **C** | W | W | W | W | W |
| F12 Search | C | C | C | **C** | **C** | C | **C** | C | C | **C** |
| F13 Pagination and sorting | W | **W** | **W** | **W** | **W** | W | **W** | **W** | W | **W** |
| F14 Counters and aggregates | **W** | **W** | W | W | W | **W** | **W** | W | **W** | W |
| F15 Audit of who did what | C | C | **C** | C | C | C | C | **C** | C | C |
| F16 Signals and Usage for the owner | **W** | W | W | W | W | **W** | W | **W** | W | W |
| F17 Email on an event | **W** | W | **C** | **C** | W | **W** | W | **W** | **W** | **C** |
| F18 Scheduled or delayed work | C | **C** | **C** | C | **W** | C | W | **N** | **W** | W |
| F19 Realtime updates | C | C | **C** | C | C | C | C | C | **C** | **C** |
| F20 Payments | N | N | N | N | N | N | N | N | **N** | **N** |
| F21 Third-party login | N | N | N | N | N | N | N | N | N | N |
| F22 Custom domain | **W** | W | W | W | **W** | W | W | W | **W** | W |
| F23 Export and migration | **W** | **W** | **W** | **W** | **W** | **W** | **W** | **W** | **W** | **W** |
| F24 Same app from both agents | **C** | **C** | **C** | **C** | **C** | **C** | **C** | **C** | **C** | **C** |

A cell differs from its feature's verdict where the shape changes the answer:

- F9 in H is N. A hub's senders are outside services with their own signature schemes and payloads, and the hub probe shows each of them refused. The nearest alternative is an adapter outside Littlebox that verifies the sender and posts the declared fields to a signed route.
- F10 in C and J is C, because a file goes to its uploader or to everyone who may open the app, never to a role or to a publisher's followers. In F it is C, because a visitor cannot upload: `transfer_begin` needs a session, so a respondent creates an account before attaching a file.
- F11 in E is C, because a post cannot embed an image or a video from another site.
- F17 is C in C, D and J, because a notice reaches only the owner or the person who acted. A team tool cannot tell the assignee, a community its members, a two-sided app the followers; they see the change when they next open the app. Sending to account IDs a query returns is the form that would close it.
- F18 is W where the work is a change to the app's rows (E publishes a post at its time, G and J expire old rows, I releases a hold that lapsed, as booking does), C where the point of the work is to tell someone (A's launch notice, B's reminder, C's and D's digest, F's daily summary): the schedule writes it into the app, and it reaches a person only when they open the app, until F17 exists. It is N in H, because a retry of an outbound call needs an action that calls out. Until 2026-11-07 every one of these cells was C or N, because a `"schedule"` key passed both doors and ran nothing.

## Counts

The counts are taken from the 240 cells above, by counting each letter.

| | Works | Compromise | Not possible | Cells |
| --- | --- | --- | --- | --- |
| Every cell | 137 | 81 | 22 | 240 |
| Cells a shape cannot ship without (bold) | 50 | 33 | 4 | 87 |

On 2026-11-06 the counts were 116, 77 and 47 over every cell and 41, 30 and 16 over the bold ones. On 2026-11-07 the custom domain moved F22 to W, notices moved F17 to W or C, and scheduled work moved F18 to W or C, which gives the counts above.

| Shape | Needed | Not possible among them | Compromises among them |
| --- | --- | --- | --- |
| A Landing page | 7 |  | F24 |
| B Personal tracker | 8 |  | F18, F24 |
| C Team tool | 10 |  | F5, F10, F15, F17, F18, F19, F24 |
| D Community | 9 |  | F5, F12, F17, F24 |
| E Blog or docs | 10 |  | F7, F11, F12, F24 |
| F Form collector | 6 |  | F10, F24 |
| G Public dataset | 7 |  | F7, F12, F24 |
| H Integration hub | 9 | F9 provider webhooks, F18 retries | F15, F24 |
| I Shop or booking | 10 | F20 payments | F19, F24 |
| J Two-sided | 11 | F20 payments | F5, F10, F12, F17, F19, F24 |

A, B, C, D, E, F and G need nothing marked N. Before 2026-11-07 only G did. Every other shape still needs at least one feature that no primitive gives.

## Where the doors disagreed

The probes found three declarations the Validation Gate and the Kernel answered differently on 2026-11-06. The editor parity review of 2026-11-07 gave both doors one shared action-shape check (`kernel-rs/src/action-cases.json`), and the probes now assert the shared answers.

| Declaration | Both doors, since 2026-11-07 | Probe |
| --- | --- | --- |
| A binding `{ "source": "actor", "field": "email" }` | Refused before publishing with `Invalid binding 1 in members.join.` | community "No email from the actor" |
| An action of `"kind": "email"` | Refused with `Action notes.mail has an invalid kind. Use "query", "mutation" or "signal".` | platform "Declare an action of kind email" |
| A `"schedule"` key on a mutation, or `"subscribe"` on a query | Passed both doors and did nothing until 2026-11-07. A schedule is now a primitive that both doors parse from `kernel-rs/src/schedule-cases.json`, so `"0 8 * * *"` on `notes.add` is refused with `Action notes.add declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values.`, and `subscribe` is refused with a message that points at polling | platform "Declare a schedule on a mutation", "Declare a subscription on a query"; schedule "Declare a schedule" |

## What blocks the most shapes

Counting the bold N cells per feature gave five features with any on 2026-11-06, the five in [the showcase findings](./showcase-findings.md#what-the-matrix-adds), each with the smallest primitive that unblocks it. Two of them, email on an event and the custom domain, landed on 2026-11-07.

| Feature | Shapes it blocks | Shapes it degrades |
| --- | --- | --- |
| F17 Email on an event | | C, D, J |
| F18 Scheduled or delayed work | H | B, C |
| F20 Payments | I, J | |
| F9 Webhooks from outside services | H | |

Payments and outside webhooks need the same primitive, because a payment provider tells the app about a payment with a webhook in its own scheme. The next features down block nothing but degrade the most shapes: search (D, E, G, J), realtime (C, I, J) and roles (C, D, J).

## Probes

| Probe | Cells it decides | Checks |
| --- | --- | --- |
| [dataset](../../examples/probes/dataset/replay.ts) | F7, F8, F12, F13, F14 | 7 |
| [hub](../../examples/probes/hub/replay.ts) | F9, F20, the hub's program API | 9 |
| [shop](../../examples/probes/shop/replay.ts) | F11, F18, F20, inventory under concurrency | 3 |
| [community](../../examples/probes/community/replay.ts) | F5, F6, F10, F15, F19 | 6 |
| [survey](../../examples/probes/survey/replay.ts) | F14, one vote per person, a multi-answer form in one write | 2 |
| [platform](../../examples/probes/platform/replay.ts) | F17 (no mail tool on the runtime door), F19, F21, an unclaimed hostname, and the F18 and F19 forms both doors refuse | 6 |
| [domain](../../examples/probes/domain/replay.ts) | F22 | 6 |
| [notify](../../examples/probes/notify/replay.ts) | F17 | 4 |
| [schedule](../../examples/probes/schedule/replay.ts) | F18 | 6 |

## What this does not verify

No probe drives the editor or its build agent; F24 rests on habits' simulation of the editor's calls. The schedule probe and the booking and habits replays run their Kernels with `LITTLEBOX_SCHEDULE_MIN_SECONDS=2`, so every schedule runs every 2 seconds; that a `"daily"` job runs at its UTC time once a day, and resumes after a restart without running twice, is proven by the Kernel's unit tests in `kernel-rs/src/schedule.rs` on a clock they move, not by a replay. Verified signup is proven only by habits, on an installation that requires it; the probes run in local mode. The probes do not open Chrome, so F11 for images rests on the gallery and club Chrome passes, and the probes check the policy header instead. The product-fit examples, such as the launch page and the poll, are cited by the earlier assessment, not re-run here.
