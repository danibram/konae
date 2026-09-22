# Club

Club is a members-only book club. The Project Owner invites people by email. Members propose books with a cover image that every member sees, and keep private notes that only they can open. An organizer, whom the Project Owner promotes, schedules the next meeting, marks a book finished and removes books. The app runs as a private Deployment with the Invitation only Signup Policy, and it moves to a second Kernel as a Package with its accounts, rows, files and rules.

## Files

| File | What it holds |
| --- | --- |
| `index.html`, `styles.css`, `app.js` | The page. `app.js` calls only `miniapp.auth`, `miniapp.actions` and `miniapp.files.upload`, plus `fetch` and `<img>` on `/_littlebox/files/<id>`. `styles.css` copies the design system's tokens as plain CSS and follows the system theme. |
| `schema.sql` | `members` (user ID, display name, role), `books` (title, author, cover file ID, status, meeting date) and `notes` (owner, book, file ID). |
| `actions.json` | Ten authenticated Server Actions. Every query names a `LIMIT`. `books.schedule`, `books.finish` and `books.remove` check `role = 'organizer'` in their `WHERE`. |
| `replay.ts` | Starts two Kernels, runs every flow below, asserts the results and the refusals, and removes both Kernels and their data when it exits. |

## Platform parts it uses

- A Private Deployment, published by `publishLocal` in `scripts/littlebox.ts`, the function behind `littlebox publish`, then set to Invitation only with `signup_policy_set`.
- `app_invitations_create`, the development mailbox and `POST /_littlebox/auth/accept`.
- `miniapp.files.upload` with and without `visibility: "public"`, and `GET /_littlebox/files/<id>` with the app session cookie, as an `<img>` source for covers and a `fetch` for notes, under the app's Content Security Policy.
- `api_keys_create` with `files: ["public"]`, used with `Authorization: Bearer`.
- Roles in SQL. The Project Owner promotes an organizer with `database_execute` after finding her user ID with `app_users_list`.
- `POST /_littlebox/auth/recover` and `POST /_littlebox/auth/reset`.
- `package_export`, then `transfer_begin` with `component`, and a first release with `initialPackage` on a second Kernel under a second Project Owner.
- `littlebox contract` and `littlebox conform`.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
LITTLEBOX_SHOWCASE_PORT=19472 bun run examples/showcase/club/replay.ts
```

The origin Kernel listens on `LITTLEBOX_SHOWCASE_PORT` (default 19470) and the destination on the next port. Both use `LITTLEBOX_MAIL_TRANSPORT=development`, and the replay reads links from each Kernel's `mailbox.jsonl`. Set `LITTLEBOX_SHOWCASE_SCREENSHOTS` to a directory to keep a light and a dark screenshot, and `LITTLEBOX_SHOWCASE_BROWSER=0` to skip Chrome. The replay prints one `PASS` line per flow and a JSON report of every refusal it observed, and exits 0 when all of them pass.

## What worked

- **Invitation only.** A visitor gets the Kernel's sign-in page with "Only invited people can create an account" and no signup form. A signup from an uninvited address answers 403. Three invitations reached the mailbox, and each link set a password and returned a session.
- **Private and public files.** Ben cannot read Ana's private note through `files_read` or the file read path. He reads her public cover both ways, byte for byte. `files_list` gives him the two covers and his own note. A visitor gets 401 from the file read path.
- **Images under the policy.** Chrome signed Ben in through the Kernel's form, rendered both 120 by 180 covers from `/_littlebox/files/<id>`, fetched his note's text from the same path and reported no policy violation in light or dark.
- **API key.** A key with `files: ["public"]` reads a cover with `image/png` and the original bytes. It gets 404 for a private note and 401 on the runtime endpoint.
- **Roles.** Ana's `books.schedule` changes one row. Ben's `books.schedule`, `books.remove` and `books.finish` change none, and the page shows "Nothing changed. Only an organizer can schedule a meeting, and the book must still be on the shortlist."
- **Recovery.** The emailed link reset Cy's password. Her earlier session then answers "Sign in to use Littlebox MCP.", the old password fails, and the link works once.
- **Package.** The Package held database, identity, files and usage. On the destination, Ana, Ben and Cy sign in with their passwords (Cy with the new one). Ana is still organizer, the rows match, the cover and the note have the same bytes, Ben still cannot read Ana's note, and Ben's organizer mutation still changes nothing. Ana's origin session and the origin's API key both answer 401 there. The Signup Policy arrived as Invitation only, and the invitation history arrived as three accepted invitations.
- **Contract.** `littlebox conform` with Ben's session reports `67 passed, 0 failed, 0 skipped.` on the origin and again on the destination.

## Does zero changes instead of an authorization error hurt?

Yes, in three places the replay checks.

1. The page cannot say why nothing happened. `books.schedule` from a member and `books.schedule` on a book that does not exist both answer `{ changes: 0, lastInsertId: 0 }`. The refusal text has to hedge ("and the book must still be on the shortlist") because the app cannot tell the cases apart.
2. Usage records the refused calls as successful. After Ben's two schedules that changed nothing and Ana's one that worked, `usage_read` counts `books.schedule` 3 times. A Project Owner reading Usage sees three schedules.
3. The Action Contract lists no refusal for a role check, so `conform` passes every organizer action as a member, with zero changes. A replacement backend that dropped the role check would also pass.

The predicate still holds on both Kernels. Nobody without the role changed a row. The cost is that only the person who wrote the SQL knows that a 0 meant "refused".

## What the platform refused or could not do

| Area | Request | Answer |
| --- | --- | --- |
| Signup Policy | `publishLocal`, the function behind `littlebox publish`, on a private app | The Deployment starts Closed. `scripts/littlebox.ts` never sends `initialSignupPolicy`, so Invitation only needs a second step with a different credential. |
| Signup Policy | `signup_policy_set` with the publish token | `Unknown tool or insufficient permission.` Changing the policy needs `manage_users`, which the CLI's publish token lacks. |
| Invitation only | `POST /_littlebox/auth/signup` from an uninvited address | 403 `{"error":"This app is not accepting new accounts for this email address."}` |
| Private file | Ben's `files_read` of Ana's note | `File not found.` |
| Private file | Ben's `GET /_littlebox/files/<note>` with his session | 404 `{"error":"File not found."}` |
| Private Deployment | A visitor's `GET /_littlebox/files/<public cover>` | 401 `{"error":"authentication-required"}`. The public `feed` example gets 404 for the same request on a public app. |
| File visibility | `files_upload` with `visibility: "organizers"` on the runtime endpoint | `Value is outside the allowed choices.` Through the SDK the same option stores a private file without an error (`src/mcp/runtime.js` keeps only `"public"`). There is no visibility for "organizers" or "one other member", so a member cannot share a note with the organizer. |
| API key | A key with `files: ["public"]` reads a private note | 404 `{"error":"File not found."}` |
| API key | The same key on `/_littlebox/mcp` | 401 `{"error":"Sign in to use Littlebox MCP."}` |
| API key | `api_keys_create` naming `books.list` | `Action books.list has no HTTP route, so a key cannot call it.` A program can read covers but cannot list which books they belong to without an anonymous route, and on a private app an anonymous route is open to anyone who knows the path. |
| Roles in SQL | Ben (member) calls `books.schedule`, `books.remove`, `books.finish` | `{ changes: 0, lastInsertId: 0 }`, the same answer as `books.schedule` on a book that does not exist |
| Roles in SQL | Ana (organizer) calls `app_invitations_create` on the runtime endpoint | `Unknown tool or insufficient permission.` The organizer is a row in the app's database. Only the Project Owner holds `manage_users`, so the club's organizer cannot invite a new member. |
| Roles in SQL | Promoting an organizer | An `actor` binding yields only the user ID, so the app's database never sees an email. The Project Owner reads `app_users_list`, copies the user ID and pastes it into a `database_execute` SQL string. That operation takes no bound parameters. |
| Usage | `usage_read` after two refused schedules and one real one | `books.schedule` counted 3 calls. Nothing separates the two that changed nothing. |
| Recovery | Cy's session from before the reset | `Sign in to use Littlebox MCP.` |
| Recovery | Cy signs in with the old password | 401 `{"error":"Email or password is incorrect."}` |
| Recovery | The same recovery link a second time | 410 `{"error":"This link is invalid, expired or already used."}` |
| Import | Ana's origin session cookie on the destination | `Sign in to use Littlebox MCP.` |
| Import | The origin's API key on the destination's file read path | 401 `{"error":"invalid-api-key"}` |
| Import | `api_keys_list` on the destination | `{ keys: [] }`. The Package's identity component drops API Keys entirely, so the destination's Project Owner cannot see which programs had access at the origin or which keys to reissue. The docs disagree. CONTEXT.md's Package entry says accounts travel "without ... API keys", while its API Key entry says "An export carries it and the import revokes it", and `docs/mcp.md` says the same of `project_export`. |
| Import | Uploading a Package component with `McpClient.upload` | It has no `component` argument, so the replay calls `transfer_begin` with `component` itself, as `tests/package.integration.test.ts` does. The CLI's `export --with-data` kit and its `deploy.mjs` do this for a person who does not write code. |

## Choices in the page

The organizer buttons show for every member with an "Organizers only" hint, so a member can press one and see the refusal. A real club would hide them by role. The covers and notes load through `/_littlebox/files/<id>` rather than `miniapp.files.list()`, because `files.list` downloads every readable file on each call.
