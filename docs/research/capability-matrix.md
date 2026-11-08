# Can the typical apps be built?

Date: 2026-11-07, checked against main at `89ba91d` with the notice integration fixes recorded in [the verification report](./integration-verification.md). This matrix compares ten app shapes with 25 features, 250 cells. Each verdict names the primitive and the replay that proves it. Probes under [examples/probes](../../examples/probes/README.md) cover cells without a showcase replay. Run them with `bun run examples/probes/replay-all.ts`.

The five additions of 2026-11-07 are custom domains ([ADR-0014](../adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md)), notices ([ADR-0012](../adr/0012-email-leaves-through-one-queue-to-addresses-the-app-has-proved.md)), scheduled mutations ([ADR-0011](../adr/0011-the-kernel-runs-a-declared-schedule.md)), provider signatures ([ADR-0013](../adr/0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md)) and outbound calls ([ADR-0013](../adr/0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md)). The call worker already retries failures with bounded backoff. F18 for the integration hub therefore counts as C, rather than N.

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
| F4 | Verified signup | W | `KONAE_ACCOUNT_VERIFICATION=required`. It is a switch for the whole installation, not one app | habits "Verified signup" |
| F5 | Roles inside the app | C | Roles are rows that each action's `WHERE` checks, refused with a guard. The owner enters through `app_owner_enter` with role `owner`; an action can check the `actor.role` binding to appoint other roles, as starter does. The `actor` binding yields an ID and never an email, so an admin promotes by ID or by a name the person typed, and an in-app role cannot invite, because invitations are owner tools that need `manage_users` | club "Roles", community "Roles inside the app" (`Only an editor can make someone a publisher.`), community "No email from the actor"; starter "Owner and team admin" |
| F6 | Public and private rows in one table | W | Each action projects and filters its own rows | booking "Public and staff views", gallery "Visitor view", community "Publishers and readers" |
| F7 | Public read API | C | An unsigned GET route on a public app. It always answers `{ "result": [rows] }` as JSON with `Cache-Control: no-store`. Since ADR-0013 it also answers `Access-Control-Allow-Origin: *`, so a page on another origin that lists the host in its `connects` reads it with a simple GET; an `OPTIONS` preflight still answers 405. There is no XML or RSS. A private app's routes take a key, a signature or a session, never a caller with none, so a public API means a public app | gallery "/feed.json", dataset "Read API from another origin", calls "connects" |
| F8 | Private write API with a key | W | A route with `"callers": ["key"]` and an API key that names the action | dataset "Private write API", habits "API key on an actor-bound route", booking "Check-in route" |
| F9 | Signed webhooks in | W | A signed route in Konae's scheme or in a provider's own: `hmac-body` (GitHub, Shopify), `timestamped` (Stripe) or `svix` (Resend). `fields` reads each input from a JSON path into the provider's body and leaves the other keys out. Each delivery is accepted once: a second copy answers 409 `replayed-signature`, a wrong secret or a changed byte 401 `invalid-signature`. Before 2026-11-07 a provider's scheme answered 401 and its body 400 `Unexpected action input: pusher` | inbox "Signed webhook on a private app", "Webhooks from outside services", booking "Check-in route", hub "Provider signature", "Svix sender", "Undeclared fields", "Program API", "Unknown scheme at both doors" |
| F10 | Files: public, private, per user; upload, publish, delete | W | `files_upload`, `files_update`, `files_delete` and `GET /_konae/files/<id>`; 2 MiB a file, 20 MiB a Deployment. A file is its uploader's or public, so no rule shares it with a role (`File not found.` to an editor), and a visitor cannot upload | gallery "Curator uploads", "Publish a draft", club "Files", community "Files by role" |
| F11 | Images embedded | W | `<img src="/_konae/files/<id>">` under `img-src 'self' blob: data:`. Images from other origins and embedded players do not load, because the policy names no other origin and `frame-src` falls back to `default-src 'self'`. No resizing | gallery "Chrome", club "Browser", shop "Payment provider in the page" |
| F12 | Search | C | `LIKE` with a nullable filter, a scan bounded by the 2-second statement deadline. Full-text search needs a virtual table, and both doors refuse it with `schema.sql may only create tables and indexes, but it contains CREATE VIRTUAL.` | dataset "Search", "Full-text search refused" |
| F13 | Pagination and sorting | W | `LIMIT min(coalesce(?, 20), 100) OFFSET coalesce(?, 0)`, a keyset `id > ?`, and a sort key that picks among `CASE` branches the statement names | dataset "Pagination and sorting", "Keyset pagination", inbox "Keyed read route" |
| F14 | Counters and aggregates | W | `count`, `sum`, `avg`, `max` with `GROUP BY` and a `LIMIT`; a total with no `GROUP BY` needs none | dataset "Aggregates", survey "Multi-answer form", inbox "In-app list" |
| F15 | Audit of who did what | C | A Signal that lists `actor` gives the owner who did what, and only for calls that changed a row. The app's members see only what the rows keep, such as `author_id`, because one statement writes one table, so there is no history table written with the change | community "Audit", inbox "Usage and Signals", booking "Validation Gate and Kernel" (two statements refused) |
| F16 | Signals and Usage for the owner | W | `signals_read`, `usage_read`, guard refusals under `refused` | inbox "Usage and Signals", habits "Signals and Usage", booking "Signals" |
| F17 | Email on an event | W | Since 2026-11-07, `"notify": { "to": "owner" \| "actor", "subject", "body" }` on a mutation ([ADR-0012](../adr/0012-email-leaves-through-one-queue-to-addresses-the-app-has-proved.md)) queues one email when the statement changes a row: to the Project Owner, or to the caller once the account answered a link. A local signup proves no address, so the call succeeds and Usage counts `unverified-email`. Templates take `{input.<key>}` and `{actor.id}` only, and no address from the input is ever a recipient, so a third person such as an assignee or a follower gets nothing. Notices count apart from account email: 200 a day per app, 500 per creator, 50 per recipient from one app. The runtime door still has no mail tool | notify "Owner notice", "Actor notice needs a proven email", "Owner reads the outcome", "Refusals at both doors"; booking "Booking confirmation"; inbox "Critical page by email" |
| F18 | Scheduled or delayed work | C | A mutation with no input declares `"schedule": { "every": "15m" }`, from 5 minutes to 24 hours, or `{ "daily": "02:00" }` in UTC, and the Kernel's timer runs it once a period with nobody calling, records each run under Usage's `scheduled`, retries a failure at the next tick and skips an app in maintenance. A run is one SQL statement. A scheduled notice reaches only the owner and has no input or changed-row values, so it cannot send a person the reminder or digest stored in their rows. A changed run can queue one generic owner notice through F17; a notice to actor records no-account. A run can be up to a minute late, and a period is never shorter than 5 minutes. Outbound calls retry independently through the call worker after 30 seconds, then 1, 2 and 4 minutes, up to five attempts. The app cannot declare a different retry calendar | schedule "A run with nobody asking", "Only the timer", "Scheduled owner notice", "A failing run is retried", "Maintenance"; booking "Release unclaimed bookings"; habits "Close the day"; platform "Declare a schedule on a mutation" |
| F19 | Realtime updates | C | The page polls a query with an `after` cursor, within 30 calls per 10 seconds per visitor address and 120 per signed-in user. Nothing is pushed. Both doors refuse a `"subscribe"` key and point at polling | community "Realtime by polling", platform "Declare a subscription on a query" |
| F20 | Payments | C | A call action creates a Checkout Session with a nested body (`line_items.0.price`) and `Idempotency-Key`, the then mutation stores the session id, the page links to the hosted checkout (or navigates to the provider URL when the answer carries one), and the provider's webhook on a `timestamped` route marks the order paid through a guarded mutation; a second payment for the order answers 403 with the guard. What is given up: checkout inside the page, since the page policy refuses the provider's script, API calls, checkout frame and a form that posts to it. A real Stripe host needs the same call shape against `api.stripe.com` and a secret set for calls; the shop probe proves it against a pay app on the same installation | shop "Checkout Session via a call", "Payment provider webhook", "Payment provider in the page"; hub "Payment provider signature" |
| F21 | Third-party login | N | App accounts are email and password; the runtime tools are `auth_signup`, `auth_verify`, `auth_recover` and `auth_reset`. Nothing inside Konae comes close, because an outside login does not produce a Runtime Actor | platform "Runtime tools" |
| F22 | Custom domain | W | `domain_set` with `publish` gives a Deployment one hostname alias next to its generated hostname, which keeps answering. The owner proves control with a TXT record `_konae.<hostname>` holding a token, which the Kernel looks up itself; then the alias is the app's own origin for the runtime's `Origin` check, cookies, the policy, routes, the file path and the maintenance page. A name under the suffix, a name another Deployment serves and a record holding another value are refused. The operator's proxy terminates TLS and forwards `Host` unchanged ([ADR-0014](../adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md)). Until 2026-11-07 another `Host` answered 404 and this cell was N | domain "TXT proof", "Serve from the alias", "A name another Deployment holds", "Package", "Maintenance page"; gallery "Custom domain"; platform "Unclaimed hostname" |
| F23 | Export and migration | W | `package_export` and an import into another Kernel, Eject, scheduled copies and restore | club "Export", "Import", habits "Eject and publish from outside", "Copy and restore" |
| F25 | Calls to another app or a service | C | A call action, which the Kernel queues, signs with a signing secret set for calls only or sends with an API key, makes at most 5 attempts over about 7.5 minutes, and answers to a `then` mutation that stores what came back, the other app's guard refusal included. A call to an app of this installation runs that app's route code in process. What is given up: a call runs no SQL, so no `WHERE` can check the caller's role and the page supplies the fields that are not constants (starter's `Call action` row: Ben, a member, is queued); the page gets `{ callId, queued: true }` and reads the answer later from a row; only https outside the installation, and never a private, loopback, link-local or metadata address (`Action tasks.escalate calls the IP address 169.254.169.254. Name a host instead.` at both doors, `address-refused` at run) | calls "A signed call to another app", "The callee's guard reaches the caller's then", "A call that sends an API key", "Limits", "Refusals at publish", "connects"; starter "Call to the team inbox"; `cargo test calls::tests` for retries and the run-time address check |
| F24 | Same app from the editor's agent and an external agent | C | The editor saves Managed sources and publishes through the Gate; an external agent publishes from the CLI. No replay drives the build agent itself: habits sends the editor's save and publish over MCP and runs the Gate on sql.js. The three declaration differences found by the probes are now closed, as listed under [Where the doors disagreed](#where-the-doors-disagreed) | habits "Managed publish", "Streak SQL within the LIMIT rules"; community "No email from the actor"; platform "Declare ..." |

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
| F9 Signed webhooks in | W | W | W | W | W | W | W | **C** | W | W |
| F10 Files | W | W | **C** | **W** | W | **C** | W | W | W | **C** |
| F11 Images embedded | **W** | W | W | **W** | **C** | W | W | W | W | W |
| F12 Search | C | C | C | **C** | **C** | C | **C** | C | C | **C** |
| F13 Pagination and sorting | W | **W** | **W** | **W** | **W** | W | **W** | **W** | W | **W** |
| F14 Counters and aggregates | **W** | **W** | W | W | W | **W** | **W** | W | **W** | W |
| F15 Audit of who did what | C | C | **C** | C | C | C | C | **C** | C | C |
| F16 Signals and Usage for the owner | **W** | W | W | W | W | **W** | W | **W** | W | W |
| F17 Email on an event | **W** | W | **C** | **C** | W | **W** | W | **W** | **W** | **C** |
| F18 Scheduled or delayed work | C | **C** | **C** | C | **W** | C | W | **C** | **W** | W |
| F19 Realtime updates | C | C | **C** | C | C | C | C | C | **C** | **C** |
| F20 Payments | C | C | C | C | C | C | C | C | **C** | **C** |
| F21 Third-party login | N | N | N | N | N | N | N | N | N | N |
| F22 Custom domain | **W** | W | W | W | **W** | W | W | W | **W** | W |
| F23 Export and migration | **W** | **W** | **W** | **W** | **W** | **W** | **W** | **W** | **W** | **W** |
| F24 Same app from both agents | **C** | **C** | **C** | **C** | **C** | **C** | **C** | **C** | **C** | **C** |
| F25 Calls to another app or a service | C | C | C | C | C | C | C | **C** | C | C |

A cell differs from its feature's verdict where the shape changes the answer:

- F9 in H is C. A hub's senders are outside services with their own signature schemes and payloads, and the four schemes take a code host's, a shop's, a payment provider's and a Svix sender's. A sender that posts a form-encoded body, as Slack's slash commands and Twilio do, still answers 400 `The request body must be a JSON object.`, a scheme with the timestamp in its own header, as Slack's, is not one of the four, and a delivery over 64 KiB answers 413. Such a sender still needs an adapter outside Konae. Before 2026-11-07 this cell was N.
- F10 in C and J is C, because a file goes to its uploader or to everyone who may open the app, never to a role or to a publisher's followers. In F it is C, because a visitor cannot upload: `transfer_begin` needs a session, so a respondent creates an account before attaching a file.
- F11 in E is C, because a post cannot embed an image or a video from another site.
- F17 is C in C, D and J, because a notice reaches only the owner or the person who acted. A team tool cannot tell the assignee, a community its members, a two-sided app the followers; they see the change when they next open the app. Sending to account IDs a query returns is the form that would close it. Since 2026-11-08 `"to": { "column": "<name>" }` sends to each verified account the changed rows name, with `{changes}` listing only that account's rows ([ADR-0012](../adr/0012-email-leaves-through-one-queue-to-addresses-the-app-has-proved.md)); these cells stay C until a probe of each shape regrades them.
- F18 is W where the work is a change to the app's rows (E publishes a post at its time, G and J expire old rows, I releases a hold that lapsed, as booking does), C where the point of the work is to tell someone (A's launch notice, B's reminder, C's and D's digest, F's daily summary): the schedule writes it into the app, and it reaches a person only when they open the app, unless the owner is its recipient. A changed run can notify the owner, but cannot address the people named in its rows or put those rows into a digest. Since 2026-11-08 it can do both through `{changes}`, `{kernel.now}` and `to: { column }` (ADR-0012, probe [notify-rows](../../examples/probes/notify-rows/replay.ts) on `konae dev`); the C cells await a regrade. It is C in H because the call worker retries outbound calls on its fixed backoff, without an app-defined retry calendar. Until 2026-11-07 every one of these cells was C or N, because a `"schedule"` key passed both doors and ran nothing.
- F25 is bold in H, whose shape names outbound calls. Every other shape can use it to tell another app what changed, as starter tells the inbox.

## Counts

The counts are taken from the 250 cells above, by counting each letter. Before the five additions they were 116, 77 and 47 of 240, and 41, 30 and 16 of 87 bold.

| | Works | Compromise | Not possible | Cells |
| --- | --- | --- | --- | --- |
| Every cell | 146 | 94 | 10 | 250 |
| Cells a shape cannot ship without (bold) | 50 | 38 | 0 | 88 |

On 2026-11-06 the counts were 116, 77 and 47 over 240 cells and 41, 30 and 16 over the 87 bold ones. On 2026-11-07 the custom domain moved F22 to W, notices moved F17 to W or C, scheduled work moved F18 to W or C, provider signature schemes moved F9 to W and F20 to C, and calls between apps added F25 as a row of 10 cells, which gives the counts above.

| Shape | Needed | Not possible among them | Compromises among them |
| --- | --- | --- | --- |
| A Landing page | 7 |  | F24 |
| B Personal tracker | 8 |  | F18, F24 |
| C Team tool | 10 |  | F5, F10, F15, F17, F18, F19, F24 |
| D Community | 9 |  | F5, F12, F17, F24 |
| E Blog or docs | 10 |  | F7, F11, F12, F24 |
| F Form collector | 6 |  | F10, F24 |
| G Public dataset | 7 |  | F7, F12, F24 |
| H Integration hub | 10 |  | F9, F15, F18, F24, F25 |
| I Shop or booking | 10 |  | F19, F20, F24 |
| J Two-sided | 11 |  | F5, F10, F12, F17, F19, F20, F24 |

All ten shapes have no required cell marked N. Before 2026-11-07 only the public dataset did. The compromises still matter, including the fixed retry calendar for the hub and the lack of a replay through the editor's build agent.

## Where the doors disagreed

The probes found three declarations the Validation Gate and the Kernel answered differently on 2026-11-06. The editor parity review of 2026-11-07 gave both doors one shared action-shape check (`kernel-rs/src/action-cases.json`), and the probes now assert the shared answers.

| Declaration | Both doors, since 2026-11-07 | Probe |
| --- | --- | --- |
| A binding `{ "source": "actor", "field": "email" }` | Refused before publishing with `Invalid binding 1 in members.join.` | community "No email from the actor" |
| An action of `"kind": "email"` | Refused with `Action notes.mail has an invalid kind. Use "query", "mutation", "signal" or "call".` | platform "Declare an action of kind email" |
| A `"schedule"` key on a mutation, or `"subscribe"` on a query | Passed both doors and did nothing until 2026-11-07. A schedule is now a primitive that both doors parse from `kernel-rs/src/schedule-cases.json`, so `"0 8 * * *"` on `notes.add` is refused with `Action notes.add declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values.`, and `subscribe` is refused with a message that points at polling | platform "Declare a schedule on a mutation", "Declare a subscription on a query"; schedule "Declare a schedule" |

## What blocks the most shapes

Five features accounted for the 16 required cells marked N on 2026-11-06. Those features and outbound calls landed on 2026-11-07. None now blocks a shape; this table lists the remaining compromises among required cells.

| Feature | Shapes it blocks | Shapes it degrades |
| --- | --- | --- |
| F9 Signed webhooks in |  | H |
| F17 Email on an event |  | C, D, J |
| F18 Scheduled or delayed work |  | B, C, H |
| F20 Payments |  | I, J |
| F22 Custom domain |  |  |
| F25 Calls to another app or a service |  | H |

Payments and webhooks from outside services blocked I, J and H until 2026-11-07. They needed one primitive, because a payment provider tells the app about a payment with a webhook in its own scheme, and [ADR-0013](../adr/0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md) added it. The next features down block nothing but degrade the most shapes: search (D, E, G, J), realtime (C, I, J) and roles (C, D, J).

## Probes

| Probe | Cells it decides | Checks |
| --- | --- | --- |
| [dataset](../../examples/probes/dataset/replay.ts) | F7, F8, F12, F13, F14 | 7 |
| [hub](../../examples/probes/hub/replay.ts) | F9, F20, the hub's program API, a scheme refused at both doors | 9 |
| [shop](../../examples/probes/shop/replay.ts) | F11, F18, F20, inventory under concurrency | 5 |
| [community](../../examples/probes/community/replay.ts) | F5, F6, F10, F15, F19 | 6 |
| [survey](../../examples/probes/survey/replay.ts) | F14, one vote per person, a multi-answer form in one write | 2 |
| [platform](../../examples/probes/platform/replay.ts) | F17 (no mail tool on the runtime door), F19, F21, an unclaimed hostname, and the F18 and F19 forms both doors refuse | 6 |
| [domain](../../examples/probes/domain/replay.ts) | F22 | 6 |
| [notify](../../examples/probes/notify/replay.ts) | F17 | 4 |
| [schedule](../../examples/probes/schedule/replay.ts) | F18, scheduled owner notices | 7 |
| [calls](../../examples/probes/calls/replay.ts) | F7, F25, the outbox, Usage on both apps, then notices | 11 |

## What this does not verify

No probe drives the editor or its build agent; F24 rests on habits' simulation of the editor's calls. Verified signup is proven only by habits, on an installation that requires it; the probes run in local mode. The probes do not open Chrome, so F11 for images rests on the gallery and club Chrome passes, and the probes check the policy header instead. The product-fit examples, such as the launch page and the poll, are cited by the earlier assessment, not re-run here. No probe sends a call to an https host outside the installation, because the probes run without a network: `send_outside` is proven against a plain-HTTP server on loopback in `cargo test`, which also proves the timeout, the 64 KiB cap, that a redirect is not followed, and that `localhost` resolves to an address a call may not reach.
