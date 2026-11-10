# Gallery

A public photo portfolio for one photographer, Lena Varga (a made-up name). Visitors see the published photographs, open an about page with a contact form, and any program can read the newest work at `/feed.json`. Only one app account exists, the curator's. It uploads photographs, keeps drafts private, publishes them and reads the messages. Nobody else can create an account.

`replay.ts` publishes it the way an owner would, and this file records what the Kernel allowed and what it refused, with the exact messages the replay sees.

## Files

| File | What it is |
| --- | --- |
| `index.html` | The gallery grid, a lightbox, and the curator's studio (upload, drafts, messages), shown only when signed in |
| `about.html` | The about page and the contact form |
| `404.html` | The page every unknown path answers with, status 404 |
| `app.js` | One module for both pages; each part runs when its elements exist |
| `styles.css` | Plain CSS with the design system's neutral surfaces, one indigo accent, and a dark scheme from `prefers-color-scheme` |
| `schema.sql` | `studio` (the curator's user ID), `photos` (draft or published), `messages` |
| `actions.json` | Seven Server Actions, one HTTP route and one Signal |
| `replay.ts` | Starts its own Kernel, publishes, runs every flow below and asserts the answers |

## Platform parts it uses

- A Public Deployment whose first release sets `initialSignupPolicy` to Closed.
- An invitation, read from the development mailbox, to admit the curator's single account.
- `files_upload` with `visibility: "public"` for published photographs and `"private"` for drafts. Publishing a draft makes its own file public with `files_update` (`miniapp.files.update` in the page), and a stray upload goes with `files_delete`. Visitors read public files through `GET /_konae/files/<id>`, the path `miniapp.files.url` returns, which the grid embeds as `<img>` under `img-src 'self'`.
- Anonymous queries (`photos.published`), authenticated queries and mutations gated by a `studio` row the owner seeds with `database_execute`. `photos.add` and `photos.publish` declare a guard, so an account outside the studio is refused with the app's sentence.
- `contact.send`, an anonymous mutation with the Signal `contact.received` carrying only the `topic`.
- `feed.items`, an anonymous query bound to `GET /feed.json` with no signature.
- A multi-page Build Artifact from `konae build`: `index.html`, `about.html`, `404.html`, the `styles.css` and `app.js` they load, and the SDK as `runtime.js`.
- `signals_read`, `usage_read`, `konae contract` and `konae conform`.
- A custom domain, `gallery.studio.example`, set with `domain_set` and proved with a TXT record that a fake resolver on a local UDP port answers ([ADR-0014](../../../docs/adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md)). The replay reaches the alias with a `Host` header, as a proxy that forwards `Host` unchanged would.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
KONAE_SHOWCASE_PORT=19620 bun run examples/showcase/gallery/replay.ts
```

The default port is 19620. The replay builds the Kernel offline, starts it on that port with a random setup key, `KONAE_MAIL_TRANSPORT=development` and its data in a temporary directory, and removes both when it exits, whether it passed or not. `KONAE_SHOWCASE_BROWSER=0` skips Chrome. `KONAE_SHOWCASE_SCREENSHOTS=<dir>` saves the home page in light and dark, the about page and the studio. `KONAE_SHOWCASE_CONTRACT_OUT=<file>` keeps a copy of the generated `action-contract.md`.

To publish it yourself on an installation where you have an owner account, such as the one `bun run dev:local` starts:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/gallery --url http://localhost:4173 --manage-users
bun run konae -- init --dir /tmp/gallery --name Gallery
rm -rf /tmp/gallery/app && mkdir /tmp/gallery/app
cp examples/showcase/gallery/{index.html,about.html,404.html,app.js,styles.css,schema.sql,actions.json} /tmp/gallery/app/
```

Set `"visibility": "public"` and `"signupPolicy": { "mode": "closed" }` in `/tmp/gallery/konae.json`, then run `bun run konae -- publish --dir /tmp/gallery`. Invite the curator over MCP with `app_invitations_create`, and once they accept, add their account ID, which `app_users_list` shows, to the studio:

```sh
bun run konae -- sql --dir /tmp/gallery --prod "INSERT INTO studio (user_id) VALUES ('<account id>')"
```

The replay runs these flows and exits 0 when all of them pass:

1. `konae build` on this Capsule, then on a copy whose about page names a missing image.
2. A release whose `contact.send` ends in `RETURNING id`, which must be refused at planning.
3. The first release, public, signup Closed.
4. A stranger's signup, the owner's invitation under Closed, the curator's own signup, the accepted invitation, a second stranger.
5. The curator uploads three public photographs and two private drafts.
6. A visitor reads the gallery, the file list and a draft through every door.
7. The curator publishes a draft by making its file public, and deletes a stray upload.
8. `/feed.json` with GET, HEAD, POST and an undeclared query parameter.
9. 40 contact messages from one address, then one from another, then the Signals and the inbox.
10. Known and unknown paths on the multi-page artifact, and `HEAD /styles.css`.
11. Usage.
12. The custom domain: `domain_set` before the TXT record, with a record that holds another value, with a name under the suffix, then with the right token; the pages, the 404, `/feed.json`, a file and the runtime on the alias, and a page on the generated hostname calling the alias's runtime.
13. In Chrome, the home page in light and dark with every image loaded, the about page with a sent contact form, the signed-in studio, and in the studio a `fetch` of `miniapp.files.url(draft)`, a draft published and hidden again with `miniapp.files.update`, and an upload with an unknown visibility. Then the owner removes the curator from the `studio` table while the page is open: the page's upload is refused with the guard and its file deleted, and its publish is refused with the guard and the draft's file put back to private.
14. `konae contract`, then `konae conform` from the CLI with no credential, and the same checks in process.

## What worked

- Closed signup from the first request. The Kernel refuses every signup, the curator's own included, and the curator's account comes in through the owner's invitation with the policy still Closed (`acceptable: true`, then 200 on accept). The app has exactly one account (`app_users_list`) and the curator still signs in.
- Drafts stayed invisible by every door a visitor has: `photos.published` leaves them out, `files_list` lists only the public files, `files_read` answers `File not found.` and `GET /_konae/files/<id>` answers 404.
- Public photographs load in the page from `/_konae/files/<id>` as `image/png`, `inline`, with `Cross-Origin-Resource-Policy: same-origin`. Chrome reported no policy violation on any page, light or dark.
- Publishing a draft keeps its file. `files_update` made the draft's file public under the same ID, `photos.publish` marked the row published, and the curator held 5 files for 5 photographs. `photos.publish` takes only the photograph's ID, because the row already names its file. A second publish of the same photograph is refused with `Only the studio's curator can publish, and only a photograph that is still a draft.` A stray upload deleted with `files_delete` left `files_list`, answered `File not found.` to `files_read` and 404 at the file path, and stopped counting against the 20 MiB quota.
- In the studio, `fetch(await miniapp.files.url(draft))` answered 200 `image/png` with the draft's bytes under `connect-src 'self'`, and the draft previews load from the same path instead of from `miniapp.files.list()`, which downloads every file.
- `contact.send` works for a signed-out visitor, from the page and over MCP. The 31st call from one address in 10 seconds answered `rate-limited` with `Retry-After: 1`, and a second address went through at once. Each sent message recorded one `contact.received` Signal with only `{ topic }`, and the curator read every message through `studio.messages`.
- `GET /feed.json` answers any program with no signature and no key: 200, `application/json`, the four published photographs newest first, with image paths that load. `HEAD` answers 405 with `Allow: GET`. `POST` answers 405 with `Allow: GET` and `No route accepts POST on this path.`
- A mutation with `RETURNING` is refused at `release_plan`.
- `usage_read` counted `GET /feed.json` once; the refused 400 and 405 requests counted nothing.
- `konae build` publishes the three pages, `styles.css`, `app.js` and the SDK, and leaves out `schema.sql` and `actions.json`. The replay publishes that artifact as it is.
- `about.html` serves its own page at `/about.html` and `/about`, and the SDK and the contact form run on it with no policy violation.
- Every path the artifact does not hold answers 404 with `404.html`, never the home page. `HEAD /styles.css` answers 200 with `text/css`.
- The studio serves from its own domain. Until the Kernel's own lookup found the token in `_konae.gallery.studio.example`, the name answered 404; a record holding another value was refused with the value it held and the token it needed. Once proved, `gallery.studio.example` answered `/`, `/about`, the 404 page, `/feed.json` with the same four items and the first photograph as `image/png`, with the same policy, and `photos.published` over the runtime with the alias as `Origin` returned the four rows. The generated hostname kept answering.
- conform with no credential passed every check it could run.
- The page reads every failure through one `reason(error)` helper, the same in every showcase app but starter. A refusal shows its own sentence, `rate-limited` becomes "Too many requests just now. Wait a few seconds and try again.", and an unreachable Kernel becomes "The site could not reach its server. Check the connection and try again." A refused upload deletes its file, and the grid shows the failure when the first call fails.

## Unknown paths on a multi-page site

| Path | Status | Content type | Page served |
| --- | --- | --- | --- |
| `/` | 200 | text/html | Lena Varga · Photographs |
| `/index.html` | 200 | text/html | Lena Varga · Photographs |
| `/about.html` | 200 | text/html | About · Lena Varga |
| `/about` | 200 | text/html | About · Lena Varga |
| `/about/` | 404 | text/html | Not found · Lena Varga |
| `/missing-page` | 404 | text/html | Not found · Lena Varga |
| `/photos/2026/fog.jpg` | 404 | text/html | Not found · Lena Varga |
| `/styles.css` | 200 | text/css | |
| `/feed` | 404 | text/html | Not found · Lena Varga |

An exact file answers first, and a path with no extension answers with its `.html` page. Every other path answers 404 with the artifact's `404.html` (`kernel-rs/kernel/src/artifact.rs`). `/about/` asks for `about/index.html`, which the gallery does not have.

## What the Kernel refused or could not do

Rows with a status or a quoted message are answers the replay receives. Rows that start with "No way" come from reading the Kernel, the CLI and the docs for a way to do it and finding none.

| Attempt | Status | Exact answer |
| --- | --- | --- |
| `konae build` on a copy whose `about.html` names `/portrait.jpg`, which the Capsule does not hold | | `about.html references /portrait.jpg, but the Capsule holds no portrait.jpg.` |
| Publish `contact.send` with `... RETURNING id` | | `Action contact.send cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.` |
| A visitor signs up while signup is Closed | 403 | `This app is not accepting new accounts for this email address.` The same text for everyone under Closed, though it reads as if the address were the problem. |
| The invited curator accepts while signup is Closed | 200 | `acceptable: true`. An owner's invitation is acceptable under Closed (ADR-0003). |
| Mark the curator as the studio's owner | | No way. `auth_session` answers `role: "member"` for the only account; the app cannot tell the curator from any other account, so the owner inserts the curator's user ID into `studio` with `database_execute`. Until then `photos.add` is refused with its guard, `Only the studio's curator can add photographs.` |
| A visitor reads a draft over MCP (`files_read`) | | `File not found.` |
| A visitor reads a draft at `GET /_konae/files/<id>` | 404 | `{"error":"File not found."}` |
| A visitor uploads a file (`transfer_begin`) | | `authentication-required` |
| A visitor makes a draft public (`files_update`) | | `authentication-required` |
| `files_update` with `visibility: "friends"` | | `Value is outside the allowed choices.` |
| Delete a file that was already deleted (`files_delete`) | | `File not found.` |
| `miniapp.files.upload(file, { visibility: "organizers" })` in the page | | `File visibility must be private or public.` The SDK refuses before it sends anything. |
| `HEAD /feed.json` | 405 | `Allow: GET` and an empty body. HTTP drops the body of a HEAD answer, so `A route does not answer HEAD. Send GET.` from the docs never reaches the caller. |
| `POST /feed.json` | 405 | `{"error":"No route accepts POST on this path."}` with `Allow: GET` |
| `GET /feed.json?page=2` | 400 | `{"error":"Unexpected action input: page"}` |
| Shape `/feed.json` as a JSON Feed document (`version`, `title`, `items`) | | No way. A route always answers `{ "result": [rows] }`. |
| Absolute image URLs in `/feed.json` | | No way. The only kernel binding is `now`; the statement cannot read the Deployment's origin, so the feed carries `/_konae/files/<id>` paths. |
| Let a browser cache `/feed.json` | | No way. `Cache-Control: no-store` on every answer. A feed reader in another origin's browser reads it, because an open GET route answers `Access-Control-Allow-Origin: *` (ADR-0013). |
| Message 31 from one address within 10 seconds (`contact.send` over MCP) | 200 | `rate-limited`, as a JSON-RPC result with `isError: true` and `Retry-After: 1`. A route answers 429; the runtime endpoint answers 200. |
| A tighter limit for `contact.send` alone, such as 3 an hour | | No way. `actions.json` declares no rate limit; every anonymous action shares 30 calls per 10 seconds per address, which lets one address send about 260,000 messages a day. |
| `contact.send` with topic `spam` | | `CHECK constraint failed: topic IN ('commission', 'print', 'other')`. Inputs have type and length but no allowed values, so the CHECK in `schema.sql` is the only guard and its SQLite text reaches the visitor. |
| The `contact.received` Signal in `action-contract.md` | | The Markdown has a Signal section that records `contact.received` with `topic` after the statement changes a row. |
| The HEAD answer of `/feed.json` in the Action Contract | 405 | `feed.items` lists `A route does not answer HEAD. Send GET.` with 405 and says HTTP drops the body, and conform sends a HEAD and checks the status. |
| `domain_set gallery.studio.example` while `_konae.gallery.studio.example` holds another value | | `The TXT record at _konae.gallery.studio.example holds "konae-verify-copied-from-another-app", not this Deployment's token. Set it to <token> and call domain_set again.` |
| `domain_set gallery.localhost` | | `gallery.localhost is under this installation's app suffix localhost, where Konae names apps itself. Name a hostname outside it.` |
| A page on the generated hostname calls the runtime on `gallery.studio.example` | 403 | `Cross-origin or unrecognized-host MCP request rejected.` |

## Conform with no credential

`konae conform --url <app> --contract action-contract.json --inputs <file>` from the CLI, with no token, key or secret, fails nothing and skips 8 checks. The in-process run, one client address per call, gives the same result. `--inputs` sets `contact.send`'s `topic` to `other`, because the generated string would fail the CHECK.

Skipped, each with "Needs a signed-in person; pass --token.":

- `photos.add`: answers a valid call; refuses bad input
- `photos.publish`: answers a valid call; refuses bad input
- `studio.messages`: answers a valid call; refuses bad input
- `studio.photos`: answers a valid call; refuses bad input

The unauthenticated refusal of those four actions (`authentication-required`) passed. conform sends one `contact.send` with real input, so the curator's inbox gets a message from it.

## The Action Contract

Write it from a copy of the app folder:

```sh
bun run konae -- contract --dir /tmp/gallery --out /tmp/gallery-contract
```

`action-contract.md` there lists each action's input, output, Signal, callers, errors and limits. The replay writes it outside the source folder, and `KONAE_SHOWCASE_CONTRACT_OUT=<file>` keeps a copy.
