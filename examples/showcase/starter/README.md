# Starter

Starter is the typical small app in one Capsule, and the example to copy first. A visitor reads a public landing: service status, updates and downloads, also served as a JSON API. Anyone can create an account, and it opens only after they answer the emailed link. A member keeps private notes and files, sets a public avatar and publishes a file as a download. Team admins see a team area, post to the team and publish updates. The owner's account inside the app opens an admin area that promotes members. A program posts updates with an API key, and a monitoring service reports status through a signed webhook. The Project Owner reads the Signals.

Every rule lives in `actions.json`. The Kernel binds the caller's ID and the SQL decides who may read or change each row; `app.js` only hides what a role cannot use.

## Files

| File | What it holds |
| --- | --- |
| `index.html` | The whole page, with its own head so it can name its title and icon |
| `styles.css` | The design system's tokens from `src/design-system.css`, light and dark, as plain CSS |
| `app.js` | The page: `miniapp.auth`, `miniapp.actions` and `miniapp.files`, `textContent` only, no inline handlers |
| `favicon.svg` | The icon, as text so the managed path can hold it |
| `schema.sql`, `schema.ts` | Six tables; the SQL the Kernel runs and the Database Model the editor reads |
| `actions.json` | 27 actions: the only door to the database |
| `replay.ts` | Starts two Kernels, proves every claim below and removes them |
| `AGENT.md` | What an agent reads before adapting this app |

## Primitives and the features they carry

| Feature | Primitive | Where |
| --- | --- | --- |
| Public landing | Public Deployment, actions declared `anonymous` | `status.list`, `updates.list`, `downloads.list` |
| Public read API | An unsigned `GET` route on a public app, with pagination and search under a `LIMIT` | `updates.list` at `GET /api/updates?before=&q=` |
| Accounts | Signup Policy Open, `LITTLEBOX_ACCOUNT_VERIFICATION=required` | `littlebox.json` `signupPolicy`, the Kernel's sign-in page |
| Member area | `actor` bindings on `authenticated` actions | `me.*`, `notes.*`, `attachments.*` |
| Team area with roles | A role column and guarded mutations whose `WHERE` checks the caller's role | `team.*`, `updates.publish`, `members.promote` |
| Admin area for the owner | The same, for `role = 'owner'` | `members.list`, `members.demote`, `updates.remove`, `admin.stats` |
| The in-app owner | A key-only route, so only the Project Owner's key appoints it | `owners.appoint` at `POST /api/owners`, `"callers": ["key"]` |
| Private write API | A key-only route on a public app; the key's ID is the actor | `updates.post` at `POST /api/updates` |
| Signed webhook | A route with a signature and a signing secret set with `secret_set` | `status.report` at `POST /hooks/status`, header `X-Starter-Signature` |
| Avatar | `miniapp.files.upload(file, { visibility: "public" })`, `miniapp.files.url` | `me.avatar` |
| Private attachments | A private upload and a row that names it | `attachments.add`, `attachments.list` |
| Publish and delete | `miniapp.files.update`, `miniapp.files.delete`, then the row | `attachments.publish`, `attachments.remove` |
| Refusals in the page | `guard` on every role and ownership check; the page shows the message | `#notice` in `app.js` |
| Signals | `signal` on mutations, with `actor`; a `signal` action for downloads | `member.joined`, `note.added`, `update.posted`, `role.changed`, `status.changed`, `attachment.published`, `download.opened` |

## Run it

The replay builds the Kernel, starts one on `LITTLEBOX_SHOWCASE_PORT` and a second on the next port, both with required verification and the development mail transport, and deletes their data on exit. It needs Chrome for the last check.

```sh
LITTLEBOX_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
LITTLEBOX_SHOWCASE_SCREENSHOTS=/tmp/starter-shots LITTLEBOX_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
LITTLEBOX_SHOWCASE_BROWSER=0 LITTLEBOX_SHOWCASE_PORT=23450 bun run examples/showcase/starter/replay.ts
```

By hand, against a Kernel you started with `bun run dev:local`:

```sh
LITTLEBOX_EMAIL=you@example.com LITTLEBOX_PASSWORD=... bun run littlebox -- login --dir /tmp/starter --url http://localhost:4173
bun run littlebox -- init --dir /tmp/starter --name Starter --url http://localhost:4173
cp examples/showcase/starter/{index.html,styles.css,app.js,favicon.svg,schema.sql,schema.ts,actions.json} /tmp/starter/
# In /tmp/starter/littlebox.json set "visibility": "public" and "signupPolicy": { "mode": "open" },
# and delete the template's other files.
# Set the webhook's secret before the first publish: in the editor's Auth > Integrations view, or over MCP
# with an owner credential that holds publish: secret_set { "projectId": "<id in littlebox.json>",
# "name": "status_webhook", "value": "<16 to 512 printable characters>" }.
bun run littlebox -- publish --dir /tmp/starter
```

Then, as the Project Owner: sign up in the app with your own email, find your user ID with `app_users_list`, issue a key with `api_keys_create { actions: ["owners.appoint"] }`, and send `POST /api/owners {"userId": "<yours>"}` with it. Your account now opens the admin area, where you make members team admins.

## What the replay proves

The run of 2026-11-07 passed every check in about 75 seconds.

| Claim | How the replay shows it |
| --- | --- |
| Open signup with required verification | Signup answers 202 with no session cookie, only the signup hint; signing in before the link answers 401 with the hint in Ana's browser and the plain answer in another; the link sets the password and opens a session |
| A visitor reads the public rows and the public API only | `status.list`, `updates.list`, `downloads.list` and `GET /api/updates` answer; `notes.list`, `team.roster` and `me.join` answer `authentication-required`; both key-only routes answer 401; a private attachment answers 404 |
| A member reads and writes only their own rows | Ben reads only his note; deleting Ana's note gets the guard; a `userId` input is refused |
| A team admin promoted by a guarded mutation sees the team area | Ben's and Ana's `members.promote` get the guard; the owner's promotes Ana; then `team.roster` and `team.posts` answer her and still answer Ben with no rows |
| A key writes within its scope and is refused outside it | The Release bot key posts 12 updates whose `author_id` is the key's ID; on `/api/owners` and the webhook it gets 403 |
| A signed webhook updates a row | Two deliveries insert and update `api`; unsigned, replayed, stale and wrongly signed deliveries answer 401, 409, 401, 401 |
| Avatar public, attachment private, publish, delete and quota | See the Files check; the Release bot key, which names no file visibility, reads the public avatar as a visitor does and gets 404 for the private draft; 8 of Ben's 2 MiB files and Ana's 2 MiB attachment fill the 20 MiB quota, the next upload is refused, and deleting Ana's attachment lets the same upload through |
| Pagination and search within the LIMIT rules | `updates.list` pages 10 and 2 and searches; `notes.list` pages 20 and 5; the Gate and the Kernel refuse `notes.list` without its `LIMIT` with the same words |
| A guarded mutation shows its refusal in the page | Chrome, as Ana, renames her to "ben" and the page shows the guard |
| Signals carry the actor and only real changes | 47 Signals: each person's `member.joined` once though each joined twice; program updates name the key; `status.changed` has `actor` null and skips the identical retry; no refused call records one |
| The Action Contract names every column; conform passes with a session and with a key | 27 actions, no warning, no unnamed column; conform with a session, the key and the secret: 184 passed, 0 failed, 0 skipped, the member meeting each role's guard on the probes that omit a nullable input too; with the key and the secret: 65 passed, 0 failed, 40 skipped. Both counts include the HEAD probe on `GET /api/updates` |
| The app exports as a package and imports with every rule intact | On the second Kernel, passwords, rows, roles, guards, file visibility, the deleted file, Signals and open signup with verification hold; old keys answer `invalid-api-key`; the origin's last webhook delivery answers 409 |
| The same Capsule publishes from the CLI and the managed path | `littlebox login`, `init`, `build`, `publish` for one Deployment; `project_sources_save` and `release_plan`/`release_apply` for another; each is built in its own folder, and their page, script, styles and icon are byte for byte the same, with no bundler comment naming a folder |
| No console error under the policy | A visitor in light, Ana in dark and the owner in light load with no violation, console error or failed request |

## What the primitives refused or could not do

Each row is a limit the app met, with the exact answer. None was worked around.

| Primitive | Request | Answer | What it costs the app |
| --- | --- | --- | --- |
| Owner inside the app | `auth_session` for the app account the Project Owner opened with the owner's own email | `role "member"`. No binding or session field tells the app this account is its owner | The app cannot recognize its owner. Starter appoints one through a key-only route the owner calls once with curl, which a person who does not write code would not do. `docs/sdk.md` says the role is "member" or "owner", but only the optional hosted identity adapter returns "owner" |
| Actor binding | An action that wants the caller's email | No binding gives it: only `actor.id`, `kernel.now` and literals exist | Display names are typed by the member, and the owner matches emails to IDs with `app_users_list` |
| Required verification | Ana signs in before opening her verification link | Was `401 {"error":"Email or password is incorrect."}`. Fixed on 2026-11-07: from her browser, `401 {"error":"Email or password is incorrect. If you signed up with this email in this browser, open the link we emailed to choose your password first."}`; from another browser, the plain answer | None now. The hint depends on a cookie the signup request set, never on the account, so it tells nobody whether an address has an account |
| conform | `conform --token` as a member, on `updates.publish`, a guarded mutation with a nullable `body` | Fixed on 2026-11-07: `PASS updates.publish: answers without the nullable body. Refused with its guard, "Only team admins can publish an update.": the bindings accepted the call without body.` Before, the same probe failed with `Refused with "Only team admins can publish an update." where the contract makes body optional.` | None now. conform passes a guard refusal on the probe that omits a nullable input, as it does on the valid call |
| Action Contract | `admin.stats` returns six `COUNT(*)` columns, each aliased | Each is typed `unknown` with `nullable: null` | A second backend learns nothing about a count's type |
| Build Artifact | `littlebox build` of `app.js` | Fixed on 2026-11-07: the build drops esbuild's module path comments, and the CLI and managed copies, built in two folders, are byte for byte the same. Before, the published `app.js` started with `// ../../../../../../../../../private/var/folders/…/littlebox-showcase-starter-…/cli/app.js` | None now |
| Build Artifact | `index.html` as a fragment, the Capsule's default shape | Fixed on 2026-11-07: a fragment that opens with `<title>` and `<link rel="icon">` gets them in the head, so the browser does not ask for `/favicon.ico`. Before, they landed in the body, Chrome requested `/favicon.ico`, got 404 and logged `Failed to load resource: the server responded with a status of 404 (Not Found)` | Starter's `index.html` stays a whole document. A binary `favicon.ico` still does not survive the managed path, which holds text only; an SVG file or a `data:` SVG icon works from every door (`docs/mcp.md`, "What the build publishes") |
| Public read API | `GET /api/updates`, whose `SELECT` lists `id, title, body, source, published_at, author` | Each row's keys arrive as `author, body, id, published_at, source, title` | The order the SQL chose is lost on the route and through `actions_invoke` |
| API key files | The Release bot key, which names no file visibility, reads Ana's public avatar | Was `404 {"error":"File not found."}`. Fixed on 2026-11-07: 200 with the image, as for a visitor; the private draft still answers 404 | None now. A key reads a public file of a public Deployment as a visitor does, plus the visibilities it names |
| Files and rows | Publishing or deleting an attachment | Two calls: `files_update` or `files_delete`, then the row's action | No transaction joins them. If the second call fails, a public file is not listed, or a row names a deleted file. An action cannot see the Kernel's file records, so a row's `file_id` is whatever the page sent |
| Signals | The in-app owner's session calls `signals_read` | `Unknown tool or insufficient permission.` | The admin area cannot show Signals or Usage; the owner reads them outside the app |
| Visitor | `app_users_list` on the runtime endpoint, as anyone in the app | `Unknown tool or insufficient permission.` | The in-app owner cannot invite, suspend or list accounts; those stay with the Project Owner's tools |
| Signed route | `littlebox publish` before `secret_set` | `Action status.report signs with the secret status_webhook, which is not set for this project. Set it with secret_set before publishing.` | Each Kernel and each project needs `secret_set` before its first release: the CLI project, the managed project and the import |
| CLI | `littlebox init` | Writes `"visibility": "private"` and `{ "mode": "invitation" }`, plus the template's own files | A public, open app takes a hand edit of `littlebox.json`, and the template's own files have to be deleted before the Capsule goes in |
| Signed webhook | A signed delivery with state "on fire" | `400 {"error":"CHECK constraint failed: state IN ('operational', 'degraded', 'down')"}` | The sender sees SQLite's words; an action has no way to state an allowed list as an input rule |
| Package import | The origin's Release bot key on the destination | `401 invalid-api-key`; `api_keys_list` shows both keys with `revokedReason: "import"` | Every program needs a new key after a move, as documented |
| Package import | Ana's origin session cookie on the destination, calling `notes.list` | `authentication-required` | People sign in again, as documented |
| File quota | Ben's 9th 2 MiB file once the app holds 18 MiB | `This deployment has reached its 20 MiB file quota.` | No SDK call tells the app or the member how much room is left |

Refusals the rules require, all asserted in `replay.ts`: the guards' own sentences, `This API key may not call owners.appoint.`, `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.`, `Unexpected action input: author`, `Repeated query parameter: q`, `before must be a number.`, `replayed-signature`, `invalid-signature`, `stale-timestamp`, `File not found.` and `Only the person who uploaded this file may change or delete it.`

## Design notes

The CSS copies the tokens from `src/design-system.css`: lime accent, ink borders, square corners and offset shadows. `docs/design-system.md` still describes an indigo accent and rounded corners, so the doc and the tokens disagree. In dark mode the ink stroke is near black on a near-black canvas, so Starter draws strokes with `--ds-border` there, and it lightens `--ds-danger`, whose dark value is too dark to read as text. The webfont is left out because a Build Artifact loads nothing from another origin.
