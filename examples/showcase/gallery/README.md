# Gallery

A public photo portfolio for one photographer, Lena Varga (a made-up name). Visitors see the published photographs, open an about page with a contact form, and any program can read the newest work at `/feed.json`. Only one app account exists, the curator's. It uploads photographs, keeps drafts private, publishes them and reads the messages. Nobody else can create an account.

This is a product probe. The app uses Littlebox's primitives as a creator would, and this file records what they allowed and what they refused, with the exact messages the replay saw.

## Files

| File | What it is |
| --- | --- |
| `index.html` | The gallery grid, a lightbox, and the curator's studio (upload, drafts, messages), shown only when signed in |
| `about.html` | The about page and the contact form |
| `app.js` | One module for both pages; each part runs when its elements exist |
| `styles.css` | Plain CSS with the design system's neutral surfaces, one indigo accent, and a dark scheme from `prefers-color-scheme` |
| `schema.sql` | `studio` (the curator's user ID), `photos` (draft or published), `messages` |
| `actions.json` | Seven Server Actions, one HTTP route and one Signal |
| `replay.ts` | Starts its own Kernel, publishes, runs every flow below and asserts the answers |

## Primitives used

- A Public Deployment whose first release sets `initialSignupPolicy` to Closed.
- An Invitation, read from the development mailbox, to admit the curator's single account.
- `files_upload` with `visibility: "public"` for published photographs and the default private visibility for drafts. Visitors read public files through `GET /_littlebox/files/<id>`, which the grid embeds as `<img>` under `img-src 'self'`.
- Anonymous queries (`photos.published`), authenticated queries and mutations gated by a `studio` row the owner seeds with `database_execute`.
- `contact.send`, an anonymous mutation with the Signal `contact.received` carrying only the `topic`.
- `feed.items`, an anonymous query bound to `GET /feed.json` with no signature.
- A multi-file Build Artifact: `index.html`, `about.html`, `styles.css`, `app.js` and the SDK as `runtime.js`.
- `signals_read`, `usage_read`, `littlebox contract` and `littlebox conform`.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
LITTLEBOX_SHOWCASE_PORT=19620 bun run examples/showcase/gallery/replay.ts
```

The default port is 19620. The replay builds the Kernel offline, starts it on that port with a random setup key, `LITTLEBOX_MAIL_TRANSPORT=development` and its data in a temporary directory, and removes both when it exits, whether it passed or not. `LITTLEBOX_SHOWCASE_BROWSER=0` skips Chrome. `LITTLEBOX_SHOWCASE_SCREENSHOTS=<dir>` saves the home page in light and dark, the about page and the studio. `LITTLEBOX_SHOWCASE_CONTRACT_OUT=<file>` keeps a copy of the generated `action-contract.md`.

It runs these flows and exits 0 when all of them pass:

1. `littlebox build` on this Capsule, to see what the CLI makes of a multi-page app.
2. A release whose `contact.send` ends in `RETURNING id`, which must be refused at planning.
3. The first release, public, signup Closed.
4. A stranger's signup, the owner's invitation under Closed, the curator's own signup, the accepted invitation, a second stranger.
5. The curator uploads three public photographs and two private drafts.
6. A visitor reads the gallery, the file list and a draft through every door.
7. The curator publishes a draft.
8. `/feed.json` with GET, HEAD, POST and an undeclared query parameter.
9. 40 contact messages from one address, then one from another, then the Signals and the inbox.
10. Known and unknown paths on the multi-page artifact.
11. Usage.
12. In Chrome, the home page in light and dark with every image loaded, the about page with a sent contact form, the signed-in studio, and an attempt to read an SDK Blob URL.
13. `littlebox contract`, then `littlebox conform` from the CLI with no credential, and the same checks in process.

## What worked

- Closed signup from the first request. The Kernel refused every signup, the curator's own included, and the curator's account came in through the owner's Invitation with the policy still Closed (`acceptable: true`, then 200 on accept). The app has exactly one account (`app_users_list`) and the curator still signs in.
- Drafts stayed invisible by every door a visitor has: `photos.published` leaves them out, `files_list` lists only the public files, `files_read` answers `File not found.` and `GET /_littlebox/files/<id>` answers 404.
- Public photographs load in the page from `/_littlebox/files/<id>` as `image/png`, `inline`, with `Cross-Origin-Resource-Policy: same-origin`. Chrome reported no policy violation on any page, light or dark.
- `contact.send` works for a signed-out visitor, from the page and over MCP. The 31st call from one address in 10 seconds answered `rate-limited` with `Retry-After: 1`, and a second address went through at once. Each sent message recorded one `contact.received` Signal with only `{ topic }`, and the curator read every message through `studio.messages`.
- `GET /feed.json` answers any program with no signature and no key: 200, `application/json`, the four published photographs newest first, with image paths that load. `HEAD` answers 405 with `Allow: GET`. `POST` answers 405 with `Allow: GET` and `No route accepts POST on this path.`
- A mutation with `RETURNING` is refused at `release_plan`.
- `usage_read` counted `GET /feed.json` once; the refused 400 and 405 requests counted nothing.
- `about.html` serves its own page, and the SDK and the contact form run on it.
- conform with no credential passed every check it could run.

## Unknown paths on a multi-page site

| Path | Status | Content type | Page served |
| --- | --- | --- | --- |
| `/` | 200 | text/html | Lena Varga · Photographs |
| `/index.html` | 200 | text/html | Lena Varga · Photographs |
| `/about.html` | 200 | text/html | About · Lena Varga |
| `/about` | 200 | text/html | Lena Varga · Photographs |
| `/about/` | 200 | text/html | Lena Varga · Photographs |
| `/missing-page` | 200 | text/html | Lena Varga · Photographs |
| `/photos/2026/fog.jpg` | 200 | text/html | Lena Varga · Photographs |
| `/styles.css` | 200 | text/css | |
| `/feed` | 200 | text/html | Lena Varga · Photographs |

Every GET path that is neither an artifact file nor a route serves `index.html` with 200 (`serve_key` in `kernel-rs/src/main.rs`). Extensionless paths do not resolve to `<path>.html` or `<path>/index.html`. There is no 404 for a page, so a mistyped link and a broken image both get the home page's HTML.

## What the primitives refused or could not do

Rows with a status or a quoted message are answers the replay received. Rows that start with "No primitive" or "No option" come from reading the Kernel, the CLI and the docs for a way to do it and finding none.

| Attempt | Status | Exact answer |
| --- | --- | --- |
| `littlebox build` or `publish` on a Capsule with `about.html` | | No error. The artifact holds only `index.html`, built from `index.html` as a body fragment with `styles.css` and `app.js` inlined, so `about.html` is dropped and `/about.html` would serve the home page. The replay publishes the multi-file artifact over MCP, as the product-fit `docs` example does. |
| `littlebox publish` with a first signup policy | | Fixed on 2026-11-06: `littlebox.json` names `project.manifest.signupPolicy` and `littlebox publish` sends it on the first release (the club and booking replays use it). This replay still publishes over MCP, because the CLI drops `about.html`. |
| Publish `contact.send` with `... RETURNING id` | | `Action contact.send cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.` |
| A visitor signs up while signup is Closed | 403 | `This app is not accepting new accounts for this email address.` The same text for everyone under Closed, though it reads as if the address were the problem. |
| The invited curator accepts while signup is Closed | 200 | Fixed on 2026-11-06 (ADR-0011, note on Closed). The replay first recorded 403 `This app is not accepting new accounts for this email address.` with `acceptable: false`. An owner's invitation is now acceptable under Closed, and the replay asserts `acceptable: true` and the 200. |
| Create the curator's account on an app that is Closed from its first release | | Fixed on 2026-11-06. The owner invites the curator and the policy stays Closed; the curator's own signup still answers 403. |
| Mark the curator as the studio owner | | No primitive. `auth_session` answers `role: "member"` for the only account; the app cannot tell the curator from any other account, so the owner inserts the curator's user ID into `studio` with `database_execute`. Until then `photos.add` answers `{ changes: 0 }`. |
| A visitor reads a draft over MCP (`files_read`) | | `File not found.` |
| A visitor reads a draft at `GET /_littlebox/files/<id>` | 404 | `{"error":"File not found."}` |
| A visitor uploads a file (`transfer_begin`) | | `authentication-required` |
| Make a private draft's file public, or delete the private copy | | No primitive. The runtime file tools are `files_list`, `files_read` and `files_upload`. Publishing a draft uploads the bytes again as public; the private copy stays and counts against the 20 MiB quota. |
| Read a draft's SDK Blob URL with `fetch()` to upload it again as public | | `TypeError: Failed to fetch`; Chrome: `Connecting to 'blob:http://gallery-….localhost:19620/…' violates the following Content Security Policy directive: "connect-src 'self'". The action has been blocked.` The studio asks the curator to pick the file again instead. |
| Show draft previews in the studio | | Works. `miniapp.files.list()` downloads every file the curator may read, public ones included, on each call. |
| `HEAD /feed.json` | 405 | `Allow: GET` and an empty body. HTTP drops the body of a HEAD answer, so `A route does not answer HEAD. Send GET.` from the docs never reaches the caller. |
| `POST /feed.json` | 405 | `{"error":"No route accepts POST on this path."}` with `Allow: GET` |
| `GET /feed.json?page=2` | 400 | `{"error":"Unexpected action input: page"}` |
| Shape `/feed.json` as a JSON Feed document (`version`, `title`, `items`) | | No primitive. A route always answers `{ "result": [rows] }`. |
| Absolute image URLs in `/feed.json` | | No primitive. The only kernel binding is `now`; the statement cannot read the Deployment's origin, so the feed carries `/_littlebox/files/<id>` paths. |
| Let a feed reader in another origin's browser read `/feed.json`, or cache it | | No primitive. No `Access-Control-Allow-Origin` header, and `Cache-Control: no-store` on every answer. |
| Message 31 from one address within 10 seconds (`contact.send` over MCP) | 200 | `rate-limited`, as a JSON-RPC result with `isError: true` and `Retry-After: 1`. A route answers 429; the runtime endpoint answers 200. |
| A tighter limit for `contact.send` alone, such as 3 an hour | | No primitive. `actions.json` declares no rate limit; every anonymous action shares 30 calls per 10 seconds per address, which lets one address send about 260,000 messages a day. |
| `contact.send` with topic `spam` | | `CHECK constraint failed: topic IN ('commission', 'print', 'other')`. Inputs have type and length but no allowed values, so the CHECK in `schema.sql` is the only guard and its SQLite text reaches the visitor. |
| `GET /missing-page`, expecting 404 | 200 | No error. The home page is served. |
| `GET /about`, extensionless | 200 | No error. The home page is served, not `about.html`. |
| Ship a `404.html` for unknown paths | | No primitive. The Kernel falls back to `index.html` and never reads `404.html`. |
| The `contact.received` Signal in `action-contract.md` | | Missing. `action-contract.json` has `"signal": {"name":"contact.received","properties":["topic"]}`; the Markdown rendering never mentions it. |
| The HEAD answer of `/feed.json` in the Action Contract | | Missing. The contract lists no 405 for `feed.items`, and conform does not probe HEAD. |

## Conform with no credential

`littlebox conform --url <app> --contract action-contract.json --inputs <file>` from the CLI, with no token, key or secret: **31 passed, 0 failed, 8 skipped.** The in-process run, one client address per call, gives the same counts. `--inputs` sets `contact.send`'s `topic` to `other`, because the generated string would fail the CHECK.

Skipped, each with "Needs a signed-in person; pass --token.":

- `photos.add`: answers a valid call; refuses bad input
- `photos.publish`: answers a valid call; refuses bad input
- `studio.messages`: answers a valid call; refuses bad input
- `studio.photos`: answers a valid call; refuses bad input

The unauthenticated refusal of those four actions (`authentication-required`) passed. conform sends one `contact.send` with real input, so the curator's inbox gets a message from it.

## The Action Contract

`bun run littlebox -- contract --dir examples/showcase/gallery --out <dir>` writes it; the replay keeps it out of the source folder. The full Markdown is below.

<details>
<summary>action-contract.md, 7 actions</summary>

### Action Contract

Generated from actions.json and schema.sql. action-contract.json is the machine-readable form and action-contract.schema.json its schema. Do not edit either; change the Capsule and generate them again.

7 actions. Every action runs on the runtime endpoint `/_littlebox/mcp` as the `actions_invoke` tool, which `miniapp.actions.call(name, input)` calls.

- A query answers an array of row objects keyed by the column names listed. A mutation answers { changes, lastInsertId }.
- Column types are SQLite affinities read from schema.sql. SQLite does not enforce them, so a value can still arrive as another JSON type when it was stored as one.
- Nullability follows NOT NULL and INTEGER PRIMARY KEY in schema.sql. A column read through the right side of a LEFT JOIN is nullable.
- A SQLite error, such as a UNIQUE constraint, is returned with SQLite's own text.
- On a private Deployment every runtime call needs a session, including anonymous actions. An HTTP route answers by the action's access level whatever the visibility.

| Action | Kind | Access | HTTP |
| --- | --- | --- | --- |
| `contact.send` | mutation | anonymous |  |
| `feed.items` | query | anonymous | `GET /feed.json` |
| `photos.add` | mutation | authenticated |  |
| `photos.publish` | mutation | authenticated |  |
| `photos.published` | query | anonymous |  |
| `studio.messages` | query | authenticated |  |
| `studio.photos` | query | authenticated |  |

#### contact.send

A mutation declared anonymous.

```sql
INSERT INTO messages (name, email, topic, body, received_at) VALUES (?1, ?2, ?3, ?4, ?5)
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `name` | string | yes | 120 UTF-16 code units |
| `email` | string | yes | 254 UTF-16 code units |
| `topic` | string | yes | 20 UTF-16 code units |
| `message` | string | yes | 4000 UTF-16 code units |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: visitor (none; a public Deployment only; actor none); person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: name` |  | runtime | a required input is omitted |
| `name must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `name is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: email` |  | runtime | a required input is omitted |
| `email must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `email is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: topic` |  | runtime | a required input is omitted |
| `topic must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `topic is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: message` |  | runtime | a required input is omitted |
| `message must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `message is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### feed.items

A query declared anonymous.

```sql
SELECT id, title, caption, '/_littlebox/files/' || file_id AS image, published_at FROM photos WHERE status = 'published' ORDER BY published_at DESC, id DESC LIMIT 20
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `id` | integer | no | photos.id |
| `title` | text | no | photos.title |
| `caption` | text | yes | photos.caption |
| `image` | unknown | unknown |  |
| `published_at` | integer | yes | photos.published_at |

##### Callers

Runtime endpoint: visitor (none; a public Deployment only; actor none); person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

HTTP route `GET /feed.json` on the Deployment host. Input is the query string, each value typed by its input's declared type. It answers `{ "result": ... }` with the same output, or `{ "error": "<message>" }` with a status.

Callers: visitor (none; actor none); service (Authorization: Bearer <API key that names this action>; it replaces the signature; actor key id); person (the app session cookie, from the app's own origin; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` | 400 | both | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` | 400 | both | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |
| `invalid-api-key` | 401 | http | Authorization carries an lbk_ key that is unknown or revoked (401) |
| `This API key may not call feed.items.` | 403 | http | the key does not name the route's action (403) |
| `Repeated query parameter: <key>` | 400 | http | a GET names one input twice (400) |
| `rate-limited` | 429 | http | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 20 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### photos.add

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
INSERT INTO photos (file_id, title, caption, status, created_at, published_at) SELECT ?2, ?3, ?4, ?5, ?6, CASE WHEN ?5 = 'published' THEN ?6 END WHERE EXISTS (SELECT 1 FROM studio WHERE user_id = ?1)
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `file_id` | string | yes | 80 UTF-16 code units |
| `title` | string | yes | 120 UTF-16 code units |
| `caption` | string or null | no | 600 UTF-16 code units |
| `status` | string | yes | 12 UTF-16 code units |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: file_id` |  | runtime | a required input is omitted |
| `file_id must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `file_id is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: title` |  | runtime | a required input is omitted |
| `title must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `title is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `caption must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `caption is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: status` |  | runtime | a required input is omitted |
| `status must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `status is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### photos.publish

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
UPDATE photos SET status = 'published', file_id = ?3, published_at = ?4 WHERE id = ?2 AND status = 'draft' AND EXISTS (SELECT 1 FROM studio WHERE user_id = ?1)
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `id` | number | yes |  |
| `file_id` | string | yes | 80 UTF-16 code units |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: id` |  | runtime | a required input is omitted |
| `id must be a number.` |  | runtime | a number input is not a JSON number |
| `Missing action input: file_id` |  | runtime | a required input is omitted |
| `file_id must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `file_id is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### photos.published

A query declared anonymous.

```sql
SELECT id, file_id, title, caption, published_at FROM photos WHERE status = 'published' ORDER BY published_at DESC, id DESC LIMIT 60
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `id` | integer | no | photos.id |
| `file_id` | text | no | photos.file_id |
| `title` | text | no | photos.title |
| `caption` | text | yes | photos.caption |
| `published_at` | integer | yes | photos.published_at |

##### Callers

Runtime endpoint: visitor (none; a public Deployment only; actor none); person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 60 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### studio.messages

A query declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
SELECT id, name, email, topic, body, received_at FROM messages WHERE EXISTS (SELECT 1 FROM studio WHERE user_id = ?1) ORDER BY received_at DESC, id DESC LIMIT 100
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `id` | integer | no | messages.id |
| `name` | text | no | messages.name |
| `email` | text | no | messages.email |
| `topic` | text | no | messages.topic |
| `body` | text | no | messages.body |
| `received_at` | integer | no | messages.received_at |

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 100 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### studio.photos

A query declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
SELECT id, file_id, title, caption, status, created_at, published_at FROM photos WHERE EXISTS (SELECT 1 FROM studio WHERE user_id = ?1) ORDER BY created_at DESC, id DESC LIMIT 200
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `id` | integer | no | photos.id |
| `file_id` | text | no | photos.file_id |
| `title` | text | no | photos.title |
| `caption` | text | yes | photos.caption |
| `status` | text | no | photos.status |
| `created_at` | integer | no | photos.created_at |
| `published_at` | integer | yes | photos.published_at |

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 200 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### Any action

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Server action not declared.` |  | runtime | actions.json declares no action with that name |
| `Arguments must be an object.` |  | runtime | input is not an object, e.g. null |
| `No route accepts POST on this path.` | 405 | http | a POST reaches a path no action binds (405) |

</details>
