# Starter

Starter is the typical small app in one Capsule, and the example to copy first. A visitor reads a public landing: service status, updates and downloads, also served as a JSON API. Anyone can create an account, and it opens only after they answer the emailed link. A member keeps private notes and files, sets a public avatar and publishes a file as a download. Team admins see a team area, post to the team and publish updates. The owner's account inside the app opens an admin area that promotes members. A program posts updates with an API key, and a monitoring service reports status through a signed webhook. A team admin tells the team's inbox app about a status change with a call the Kernel sends. The owner reads the Signals and the calls.

Every rule lives in `actions.json`. The Kernel binds the caller's ID and the SQL decides who may read or change each row; `app.js` only hides what a role cannot use.

## Files

| File | What it holds |
| --- | --- |
| `index.html` | The whole page, with its own head so it can name its title and icon |
| `styles.css` | The design system's tokens from `src/design-system.css`, light and dark, as plain CSS |
| `app.js` | The page: `miniapp.auth`, `miniapp.actions` and `miniapp.files`, `textContent` only, no inline handlers |
| `favicon.svg` | The icon, as text so the editor's saved sources can hold it |
| `schema.sql` | Seven tables; the SQL the Kernel runs |
| `actions.json` | 29 actions: the only door to the database, and the one call to another app |
| `replay.ts` | Starts two Kernels, proves every claim below and removes them |
| `AGENT.md` | What an agent reads before adapting this app |

## The platform parts behind each feature

| Feature | Platform part | Where |
| --- | --- | --- |
| Public landing | Public Deployment, actions declared `anonymous` | `status.list`, `updates.list`, `downloads.list` |
| Public read API | An unsigned `GET` route on a public app, with pagination and search under a `LIMIT` | `updates.list` at `GET /api/updates?before=&q=` |
| Accounts | Signup Policy Open, `KONAE_ACCOUNT_VERIFICATION=required` | `konae.json` `signupPolicy`, the Kernel's sign-in page |
| Member area | `actor` bindings on `authenticated` actions | `me.*`, `notes.*`, `attachments.*` |
| Team area with roles | A role column and guarded mutations whose `WHERE` checks the caller's role | `team.*`, `updates.publish` |
| Admin area for the owner | Guards that find the caller's `profiles` row with role `'owner'` through `{ "source": "actor", "field": "id" }` | `members.list`, `members.promote`, `members.demote`, `updates.remove`, `admin.stats` |
| The in-app owner | The owner sets one `profiles` row to `owner` with SQL, in the editor's Data panel or with `konae sql --prod`; the Kernel keeps no role | `me.join` adds everyone as member |
| Private write API | A key-only route on a public app; the key's ID is the actor | `updates.post` at `POST /api/updates` |
| Signed webhook | A route with a signature and a signing secret set with `secret_set` | `status.report` at `POST /hooks/status`, header `X-Starter-Signature` |
| Call to the team inbox | A call action the Kernel signs and sends to the [inbox](../inbox/README.md) showcase's webhook, with a `then` mutation that keeps the answer (ADR-0013) | `status.escalate`, `status.escalated`, `status.escalations`; the secret `inbox_events` set with `"use": "call"` |
| Avatar | `miniapp.files.upload(file, { visibility: "public" })`, `miniapp.files.url` | `me.avatar` |
| Private attachments | A private upload and a row that names it | `attachments.add`, `attachments.list` |
| Publish and delete | `miniapp.files.update`, `miniapp.files.delete`, then the row | `attachments.publish`, `attachments.remove` |
| Refusals in the page | `guard` on every role and ownership check; the page shows the message | `#notice` in `app.js` |
| Signals | `signal` on mutations, with `actor`; a `signal` action for downloads | `member.joined`, `note.added`, `update.posted`, `role.changed`, `status.changed`, `attachment.published`, `download.opened` |

## Run it

The replay builds the Kernel, starts one on `KONAE_SHOWCASE_PORT` and a second on the next port, both with required verification and the development mail transport, and deletes their data on exit. It needs Chrome for the last check.

```sh
KONAE_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
KONAE_SHOWCASE_SCREENSHOTS=/tmp/starter-shots KONAE_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
KONAE_SHOWCASE_BROWSER=0 KONAE_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
```

By hand, against an installation where you have an owner account, such as the one `bun run dev:local` starts:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/starter --url http://localhost:4173 --manage-users
bun run konae -- init --dir /tmp/starter --name Starter
rm -rf /tmp/starter/app && mkdir /tmp/starter/app
cp examples/showcase/starter/{index.html,styles.css,app.js,favicon.svg,schema.sql,actions.json} /tmp/starter/app/
```

1. In `/tmp/starter/konae.json` set `"visibility": "public"` and `"signupPolicy": { "mode": "open" }`.
2. `status.escalate` calls a team inbox. Publish the [inbox](../inbox/README.md) first, and put its URL with `/hooks/events` in place of `http://inbox.localhost/hooks/events` in `actions.json`.
3. Run `bun run konae -- publish --dir /tmp/starter`. The first run registers the project and is refused, naming an unset signing secret, `status_webhook` or the call's `inbox_events`.
4. Over MCP, with the `id` from `konae.json` as `projectId`, set `secret_set { "projectId": "<id>", "name": "status_webhook", "value": "<16 to 512 printable characters>" }` and `secret_set { "projectId": "<id>", "name": "inbox_events", "value": "<the inbox's events_webhook value>", "use": "call" }`.
5. Run `bun run konae -- publish --dir /tmp/starter` again.

Then join the app with your own app account, find its ID with `app_users_list` over MCP, and make it the app's owner:

```sh
bun run konae -- sql --dir /tmp/starter --prod "UPDATE profiles SET role = 'owner' WHERE user_id = '<account id>'"
```

A project published from a folder does not open in the editor. The app's admin area is where you then make members team admins.

## What the replay proves

| Claim | How the replay shows it |
| --- | --- |
| Open signup with required verification | Signup answers 202 with no session cookie, only the signup hint; signing in before the link answers 401 with the hint in Ana's browser and the plain answer in another; the link sets the password and opens a session |
| A visitor reads the public rows and the public API only | `status.list`, `updates.list`, `downloads.list` and `GET /api/updates` answer; `notes.list`, `team.roster` and `me.join` answer `authentication-required`; the key-only route answers 401; a private attachment answers 404 |
| A member reads and writes only their own rows | Ben reads only his note; deleting Ana's note gets the guard; a `userId` input is refused |
| A team admin promoted by a guarded mutation sees the team area | Ben's and Ana's `members.promote` get the guard; the owner's promotes Ana; then `team.roster` and `team.posts` answer her and still answer Ben with no rows |
| The owner's account is the app's owner without a key | Ana, Ben and the owner sign up and join as members; the owner's `database_execute` sql sets the owner's `profiles` row to `owner`, and the owner-only actions read that row against `actor.id` |
| A key writes within its scope and is refused outside it | The Release bot key posts 12 updates whose `author_id` is the key's ID; on the webhook it gets 403 |
| A signed webhook updates a row | Two deliveries insert and update `api`; unsigned, replayed, stale and wrongly signed deliveries answer 401, 409, 401, 401 |
| A status change reaches the team inbox through a call | The replay publishes the inbox showcase on the same Kernel. Ana's `status.escalate` answers `{ callId, queued: true }`; the Kernel signs the request with `inbox_events` and the inbox's route files one event with source `starter`, which `GET /api/events` with an inbox key returns. `status.escalated` stores status 200 and filed 1, and filed 0 for the same change again. A visitor's call answers `authentication-required`, writing the answer by hand is refused, and `calls_read` lists three calls delivered on the first attempt |
| Avatar public, attachment private, publish, delete and quota | See the Files check; the Release bot key, which names no file visibility, reads the public avatar as a visitor does and gets 404 for the private draft; 8 of Ben's 2 MiB files and Ana's 2 MiB attachment fill the 20 MiB quota, the next upload is refused, and deleting Ana's attachment lets the same upload through |
| Pagination and search within the LIMIT rules | `updates.list` pages 10 and 2 and searches; `notes.list` pages 20 and 5; the Gate and the Kernel refuse `notes.list` without its `LIMIT` with the same words |
| A guarded mutation shows its refusal in the page | Chrome, as Ana, renames her to "ben" and the page shows the guard |
| Signals carry the actor and only real changes | Each person's `member.joined` once though each joined twice; program updates name the key; `status.changed` has `actor` null and skips the identical retry; no refused call records one |
| The Action Contract names every column; conform passes with a session and with a key | 29 actions, no warning, no unnamed column; conform with a session, the key and the secret: 0 failed and 1 skipped, the skip being `status.escalate`'s valid call, which conform does not send; the member meets each role's guard on the probes that omit a nullable input too. With the key and the secret alone: 0 failed, and the checks that need a session skipped |
| The app exports and imports with every rule intact | On the second Kernel, passwords, rows, roles, guards, file visibility, the deleted file, Signals and open signup with verification hold; old keys answer `invalid-api-key`; the origin's last webhook delivery answers 409 |
| The same Capsule publishes from the CLI and the editor's path | `konae login`, `init`, `build`, `publish` for one Deployment; `project_sources_save` and `release_plan`/`release_apply` for another; each is built in its own folder, and their page, script, styles and icon are byte for byte the same, with no bundler comment naming a folder |
| No console error under the policy | A visitor in light, Ana in dark and the owner in light load with no violation, console error or failed request |

## What the Kernel refused or could not do

Each row is a limit the app met, with the exact answer. None was worked around.

| Part | Request | Answer | What it costs the app |
| --- | --- | --- | --- |
| Owner inside the app | `auth_session` for the app account the owner opened with their own email | An ordinary app account; the Kernel keeps no owner role (ADR-0001) | The owner sets a `profiles` row to `owner` with SQL, and the owner-only actions check that row against `actor.id` |
| Actor binding | An action that wants the caller's email | No binding gives it: only `actor.id`, `kernel.now` and literals exist | Display names are typed by the member, and the owner matches emails to IDs with `app_users_list` |
| Required verification | Ana signs in before opening her verification link | From her browser, `401 {"error":"Email or password is incorrect. If you signed up with this email in this browser, open your confirmation link to choose your password first. This installation sends no email, so the link is in the mailbox.jsonl file on its server."}`; from another browser, `Email or password is incorrect.` | None. The hint depends on a cookie the signup request set, never on the account, so it tells nobody whether an address has an account |
| conform | `conform --token` as a member, on `updates.publish`, a guarded mutation with a nullable `body` | `PASS updates.publish: answers without the nullable body. Refused with its guard, "Only team admins can publish an update.": the bindings accepted the call without body.` | None. conform passes a guard refusal on the probe that omits a nullable input, as it does on the valid call |
| Action Contract | `admin.stats` returns six `COUNT(*)` columns, each aliased | Each is typed `unknown` with `nullable: null` | A second backend learns nothing about a count's type |
| Build Artifact | `konae build` of `app.js` | The build drops esbuild's module path comments, and the CLI and editor copies, built in two folders, are byte for byte the same | None |
| Build Artifact | `index.html` as a fragment, the Capsule's default shape | A fragment that opens with `<title>` and `<link rel="icon">` gets them in the head, so the browser does not ask for `/favicon.ico` | Starter's `index.html` stays a whole document. A binary `favicon.ico` does not survive the editor's saved sources, which hold text only; an SVG file or a `data:` SVG icon works from every door (`docs/mcp.md`, "What the build publishes") |
| Public read API | `GET /api/updates`, whose `SELECT` lists `id, title, body, source, published_at, author` | Each row's keys arrive as `author, body, id, published_at, source, title` | The order the SQL chose is lost on the route and through `actions_invoke` |
| API key files | The Release bot key, which names no file visibility, reads Ana's public avatar | 200 with the image, as for a visitor; the private draft answers 404 | None. A key reads a public file of a public Deployment as a visitor does, plus the visibilities it names |
| Files and rows | Publishing or deleting an attachment | Two calls: `files_update` or `files_delete`, then the row's action | No transaction joins them. If the second call fails, a public file is not listed, or a row names a deleted file. An action cannot see the Kernel's file records, so a row's `file_id` is whatever the page sent |
| Signals | The in-app owner's session calls `signals_read` | `Unknown tool or insufficient permission.` | The admin area cannot show Signals or Usage; the owner reads them outside the app |
| Visitor | `app_users_list` on the runtime endpoint, as anyone in the app | `Unknown tool or insufficient permission.` | The in-app owner cannot invite, suspend or list accounts; those stay with the owner's tools |
| Call action | Ben, a member with no team role, invokes `status.escalate` with a title of his own | `{ callId, queued: true }`; the inbox files "Ben says api is down" under source `starter` | A call runs no SQL, so no `WHERE` can check the caller's role, and it cannot read the app's rows, so the page supplies the title and the event ID. Only the action's access gates it. The page shows the button to team admins only, which is a courtesy; the inbox's one event per source and ID is the only other check |
| Call action | Ana writes `status.escalated` herself | `Action status.escalated runs only after the call status.escalate answers.` | None. Only the Kernel records what the inbox answered |
| Signing secret | `secret_set` of the inbox's `events_webhook` value as the starter's `inbox_events` | `Another of your projects already uses this secret value. ...` without `"use": "call"`; accepted with it | One owner's two apps need the call-only use to share a value |
| Signed route | `konae publish` before `secret_set` | `Action status.report signs with the secret status_webhook, which is not set for this project. Set it with secret_set before publishing.` | Each Kernel and each project needs `secret_set` before its first release can pass: the CLI project, the editor's project and the import, for `status_webhook` and `inbox_events` both |
| CLI | `konae init` | Writes `"visibility": "private"` and `{ "mode": "invitation" }`, plus the template's own files | A public, open app takes a hand edit of `konae.json`, and the template's own files have to be deleted before the Capsule goes in |
| Signed webhook | A signed delivery with state "on fire" | `400 {"error":"CHECK constraint failed: state IN ('operational', 'degraded', 'down')"}` | The sender sees SQLite's words; an action has no way to state an allowed list as an input rule |
| Import | The origin's Release bot key on the destination | `401 invalid-api-key`; `api_keys_list` shows both keys with `revokedReason: "import"` | Every program needs a new key after a move, as documented |
| Import | Ana's origin session cookie on the destination, calling `notes.list` | `authentication-required` | People sign in again, as documented |
| File quota | Ben's 9th 2 MiB file once the app holds 18 MiB | `This deployment has reached its 20 MiB file quota.` | No SDK call tells the app or the member how much room is left |

Refusals the rules require, all asserted in `replay.ts`: the guards' own sentences, `This API key may not call status.report.`, `This link is invalid, expired or already used.` for a second use of the owner's entry link, `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.`, `Unexpected action input: author`, `Repeated query parameter: q`, `before must be a number.`, `replayed-signature`, `invalid-signature`, `stale-timestamp`, `File not found.` and `Only the person who uploaded this file may change or delete it.`

## Design notes

The CSS copies the tokens from `src/design-system.css`, with the lime accent, ink borders and offset shadows. In dark mode the ink stroke is near black on a near-black canvas, so Starter draws strokes with `--ds-border` there, and it lightens `--ds-danger`, whose dark value is too dark to read as text. The webfont is left out because a Build Artifact loads nothing from another origin.
