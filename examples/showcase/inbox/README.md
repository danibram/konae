# Inbox

A private operations inbox. Outside services post events to it, and a team that joined by invitation reads and acknowledges them. A program the owner wrote reads the same events as JSON with an API key.

`replay.ts` publishes it the way an owner would and records what the Kernel allowed and what it refused.

## What it does

- Services post `{ source, id, severity, title, body? }` to `POST /hooks/events`, signed with the inbox's secret. A repeated `source` and `id` is stored once.
- A service that has something critical posts `{ source, id, title, body? }` to `POST /hooks/critical`, signed the same way. The event is stored as critical and the app's contact email gets a notice.
- A code host posts its issue events to `POST /hooks/github` and a payment provider its charge events to `POST /hooks/payments`, each as it sends them to anyone: its own nested payload, signed its own way. The routes name the provider, whose preset stands for its signature scheme, and read the event's ID and title from JSON paths.
- Signed-in app users see the newest 50 events, filter them by severity and by open or acknowledged, and acknowledge one. The list shows who acknowledged each event and when.
- A program reads `GET /api/events?after=<id>` with an API key that names that route. The route declares `"callers": ["key"]`, so nothing else reads it.
- The owner reads how often each route and action ran, how often the acknowledge guard refused, and the `event.received` and `event.acknowledged` Signals, the second naming who acknowledged.

## Files

| File | Role |
| --- | --- |
| `index.html`, `styles.css`, `app.js` | The page. Plain CSS with the design system's tokens copied as custom properties, light and dark from the system theme |
| `schema.sql` | `events` with a CHECK on severity and a unique `(source, external_id)`, and `members` |
| `actions.json` | Nine Server Actions, listed below |
| `replay.ts` | Starts a Kernel, publishes, runs every flow and asserts each answer |

| Action | Kind and access | What it does |
| --- | --- | --- |
| `events.receive` | mutation, anonymous, `POST /hooks/events` signed with `events_webhook` | Inserts an event, `ON CONFLICT DO NOTHING`; declares the `event.received` Signal with `source` and `severity` |
| `github.receive` | mutation, anonymous, `POST /hooks/github` with `"provider": "github"`, which stands for `hmac-body` over `X-Hub-Signature-256` with `sha256=` and hex, and `github_webhook` | Reads `id`, `title` and `repo` from `$.issue.node_id`, `$.issue.title` and `$.repository.full_name`, and inserts a `warning` event, `ON CONFLICT DO NOTHING` |
| `payments.receive` | mutation, anonymous, `POST /hooks/payments` with `"provider": "stripe"`, which stands for `timestamped` over `Stripe-Signature` with `t=`, `v1=` and a 300-second window, and `payments_webhook` | Reads `id`, `title` and `detail` from `$.id`, `$.type` and `$.data.object.failure_message`, and inserts a `critical` event, `ON CONFLICT DO NOTHING` |
| `events.feed` | query, anonymous, `GET /api/events` with `"callers": ["key"]` | Up to 100 events after an optional `after` id |
| `events.list` | query, authenticated | Nullable `severity` and `state` filters, `LIMIT 50` |
| `events.counts` | query, authenticated | Total and acknowledged counts, no LIMIT needed |
| `events.acknowledge` | mutation, authenticated | Sets the actor and time once; its guard refuses a second acknowledgement; declares the `event.acknowledged` Signal with `id` and `actor` |
| `members.hello` | mutation, authenticated | Stores the actor's ID with the email the page sends |
| `pages.critical` | mutation, anonymous, `POST /hooks/critical` signed with `events_webhook` | Inserts a critical event, `ON CONFLICT DO NOTHING`; declares `event.received` with `source`, and `"notify": { "to": "owner" }` with the subject `Critical: {input.title}` |

## Platform parts it uses

A private Deployment with signup by invitation, set on the first release with `initialSignupPolicy`. Signing secrets through `secret_set`. Five HTTP routes: two signed in Konae's scheme, two in a provider's scheme with `fields`, and one for API keys only. API keys with `api_keys_create`, `api_keys_list` and `api_keys_revoke`. Invitations through `app_invitations_create` and the development mailbox. `app_users_list` and `app_user_suspend`. Signals declared on three mutations, one naming the actor, and a guard, read with `signals_read` and `usage_read`. Nullable inputs. The Validation Gate and the Kernel's LIMIT rule. `konae contract` and `konae conform`.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
KONAE_SHOWCASE_PORT=19470 bun run examples/showcase/inbox/replay.ts
```

The port defaults to 19470. The script builds the Kernel offline, starts it on that port with a temporary data directory and the development mail transport, and creates a throwaway owner. It copies the sources to a scratch directory, so the repository stays clean. When it exits it stops the Kernel and deletes the directory. `KONAE_SHOWCASE_BROWSER=0` skips Chrome, and `KONAE_SHOWCASE_SCREENSHOTS=<dir>` keeps a light and a dark screenshot.

`konae conform` reports no failure and no skip against the private Deployment with Ana's session, a key and the three webhook secrets, and no failure against the signed variant with its four secrets and no session, where the checks that need a session are skipped.

To publish it yourself on an installation where you have an owner account, such as the one `bun run dev:local` starts:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/inbox --url http://localhost:4173 --manage-users
bun run konae -- init --dir /tmp/inbox --name Inbox
rm -rf /tmp/inbox/app && mkdir /tmp/inbox/app
cp examples/showcase/inbox/{index.html,styles.css,app.js,schema.sql,actions.json} /tmp/inbox/app/
```

Set `"visibility": "private"` and `"signupPolicy": { "mode": "invitation" }` in `/tmp/inbox/konae.json`, then run `bun run konae -- publish --dir /tmp/inbox`. The first run registers the project and is refused, naming the unset signing secrets `events_webhook`, `github_webhook` and `payments_webhook`. Set each over MCP with `secret_set`, with the `id` from `konae.json` as `projectId`, and publish again. A project published from a folder does not open in the editor, so invite the team with `app_invitations_create` and issue the feed's key with `api_keys_create`.

## What worked

- A release whose two secrets are unset is refused at `release_plan` with one message that names both. After `secret_set` the app publishes private with signup by invitation, and `secrets_list` returns names without values.
- With no session, `GET /` answers the Kernel's sign-in page and the runtime endpoint answers `Sign in to use Konae MCP.`, while `POST /hooks/events` stores a correctly signed event.
- The route refuses a replayed signature, a wrong secret, a body changed after signing and a missing signature with `invalid-signature`, and a `t` 400 seconds old or ahead with `stale-timestamp`. A retry signed with a new nonce, in the same second or later, answers 200 and stores nothing. The owner's editor cookie does not replace the signature.
- `POST /hooks/github` stores a GitHub-style issues event signed `sha256=<hex>` over its body, and `POST /hooks/payments` a Stripe-style `charge.failed` signed `t=...,v1=...` over `<t>.<body>`, both from their nested payloads, with `action`, `labels`, `sender`, `livemode` and the rest left out. A second copy of each answers 409 `replayed-signature`, even with a new `X-GitHub-Delivery`; a wrong secret and a changed byte answer 401 `invalid-signature`; a Stripe `t` 400 seconds old answers 401 `stale-timestamp`. The provider's retry with a new `t` answers 200 and stores nothing, and Usage counts only the deliveries that ran. `konae conform` signs its probes for both routes in their schemes.
- `GET /api/events` with no key answers 401 `authentication-required`. A key that names `events.feed` reads the rows and filters with `?after=`. A key that names only files gets 403. The runtime MCP endpoint refuses keys with 403 and says where they work. After `api_keys_revoke` the key gets `invalid-api-key`, and `api_keys_list` shows the revocation without the key.
- A second project without `callers` on the feed answers 401 `authentication-required` to a request with no key or session, since it is private. A third signs the feed: a signature for `?after=0` does not fit `?after=4`, and two signed GETs in one second differ by their nonce.
- Two invitations reach the development mailbox, scoped to the Deployment. Both people accept and land active. A used link answers 410, and a stranger cannot sign up.
- After `app_user_suspend` the suspended app user's open session is refused, the right password answers 403, the shell sends them to sign-in, and a new invitation to that address is refused because the account is suspended. The other app user keeps working.
- `events.list` returns the same rows for `{}` and for explicit nulls. Each filter narrows them, and an empty string is refused.
- The Validation Gate and `release_plan` refuse `events.list` without its LIMIT with the same sentence: `Action events.list reads rows and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.` The Gate puts `actions.json is not a valid server action program.` in front of it.
- `usage_read` counts each route call that ran under its method and path and counts none of the refused ones, except the two acknowledge guard refusals under `refused`. It also counts both signups. `signals_read` returns one `event.received` per stored event and one `event.acknowledged` per acknowledgement, naming who acknowledged.
- `POST /hooks/critical` stores a critical event and sends the notice `Critical: Payroll export failed` to the app's contact email. The sender's body carried `\r\nBcc: everyone@elsewhere.example`, which arrives as text on the same line, and nobody else gets mail. A signed retry answers `{ "changes": 0 }` and sends nothing. `usage_read` lists `pages.critical: queued` once and `mail_status` counts one delivered notice.
- `konae contract` writes the eight actions and names each route's signature scheme. `konae conform` probes the private Deployment, including the visitor refusals and the guard, with no check skipped.
- Chrome signs in through the Kernel's page, renders the list in light and dark, acknowledges an event from the page and reports no Content Security Policy violation.
- When an event was acknowledged in another session after the page drew it, the page's Acknowledge is refused with the guard and the sentence stays on screen after the list reloads. The page reads every failure through one `reason(error)` helper, the same in every showcase app but starter, so `rate-limited` and an unreachable Kernel show a sentence too.

## What the Kernel refused or could not do

| # | What I tried | What happened | Exact answer |
| --- | --- | --- | --- |
| 1 | Make `GET /api/events` readable only with a key | The route declares `"callers": ["key"]`. Without `callers`, a private app's unsigned route takes a key or a session; the replay proves it on a second project | 401 `authentication-required` with no session and no key |
| 2 | Sign `GET /api/events` with its secret | The MAC covers `t`, a nonce, the method, the path, the canonical query string and the body. The replay signs the feed on a third project | 401 `invalid-signature` for `?after=4` with the header signed for `?after=0`; 200 for a second GET in the same second with a new nonce |
| 3 | Run `konae conform` with `--secret events_feed=...` | Each signed probe carries its own nonce | No failure against the signed variant |
| 4 | A sender retries a POST at once | The retry signs again with a new nonce | 200 `{ "changes": 0, "lastInsertId": 0 }`, the event stored once |
| 5 | Tell a stale timestamp from a wrong secret | A `t` outside the window is checked before the MAC, so the answer says nothing about the secret | 401 `stale-timestamp`, with the right secret or a wrong one; a wrong secret with a fresh `t` gets 401 `invalid-signature` |
| 6 | Keep app users away from the webhook action | A signed route is its action's only door | `Action events.receive answers only on its HTTP route, POST /hooks/events.` |
| 7 | Record `event.acknowledged` only when an acknowledgement happens | The Signal sits on `events.acknowledge`, which records it only when a row changed, and its guard refuses event 999 and a second acknowledgement. `event.received` skips the retries that stored nothing | `Someone acknowledged this event first, or it does not exist.` |
| 8 | Record who acknowledged in the Signal | `actor` among a Signal's properties records the caller's ID | `event.acknowledged` carries `{ "id", "actor" }` with Ana's and Ben's IDs |
| 9 | Show who acknowledged by email | An `actor` binding yields the ID only. `members.hello` stores the email the page sends, so one app user can store another's email under their own ID | `{ "changes": 1 }` |
| 10 | Create a key that names nothing | Refused at creation, so there is no empty key to refuse on the route. The replay shows the 403 with a key that names only file visibilities | `Name at least one action or file visibility for the key.` |
| 11 | Give a key the in-app list | Only routed actions can be named | `Action events.list has no HTTP route, so a key cannot call it.` |
| 12 | Call the runtime MCP endpoint with a key | The endpoint refuses a key and says where it works | 403 `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.` |
| 13 | Publish before setting the secrets | The refusal names every unset secret and the actions that sign with them | `Actions events.receive, github.receive and payments.receive sign with the secrets events_webhook, github_webhook and payments_webhook, which are not set for this project. Set each with secret_set before publishing.` |
| 14 | Check that the invitation names the inviting owner | The development mailbox writes `to`, `purpose`, `scope`, `link` and `expiresAt`, and no subject or body | Not checkable with the development transport |
| 15 | Send a severity outside the CHECK | The sender receives SQLite's text, which shows the constraint | 400 `CHECK constraint failed: severity IN ('info', 'warning', 'critical')` |
| 16 | Tell a suspended app user why their session stopped | The runtime endpoint answers as if signed out. Only the login route says suspended | `Sign in to use Konae MCP.`, then 403 `This account is suspended.` |
| 17 | Take a code host's or a payment provider's webhook as it arrives | The routes declare the provider's signature scheme and `fields` ([ADR-0013](../../../docs/adr/0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md)) | 200 `{ "changes": 1 }` |

Other refusals the replay asserts, all as documented:

| Request | Answer |
| --- | --- |
| A stranger signs up | 403 `This app is not accepting new accounts for this email address.` |
| An accepted invitation link again | 410 `This link is invalid, expired or already used.` |
| An app user calls `app_invitations_create` or `usage_read` on the runtime endpoint | `Sign in to use Konae MCP.` and `Unknown tool or insufficient permission.` |
| A payload field the action does not declare, on the route without `fields` | 400 `Unexpected action input: priority` |
| A GitHub-style or Stripe-style delivery sent a second time | 409 `replayed-signature` |
| A GitHub-style or Stripe-style delivery signed with another secret, or changed after signing | 401 `invalid-signature` |
| A Stripe-style delivery with `t` 400 seconds old | 401 `stale-timestamp` |
| `?after=three` | 400 `after must be a number.` |
| `events.list` with `severity: ""` | `severity must be a non-empty string.` |

Findings 1, 2, 6 and 7 matter most for a private inbox, because each one keeps a route's signature guarding the data and the action behind the path as well as the path itself.
