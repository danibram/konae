# Inbox

A private operations inbox. Outside services post events to it, and a team that joined by invitation reads and acknowledges them. A program the owner wrote reads the same events as JSON with an API key.

The app is a probe as much as an app. It uses Littlebox the way a creator would and writes down what the platform let it do and what it refused.

## What it does

- Services post `{ source, id, severity, title, body? }` to `POST /hooks/events`, signed with the inbox's secret. A repeated `source` and `id` is stored once.
- Signed-in members see the newest 50 events, filter them by severity and by open or acknowledged, and acknowledge one. The list shows who acknowledged each event and when.
- A program reads `GET /api/events?after=<id>` with an API key that names that route.
- The owner reads how often each route and action ran, and the `event.received` and `event.acknowledged` Signals.

## Files

| File | Role |
| --- | --- |
| `index.html`, `styles.css`, `app.js` | The page. Plain CSS with the design system's tokens copied as custom properties, light and dark from the system theme |
| `schema.sql` | `events` with a CHECK on severity and a unique `(source, external_id)`, and `members` |
| `actions.json` | Seven Server Actions, listed below |
| `replay.ts` | Starts a Kernel, publishes, runs every flow and asserts each answer |

| Action | Kind and access | What it does |
| --- | --- | --- |
| `events.receive` | mutation, anonymous, `POST /hooks/events` signed with `events_webhook` | Inserts an event, `ON CONFLICT DO NOTHING`; declares the `event.received` Signal with `source` and `severity` |
| `events.feed` | query, anonymous, `GET /api/events` signed with `events_feed` | Up to 100 events after an optional `after` id |
| `events.list` | query, authenticated | Nullable `severity` and `state` filters, `LIMIT 50` |
| `events.counts` | query, authenticated | Total and acknowledged counts, no LIMIT needed |
| `events.acknowledge` | mutation, authenticated | Sets the actor and time once |
| `events.acknowledged` | signal, authenticated | Records `event.acknowledged` with `id` and `severity` |
| `members.hello` | mutation, authenticated | Stores the actor's ID with the email the page sends |

## Primitives it uses

A private Deployment with signup by invitation, set on the first release with `initialSignupPolicy`. Signing secrets through `secret_set`. Two HTTP routes. API keys with `api_keys_create`, `api_keys_list` and `api_keys_revoke`. Invitations through `app_invitations_create` and the development mailbox. `app_users_list` and `app_user_suspend`. A Signal declared on a mutation and an action of kind `signal`, read with `signals_read` and `usage_read`. Nullable inputs. The Validation Gate and the Kernel's LIMIT rule. `littlebox contract` and `littlebox conform`.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
LITTLEBOX_SHOWCASE_PORT=19470 bun run examples/showcase/inbox/replay.ts
```

The port defaults to 19470. The script builds the Kernel offline, starts it on that port with a temporary data directory and the development mail transport, and creates a throwaway owner. It copies the sources to a scratch directory, so the repository stays clean. When it exits it stops the Kernel and deletes the directory. `LITTLEBOX_SHOWCASE_BROWSER=0` skips Chrome, and `LITTLEBOX_SHOWCASE_SCREENSHOTS=<dir>` keeps a light and a dark screenshot.

The last run passed all 12 checks and exited 0. `littlebox conform` reported `72 passed, 0 failed, 0 skipped.` against the private Deployment.

## What worked

- A release whose two secrets are unset is refused at `release_plan` with one message that names both. After `secret_set` the app publishes private with signup by invitation, and `secrets_list` returns names without values.
- With no session, `GET /` answers the Kernel's sign-in page and the runtime endpoint answers `Sign in to use Littlebox MCP.`, while `POST /hooks/events` stores a correctly signed event.
- The route refuses a replayed signature, a wrong secret, a body changed after signing, a `t` 400 seconds old or ahead, and a missing signature. The owner's editor cookie does not replace the signature.
- A key that names `events.feed` reads the rows and filters with `?after=`. A key that names only files gets 403. The runtime MCP endpoint does not accept keys. After `api_keys_revoke` the key gets `invalid-api-key`, and `api_keys_list` shows the revocation without the key.
- Two invitations reach the development mailbox, scoped to the Deployment. Both people accept and land active. A used link answers 410, and a stranger cannot sign up.
- After `app_user_suspend` the suspended member's open session is refused, the right password answers 403, the shell sends the member to sign-in, and a new invitation to that address answers `suspended`. The other member keeps working.
- `events.list` returns the same rows for `{}` and for explicit nulls. Each filter narrows them, and an empty string is refused.
- The Validation Gate and `release_plan` refuse `events.list` without its LIMIT with the same sentence: `Action events.list reads rows and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.` The Gate puts `actions.json is not a valid server action program.` in front of it.
- `usage_read` counts each route call that ran under its method and path and counts none of the refused ones. It also counts both signups and the signal action. `signals_read` returns both Signals with their properties.
- `littlebox contract` writes the seven actions. `littlebox conform` probes the private Deployment, including the visitor refusals, with no check skipped.
- Chrome signs in through the Kernel's page, renders the list in light and dark, acknowledges an event from the page and reports no Content Security Policy violation.

## What the primitives refused or could not do

| # | What I tried | What happened | Exact answer |
| --- | --- | --- | --- |
| 1 | Make `GET /api/events` readable only with a key | No declaration says "key required". An unsigned GET route on a private app answers anyone with the rows, while the shell still asks for sign-in. The replay proves it on a second project. The inbox signs its GET route with a secret nobody sends, so only a key, which replaces the signature, gets through | 200 `{ "result": [ ... ] }` with no session and no key |
| 2 | Sign `GET /api/events` with its secret | The MAC covers `"<t>."` and the empty body, not the query string. Two signed GETs in the same second carry the same signature whatever their query, so the second is a replay. The Kernel checks the MAC before the replay table, so the 409 rather than a 401 shows the signature verified for the other query. A captured signature fits any query | 409 `replayed-signature` for `?after=4` after `?after=1` with the same `t` |
| 3 | Run `littlebox conform` with `--secret events_feed=...` | Conform's route probes reuse a second, so one or two of its signed probes answer as replays and the run fails. Reading the route with `--key` passes | For example `FAIL events.feed: refuses an undeclared input on the route. Answered 409 "replayed-signature" where the contract says 400 "Unexpected action input: conformProbe".` Another run also failed `events.receive: refuses a body that is not JSON` the same way |
| 4 | A sender retries a POST at once | The retry has the same body and, inside the same second, the same signature, so it is refused. The sender has to sign the retry with a new `t` | 409 `replayed-signature` |
| 5 | Tell a stale timestamp from a wrong secret | Both answer the same, so a sender with a skewed clock cannot see why it fails | 401 `invalid-signature` |
| 6 | Keep members away from the webhook action | A signed-in member calls `events.receive` through `actions_invoke` with no signature and the event is stored. The signature binds the route, not the action, and no declaration makes an action route-only | `{ "changes": 1 }` |
| 7 | Record `event.acknowledged` only when an acknowledgement happens | The signal action is separate from the mutation, so a member records it for event 999, which nobody acknowledged. The Signal on `events.receive` is recorded for a retry that stored nothing (`changes: 0`) too | `{ "recorded": true }` |
| 8 | Record who acknowledged in the Signal | Signal properties come only from input bindings, and an `actor` binding has no key to name. The Signal can carry only what the page sends. The replay does not publish the refused declaration; the documented refusal is in the next column | `Action <name> names signal property <key>, which is not one of its input bindings.` |
| 9 | Show who acknowledged by email | An `actor` binding yields the ID only. `members.hello` stores the email the page sends, and a member stored another member's email under their own ID | `{ "changes": 1 }` |
| 10 | Create a key that names nothing | Refused at creation, so there is no empty key to refuse on the route. The replay shows the 403 with a key that names only file visibilities | `Name at least one action or file visibility for the key.` |
| 11 | Give a key the in-app list | Only routed actions can be named | `Action events.list has no HTTP route, so a key cannot call it.` |
| 12 | Call the runtime MCP endpoint with a key | The endpoint takes app sessions only | 401 `{"error":"Sign in to use Littlebox MCP."}` |
| 13 | Publish before setting the two secrets | Fixed on 2026-11-06. The refusal first named only the first unset secret, so a project with two needed two failed plans to learn both. It now names every unset secret and the actions that sign with them, and the replay asserts it | `Actions events.feed and events.receive sign with the secrets events_feed and events_webhook, which are not set for this project. Set each with secret_set before publishing.` |
| 14 | Publish with `bun run littlebox -- publish` as invitation only | Fixed on 2026-11-06. The CLI's release payload carried no `initialSignupPolicy`, so a private app started Closed and the owner needed a `manage_users` credential for `signup_policy_set`. `littlebox.json` now names `project.manifest.signupPolicy`, and the club and booking replays publish with it. This replay still calls `release_plan` over MCP | No error; read from `scripts/littlebox.ts` |
| 15 | Check that the invitation names the inviting owner | The development mailbox writes `to`, `purpose`, `scope`, `link` and `expiresAt`, and no subject or body | Not checkable with the development transport |
| 16 | Send a severity outside the CHECK | The sender receives SQLite's text, which shows the constraint | 400 `CHECK constraint failed: severity IN ('info', 'warning', 'critical')` |
| 17 | Tell a suspended member why their session stopped | The runtime endpoint answers as if signed out. Only the login route says suspended | `Sign in to use Littlebox MCP.`, then 403 `This account is suspended.` |

Other refusals the replay asserts, all as documented:

| Request | Answer |
| --- | --- |
| A stranger signs up | 403 `This app is not accepting new accounts for this email address.` |
| An accepted invitation link again | 410 `This link is invalid, expired or already used.` |
| A member calls `app_invitations_create` or `usage_read` on the runtime endpoint | `Sign in to use Littlebox MCP.` and `Unknown tool or insufficient permission.` |
| A payload field the action does not declare | 400 `Unexpected action input: priority` |
| `?after=three` | 400 `after must be a number.` |
| `events.list` with `severity: ""` | `severity must be a non-empty string.` |

Findings 1, 2, 6 and 7 are the ones that matter for a private inbox. Read together, they say a route's signature protects the path and not the data or the action behind it.
