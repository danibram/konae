# Club

Club is a members-only book club. The Project Owner invites people by email. Members propose books with a cover image that every member sees, and keep private notes that only they can open. An organizer, whom the Project Owner promotes, schedules the next meeting, marks a book finished and removes books. The app runs as a private Deployment with the Invitation only Signup Policy, and it moves to a second Kernel as a Package with its accounts, rows, files and rules.

## Files

| File | What it holds |
| --- | --- |
| `index.html`, `styles.css`, `app.js` | The page. `app.js` calls only `miniapp.auth`, `miniapp.actions`, `miniapp.files.upload` and `miniapp.files.url`, plus `fetch` and `<img>` on `/_konae/files/<id>`. `styles.css` copies the design system's tokens as plain CSS and follows the system theme. |
| `schema.sql` | `members` (user ID, display name, role), `books` (title, author, cover file ID, status, meeting date) and `notes` (owner, book, file ID). |
| `actions.json` | Ten authenticated Server Actions. Every query names a `LIMIT`. `books.schedule`, `books.finish` and `books.remove` check `role = 'organizer'` in their `WHERE`, and `books.schedule` also checks that no other book is being read. `books.propose` and `notes.add` check that the caller joined. Each of these five declares a guard. |
| `replay.ts` | Starts two Kernels, runs every flow below, asserts the results and the refusals, and removes both Kernels and their data when it exits. |

## Platform parts it uses

- A Private Deployment, published by `publishLocal` in `scripts/konae.ts`, the function behind `konae publish`, as Invitation only from `konae.json`'s `signupPolicy`, then a republish that names another policy.
- `app_invitations_create`, the development mailbox and `POST /_konae/auth/accept`.
- `miniapp.files.upload` with and without `visibility: "public"`, and `GET /_konae/files/<id>` with the app session cookie, as an `<img>` source for covers and a `fetch` for notes, under the app's Content Security Policy.
- `api_keys_create` with `files: ["public"]`, used with `Authorization: Bearer`.
- Roles in SQL. The Project Owner promotes an organizer with `database_execute` after finding her user ID with `app_users_list`.
- `POST /_konae/auth/recover` and `POST /_konae/auth/reset`.
- `package_export`, then `McpClient.upload` with `component`, and a first release with `initialPackage` on a second Kernel under a second Project Owner.
- `konae contract` and `konae conform`.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
KONAE_SHOWCASE_PORT=19472 bun run examples/showcase/club/replay.ts
```

The origin Kernel listens on `KONAE_SHOWCASE_PORT` (default 19470) and the destination on the next port. Both use `KONAE_MAIL_TRANSPORT=development`, and the replay reads links from each Kernel's `mailbox.jsonl`. Set `KONAE_SHOWCASE_SCREENSHOTS` to a directory to keep a light and a dark screenshot, and `KONAE_SHOWCASE_BROWSER=0` to skip Chrome. The replay prints one `PASS` line per flow and a JSON report of every refusal it observed, and exits 0 when all of them pass.

## What worked

- **Invitation only.** A visitor gets the Kernel's sign-in page with "Only invited people can create an account" and no signup form. A signup from an uninvited address answers 403. Three invitations reached the mailbox, and each link set a password and returned a session.
- **Private and public files.** Ben cannot read Ana's private note through `files_read` or the file read path. He reads her public cover both ways, byte for byte. `files_list` gives him the two covers and his own note. A visitor gets 401 from the file read path for a public cover, a private note and a missing ID alike.
- **Images under the policy.** Chrome signed Ben in through the Kernel's form, rendered both 120 by 180 covers from `/_konae/files/<id>`, fetched his note's text from the path `miniapp.files.url` returned and reported no policy violation in light or dark.
- **API key.** A key with `files: ["public"]` reads a cover with `image/png` and the original bytes. It gets 404 for a private note and the runtime endpoint refuses it with 403 and says where keys work.
- **Roles.** Ana's `books.schedule` changes one row. Ben's `books.schedule`, `books.remove` and `books.finish` change none and are refused with each action's guard, and the page shows "Only an organizer can schedule a meeting, for a book on the shortlist while no other book is being read." Usage counts Ana's call under `action` and Ben's two schedules under `refused`.
- **One book at a time.** Fixed on 2026-11-07. `books.schedule` checked only that the book was not finished, so an organizer could schedule a second book while one was current; the page showed one of them and hid the other. With The Dispossessed current, Ana's schedule of Solaris is now refused with the schedule guard, and the list holds one current book.
- **Joining first.** Fixed on 2026-11-07. Cy's `books.propose` before she joined is refused with `Join the club before proposing a book.`; before, it answered `{ changes: 0 }` and the page judged that itself.
- **Notes outlive their book.** Fixed on 2026-11-07. `notes.mine` joined `books` with an inner join, so when an organizer removed a book every member's notes on it vanished from their list while the files stayed. It now lists them under "A removed book", and `notes.add` on a removed book is refused with `Join the club first, and pick a book that is still on the list.`
- **No orphaned uploads.** Fixed on 2026-11-07. The cover and note forms upload first and then call the action. When the action refuses, the page deletes the upload, so a refused note leaves no file counting against the quota; the replay submits a note for a removed book from Ben's page and counts his files before and after. The forms keep their button disabled while a call runs, so a double press stores one book or note.
- **Errors on the page.** The page reads every failure through one `reason(error)` helper, the same in all five showcase apps, so `rate-limited` and an unreachable Kernel show a sentence, and opening a note while offline says so instead of leaving "Opening…". A meeting is scheduled two weeks from the organizer's own calendar day, not the UTC date.
- **Recovery.** The emailed link reset Cy's password. Her earlier session then answers "Sign in to use Konae MCP.", the old password fails, and the link works once.
- **Package.** The Package held database, identity, files and usage. On the destination, Ana, Ben and Cy sign in with their passwords (Cy with the new one). Ana is still organizer, the rows match, the cover and the note have the same bytes, Ben still cannot read Ana's note, and Ben's organizer mutation is still refused with its guard. Ana's origin session and the origin's API key both answer 401 there. The Signup Policy arrived as Invitation only, and the invitation history arrived as three accepted invitations.
- **Contract.** `konae conform` with Ben's session reported `70 passed, 0 failed, 2 skipped.` on the origin and again on the destination on 2026-11-07, including a guard check for each organizer action. The two skips are the guards of `books.propose` and `notes.add`: Ben is a member and the generated book exists, so conform's second identical call inserts again and cannot make the statement change nothing. The replay proves both guards itself. On 2026-11-06, before those two guards, it reported `70 passed, 0 failed, 0 skipped.`

## Does zero changes instead of an authorization error hurt?

It did, in three places the first replay checked, and the guard fixed on 2026-11-06 answers each.

1. The page could not say why nothing happened: a member's `books.schedule` answered `{ changes: 0, lastInsertId: 0 }`. Each organizer action now declares a guard, so the call is refused with "Only an organizer can schedule a meeting, for a book on the shortlist while no other book is being read." The sentence still hedges, because one statement cannot tell a missing role from a book that does not exist or a book already being read; all three change no row.
2. Usage recorded the refused calls as successful: `books.schedule` counted 3 times. Now it counts Ana's one under `action` and Ben's two under `refused`.
3. The Action Contract listed no refusal for a role check, so `conform` passed a backend that dropped it. The contract now lists each guard, and `conform` fails a backend that answers `{ changes: 0 }` where the guard should refuse.

The predicate still holds on both Kernels. Nobody without the role changed a row.

## What the platform refused or could not do

| Area | Request | Answer |
| --- | --- | --- |
| Signup Policy | `publishLocal`, the function behind `konae publish`, on a private app | Fixed on 2026-11-06. The Deployment started Closed because `scripts/konae.ts` never sent `initialSignupPolicy`, so Invitation only needed a second step with a different credential. `konae.json` now names `project.manifest.signupPolicy`; the replay publishes as Invitation only with the publish token alone, and a republish naming Open prints a warning that names the Users view and `signup_policy_set` and changes nothing. |
| Signup Policy | `signup_policy_set` with the publish token | `Unknown tool or insufficient permission. signup_policy_set needs a credential with manage_users. Run konae login with --manage-users, or use a token from token_create that includes it.` Changing an existing app's policy needs `manage_users`, which the CLI's publish token lacks. This is the intended split: a publish names only the first release's policy. |
| Invitation only | `POST /_konae/auth/signup` from an uninvited address | 403 `{"error":"This app is not accepting new accounts for this email address."}` |
| Private file | Ben's `files_read` of Ana's note | `File not found.` |
| Private file | Ben's `GET /_konae/files/<note>` with his session | 404 `{"error":"File not found."}` |
| Private Deployment | A visitor's `GET /_konae/files/<id>` for a public cover, a private note or a missing ID | 401 `{"error":"authentication-required"}` for all three. This is the documented rule. A visitor of a private app is asked to sign in before any file is looked up, and on a public app a file the visitor may not read answers 404, as a missing one. |
| File visibility | `files_upload` with `visibility: "organizers"` on the runtime endpoint | `Value is outside the allowed choices.` There is no visibility for "organizers" or "one other member", so a member cannot share a note with the organizer. |
| File visibility | `files_update` of Ben's note with `visibility: "organizers"` | `Value is outside the allowed choices.` |
| File visibility | `miniapp.files.upload(file, { visibility: "organizers" })` in Ben's page | `File visibility must be private or public.` and no file stored. Before 2026-11-06 the SDK stored a private file without an error. |
| File lifecycle | Ana (organizer) deletes Ben's public cover with `files_delete` | `Only the person who uploaded this file may change or delete it.` An organizer is a row in the app; the Kernel knows only the uploader, so a removed book's cover stays until its uploader deletes it. |
| File lifecycle | Ana deletes Ben's private note with `files_delete` | `File not found.` |
| API key | A key with `files: ["public"]` reads a private note | 404 `{"error":"File not found."}` |
| API key | The same key on `/_konae/mcp` | Fixed on 2026-11-06: 403 `{"error":"An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session."}`. Before, 401 `{"error":"Sign in to use Konae MCP."}` |
| API key | `api_keys_create` naming `books.list` | `Action books.list has no HTTP route, so a key cannot call it.` A program can read covers but cannot list which books they belong to without an anonymous route, and on a private app an anonymous route is open to anyone who knows the path. |
| Roles in SQL | Ben (member) calls `books.schedule`, `books.remove`, `books.finish` | Fixed on 2026-11-06: each guard, such as `Only an organizer can remove a book.` `books.schedule` on a book that does not exist gets the same schedule guard. Before, `{ changes: 0, lastInsertId: 0 }` |
| Roles in SQL | Ana (organizer) calls `app_invitations_create` on the runtime endpoint | `Unknown tool or insufficient permission.` The organizer is a row in the app's database. Only the Project Owner holds `manage_users`, so the club's organizer cannot invite a new member. |
| Roles in SQL | Promoting an organizer | An `actor` binding yields only the user ID, so the app's database never sees an email. The Project Owner reads `app_users_list`, copies the user ID and pastes it into a `database_execute` SQL string. That operation takes no bound parameters. |
| Usage | `usage_read` after two refused schedules and one real one | Fixed on 2026-11-06 with the guard: `books.schedule` counts 1 under `action` and 2 under `refused`. Before, 3 calls with nothing separating the two that changed nothing. |
| Recovery | Cy's session from before the reset | `Sign in to use Konae MCP.` |
| Recovery | Cy signs in with the old password | 401 `{"error":"Email or password is incorrect."}` |
| Recovery | The same recovery link a second time | 410 `{"error":"This link is invalid, expired or already used."}` |
| Import | Ana's origin session cookie on the destination | `Sign in to use Konae MCP.` |
| Import | The origin's API key on the destination's file read path | 401 `{"error":"invalid-api-key"}` |
| Import | `api_keys_list` on the destination | Fixed on 2026-11-06. The replay first got `{ keys: [] }`, because the identity component dropped API keys, against what CONTEXT.md and `docs/mcp.md` said. The package now carries each key's name, scope and hash, revoked at import, and the replay asserts "Cover mirror" with `revokedReason: "import"`; the key itself still answers 401 `invalid-api-key`. |
| Import | Uploading a Package component with `McpClient.upload` | Fixed on 2026-11-06. The replay first called `transfer_begin` with `component` itself; `McpClient.upload(bytes, projectId, component)` now names the component, and the replay uses it. |

## Choices in the page

The organizer buttons show for every member with an "Organizers only" hint, so a member can press one and see the refusal. A real club would hide them by role. The covers and notes load through `/_konae/files/<id>`, the path `miniapp.files.url` returns, rather than `miniapp.files.list()`, because `files.list` downloads every readable file on each call. The cover uses the path directly because the page draws it in a synchronous template.
