# MCP guide

Konae's MCP on `/mcp` is how the editor, the build agent and an outside agent operate apps with an owner's credential. An app's own pages call the runtime endpoint, `/_konae/mcp` on the app's host, with an app session. Both speak MCP over Streamable HTTP, revision `2025-11-25`.

This page states each tool's arguments, answers and refusals. [MCP tools and guide topics](./mcp-tools.md) lists the tools, the permission each needs and the read-back after each write. [The walkthrough](./mcp-walkthrough.md) runs them in order on one app, with recorded answers.

## Credentials

Run `bun run dev:local`, open `http://localhost:4173/platform-access` and sign in on the owner sign-in page. The first owner of an installation uses "Create owner account" with the field "This computer's setup key", which a local install fills in. Every later owner needs an invitation code from the operator, as [Operate an installation](./operate.md#creator-admission) describes.

| Credential | Permissions it can hold | Lifetime |
| --- | --- | --- |
| A token from the owner sign-in page | `read` alone, or `read`, `write` and `publish` | 24 hours |
| `token_create`, from the owner's browser session | Any of `read`, `write`, `publish`, `manage_users` and `export_data` | Up to 30 days, one hour on an installation that signs owners in through Clerk |
| `konae login` | `read`, `write` and `publish`, plus `--manage-users` and `--export-data` | 24 hours |
| An OAuth client | `read`, `write`, `publish` and `manage_users`. Asking for `export_data` answers `Invalid scope.` | One hour, with no refresh token |

The owner's browser session holds every permission. A delegated token cannot create tokens or add a permission to itself, so ask `login` for every permission the work needs. The setup key never authenticates a tool, and Konae refuses to start on a reachable address while the key is the development default ([Before a Kernel is reachable](./operate.md#before-a-kernel-is-reachable)).

Configure `<editor origin>/mcp` in the MCP client, with OAuth or with `Authorization: Bearer <token>`. Then call `session_inspect`, whose `can` map says what this credential may do, and `platform_guide` with topic `rules`. A tool the credential does not list answers `Unknown tool or insufficient permission.` and the permission it needs, such as:

```text
app_invitations_create needs a credential with manage_users. Run konae login with --manage-users, or use a token from token_create that includes it.
```

A tool of the runtime endpoint called on `/mcp` answers that it runs on `/_konae/mcp` with an app session. A token limited to some projects answers `Project is outside this token's grants.` for any other project and cannot create projects.

## Projects

`project_create { name, projectId?, slug?, manifest?, template?, sourcesTransferId? }` needs `write`. It registers a project and returns a template's files, the ones `konae init` writes, because both read `kernel-rs/konae/src/project-templates.json`. The template is `tasks`, `notes` (the default), `members`, `form`, `webhook`, `digest` or `gallery`. Without a `manifest` the project takes the template's visibility. `project_inspect` returns the project, its Deployment and `sourcesRevision`, the number of times the editor saved its sources.

The editor uploads a new app's sources without a `projectId`, then calls `project_create` with `sourcesTransferId`. Konae checks the files and saves the project and its sources in one transaction, answering `sourcesRevision: 1`. A failed upload leaves no project. Repeating the same files returns the saved revision, and other files cannot replace a save through `project_create`.

`project_sources_save { projectId, transferId, expectedRevision }` saves the sources the editor opens when the revision still matches. The transfer holds this JSON, and an agent that saves sources sends the same:

```json
{ "id": "app_notes", "name": "Notes", "slug": "notes", "updatedAt": 1762430000000,
  "files": { "index.html": "<main id=app></main>", "app.js": "...", "schema.sql": "...", "actions.json": "..." },
  "directories": [],
  "manifest": { "version": 1, "visibility": "private",
    "runtime": { "database": { "mode": "declared-actions", "actions": [{ "name": "notes.list", "kind": "query", "access": "authenticated" }] },
      "auth": { "mode": "optional" }, "files": { "access": "deployment" } } } }
```

A save without one of these parts is refused with the part and its shape, such as `Invalid project manifest. Send { version: 1, visibility, runtime: … }`. A publish from a folder does not change the sources the editor saved.

`project_delete { projectId, name }` needs `write` and deletes a project for good. `name` must equal the project's name, or the call answers `The name does not match this project. Type the project's name exactly to delete it.` The address stops answering at once. Konae deletes the app folder (database, app accounts, uploaded files, backups, custom domain), the stored artifacts, the saved sources and the signing secrets, and answers with the deleted Deployment IDs. Read back with `projects_list`. An app in maintenance refuses it. Export the app first to keep its data. The build agent does not get this tool.

## Signup policy

Each Deployment has a Signup Policy that decides who may create an app account. `signup_policy_set { deploymentId, mode, allowlist? }` needs `manage_users`, and `signup_policy_read` needs `read`.

| `mode` | Who may create an account |
| --- | --- |
| `open` | Anyone. |
| `invitation` | Nobody signs up. An invited person creates the account from the invitation link. |
| `closed` | Nobody signs up, and a person the owner invites can still accept. Pending invitations stay acceptable when the owner closes signup, so revoke the ones that should end. |
| `allowlist` | Only exact emails or `@domain` entries, up to 100. A domain entry matches that domain and not its subdomains. Listing an address sends no invitation. |

A public Deployment starts Open and a private one Closed. A first release may set any mode in `initialSignupPolicy` with `publish`, and the Kernel installs it before the app serves a request. When the owner's request leaves access open for an app with accounts, an agent proposes private and `invitation`. A republish keeps the policy, except that republishing a public app as private turns Open into Closed. `initialSignupPolicy` on a republish answers `Portable state can only be imported on the first deployment.` The editor's Deploy dialog sends `initialSignupPolicy` on a first publish and shows the current policy read-only after that. The owner changes it in Auth > Users.

## Sign-in methods

`sign_in_methods_set { deploymentId, methods }` takes `["password"]` or `["password", "google"]` and needs `manage_users`. `sign_in_methods_read` reads it back with `read`. Password cannot be turned off, so every account keeps a way back in.

Each app has its own Google client, which the owner makes as a Web application client in their own Google Cloud console and sets as two signing secrets with `secret_set { projectId, name, value, use: "google" }`, named `google_client_id` and `google_client_secret`. Those names take only `use: "google"`, the values are never returned, and no route or call reads them. `sign_in_methods_read` answers `googleClient { clientIdSet, clientSecretSet }` and `redirectUris`, the callbacks to register in that client:

- `<app address>/_konae/auth/google/callback` on the generated host;
- the same on a verified custom domain;
- `<domain>/p/<id>/_konae/auth/google/callback` while the path address is on. The name address uses this callback too, so a rename leaves it working.

The installation's home address has no callback, and Google sign-in there answers "Google sign-in does not run on the installation's home address. Open this app on its own address to continue with Google." `sign_in_methods_set` refuses `google` while either half is unset, with "Set this app's Google client ID and client secret before turning Google on.", and `secret_delete` refuses either half while Google is on. Auth > Users in the editor shows the same redirect URIs, the two fields and the switch.

The flow stays on the app's host. `/_konae/auth/google/start` records state, nonce, PKCE verifier, Deployment and origin behind a cookie and sends the browser to Google. `/_konae/auth/google/callback` takes the flow once, on the same Deployment, origin and browser, exchanges the code with the app's client and checks the ID token: Google as issuer, the app's client as audience, expiry, the nonce, a key from Google's JWKS and a verified email. It then opens the app's own session.

A Google sign-in reaches the app account with the same email, compared without case, whether that account was made with a password or with Google. Google proves only an address it manages, a `gmail.com` address or one whose Workspace domain the token names in `hd`. Any other address answers "This Google account uses an email address Google does not manage, so it cannot sign in here. Sign in with email and password." A suspended account is refused and a pending one keeps waiting for its link. An active account whose email nobody proved, as local verification allows, goes to the person Google vouches for, and its password and sessions stop working. For an email with no account, the Signup Policy decides as it does for a password. An account created through Google has no password until recovery by email sets one.

An export carries `signInMethods` and the accounts, never the client. On another installation the owner sets the two secrets again and registers that host's redirect URI. Until then the sign-in page offers only email and password, and `sign_in_methods_read` returns a `notice` that says so.

## Users and invitations

These tools take a `deploymentId`, need `manage_users` and the project grant, and write an audit row naming the Deployment. The editor's Auth > Users calls the same tools with the owner's session. It reuses one idempotency key when a batch is retried, and it turns invite and resend off, with the reason, while `mail_status` reports `sending: false`.

| Tool | Arguments | Result |
| --- | --- | --- |
| `app_users_list` | `limit` (1 to 100, default 50), `cursor`, `state` | `{ users: [{ userId, email, state, verified, verifiedAt, createdAt }], nextCursor }`, ordered by email. |
| `app_user_suspend`, `app_user_resume` | `userId` | `{ userId, email, state }`. Suspension ends sessions and open links and keeps the ID, password and rows. Resume returns `active` if the account ever verified, otherwise `pending`. |
| `app_user_enter` | `userId` | `{ userId, email, url, expiresAt }`. `url` is `/_konae/enter#entry=<token>` on the app's host and signs the browser that opens it in as that active account, once, within two minutes. Needs the owner's own browser session. The audit row names the account, and Usage and Signals do not count the session. |
| `app_invitations_create` | `emails` (1 to 50), `idempotencyKey` (8 to 128 characters) | `{ policy, results: [...] }`, one per distinct address: `invited` with `invitationId`, `expiresAt`, `acceptable` and `delivery`; `already-invited` with the pending `invitationId`; `already-member`; `suspended`; or `invalid-email`. |
| `app_invitations_list` | `limit`, `cursor` | `{ invitations: [{ invitationId, email, status, invitedBy, createdAt, expiresAt, acceptedAt, revokedAt, delivery, sends }], nextCursor }`, newest first. |
| `app_invitation_resend` | `invitationId`, `idempotencyKey` | A new seven-day link for a pending invitation; the earlier link stops working. Refused for an accepted or revoked invitation and for an active or suspended account. |
| `app_invitation_revoke` | `invitationId` | Ends a pending invitation. Revoking twice answers the same; an accepted invitation is refused. |

An invitation sends email, so an agent calls `app_invitations_create` only when the owner asked for those invitations. The subject names the inviting owner's email and the app, such as `owner@example.com invited you to Tasks`. An owner who signs in through Clerk has no email in Konae, so the subject names only the app, `You are invited to Tasks`. No text from the call reaches the message. Without a mail transport the call refuses before creating anything. A retry with the same `idempotencyKey` from the same owner returns the first answer, kept seven days, and sends nothing; the same key with other arguments is refused. `acceptable` is false only when an Allowlist leaves the address out. Closed accepts an invitation, because the owner chose the person.

`status` is `pending`, `expired`, `needs-resend`, `already-member`, `accepted` or `revoked`. `needs-resend` means the link is gone, because the invitation came through a copy or its account was suspended. `already-member` means the address became an active account another way. `delivery` is `queued` when the mail transport took the message, `failed` when it refused it, and `not-sent` when nothing left this installation; `queued` says nothing about the inbox. A tighter policy blocks new accounts only, and a refused signup gets the same answer whether signup is closed or the address is not listed.

Under local verification nobody proves an email, so whoever signs up first with a listed address gets that account. Use Closed for an app that holds private data, or require verification.

## Verified accounts

`KONAE_ACCOUNT_VERIFICATION=required` makes every new account, owner or app user, prove its email before it is active. The default `local` keeps unverified signup for development; [Operate an installation](./operate.md#verified-accounts) covers the switch.

An email is a plain `local@domain`: one `@`, a dotted domain, and no spaces, control characters or any of `<>()[],;:\"`. Anything else answers `Enter a valid email address.` An account holds at most 20 sessions, and a sign-in past that ends the oldest.

An account is `pending`, `active` or `suspended`, and only an active one gets a session. A challenge has a purpose, `verification`, `recovery` or `invitation`, and a scope, the owners or one Deployment. The Kernel keeps only its SHA-256 hash. A link works once and expires: verification after 24 hours, invitation after seven days, recovery after 30 minutes. A new link for the same purpose replaces the earlier one.

In required mode, signup takes only an email, and the person chooses the password on the page the link opens. Links carry the token in the fragment, `#verification=`, `#recovery=` or `#invitation=`, so it reaches no server log. Opening a link changes nothing until the person posts the form.

| Route | Host | Body | Result |
| --- | --- | --- | --- |
| `POST /_konae/auth/signup` | app | `{ "email" }` | 202 `{ "result": { "status": "link-sent" } }` for every email, with the signup hint cookie. 403 when the policy refuses. |
| `POST /_konae/auth/verify` | app | `{ "token", "password" }` | 200 with the app session cookie. Under Closed, or for an email no longer listed, 403 and the link stays usable. |
| `POST /_konae/auth/recover` | app | `{ "email" }` | 202 for every email. An active account gets a recovery link, a pending one a new verification link, others nothing. |
| `POST /_konae/auth/reset` | app | `{ "token", "password" }` | 200 with a new session; every earlier session ends. |
| `GET /_konae/account` | app | | The page links open. Without a token it offers recovery. |
| `GET /_konae/invitation` | app | | The page an invitation link opens. It changes nothing. |
| `POST /_konae/auth/invitation` | app | `{ "token" }` | 200 `{ "result": { "email", "app", "status" } }` with `status` `new-account`, `existing-account` or `signup-closed`. Reads only. |
| `POST /_konae/auth/accept` | app | `{ "token", "password" }` | 200 with the app session cookie. One transaction consumes the token, rechecks the policy and the account, sets the password and activates the account. 403 under an Allowlist without the address and for a suspended account, 409 for an active one. Every refusal keeps the token usable. |
| `POST /_konae/owner/signup` | editor | `{ "email", "code" }`, or `"setupKey"` for the first owner | See [owner signup](#owner-signup). |
| `POST /_konae/owner/verify`, `/_konae/owner/reset`, `/_konae/owner/recover` | editor | as above | The owners' scope. Links open `/_konae/owner/login`. |

Every 202 from an app signup, over HTTP or `auth_signup`, sets an `HttpOnly` cookie, `konae_signup` (`__Host-konae_signup` on HTTPS), holding the SHA-256 of the Deployment ID and the email typed. A failed `POST /_konae/auth/login` still answers 401, and when the request carries that cookie for the email typed, the message is `Email or password is incorrect. If you signed up with this email in this browser, open the link we emailed to choose your password first.` Without it the message is `Email or password is incorrect.` The hint depends on the cookie alone, so it tells nobody which addresses have accounts.

A used, expired, replaced, revoked or wrong-purpose token answers 410. A suspended account answers 403. A weak password answers 400 before the token is read. Without a mail transport, signup and recovery answer 503.

The runtime tools `auth_signup`, `auth_verify`, `auth_recover` and `auth_reset` run the same flows for the app on the request's host. A signed-out visitor may call them on a public or a private app; on a private app they are the only tools a signed-out caller reaches. In local mode `auth_signup` also takes `password` and signs in at once. A tool that signs someone in sets the session cookie and never returns a token.

## Owner signup

`POST /_konae/owner/signup` takes `{ "email", "code" }`, plus `"password"` in local mode, or `"setupKey"` instead of the code for the first owner. One transaction consumes the code and creates the account, which is pending with a verification link in required mode and active with a session in local mode. A code for an email whose account is active or suspended is consumed, creates nothing and gets the same 202. When both `code` and `setupKey` arrive, the code decides. `GET /_konae/owner/config` answers `"admission": "setup-key"` while the key can still create an account and `"code"` otherwise.

| Request | Status | Message |
| --- | --- | --- |
| A used, expired, revoked, unknown or mistyped code | 403 | `This invitation code is not valid. It may be mistyped, expired, revoked or already used.` |
| The setup key once a code or another account exists | 403 | `An invitation code is required to create a creator account. Ask the operator of this installation for one.` |
| A wrong setup key while it can still create the first owner | 403 | `A valid installation setup key is required to create the first owner.` |

An MCP client that starts OAuth while the owner is signed out sends them to `/_konae/owner/login?returnTo=/oauth/authorize?...`. Konae keeps `returnTo` only when it starts with `/oauth/authorize?` and fits in 4096 bytes, and puts it in the verification link, so the person lands back on the consent screen after choosing a password.

## Account email

Account links and every [notice](#notices) leave through the queue in `mail.sqlite`. [Operate an installation](./operate.md#account-email) lists the transports, their variables and the limits. A refused request stores nothing. 503 means no transport, or a sender the preflight refused, and 429 means a limit.

`mail_status` answers for any owner credential with the transport, the sender, `sending`, the preflight (`checking`, `ready` or `refused`) with its reason and time, the queue depth and count per state, the same counts for notices under `queue.notices`, the last failure as `{ kind, at, purpose }` with `purpose` `verification`, `recovery`, `invitation` or `notify`, and the limits. `refresh: true` runs the preflight again. It names no recipient. `/_konae/health` carries `mail` with `transport`, `sending`, `preflight`, `depth` and `lastFailure`.

A job is `pending`, `accepted` (the provider took it), `delivered` (the provider reported delivery to the recipient's server) or `failed`. No job state activates an account.

A failure kind is one of `provider-unavailable` (retrying, or out of retries), `rejected` (the provider refused that message), `sender-refused` (the provider refused the sender; sending stops until a preflight passes), `superseded` (a newer link replaced it, or its challenge ended), `link-expired`, `secret-unavailable` (the sealing secret changed), `bounced`, `provider-failed`, `suppressed`, `restored` (a restore ended it), and for a notice `sender-not-ready` (the preflight was not ready when the mutation ran, so the notice was recorded and never sent) and `notice-expired` (not sent within a day).

## Roles inside an app

The Kernel keeps no role for an app user. An app keeps roles in its own table and checks them in SQL against the actor's ID:

```json
{
  "kind": "mutation",
  "access": "authenticated",
  "statement": "UPDATE profiles SET role = 'admin' WHERE user_id = ?1 AND (SELECT role FROM profiles WHERE user_id = ?2) = 'admin'",
  "bindings": [{ "source": "input", "key": "userId", "type": "string", "maxLength": 64 }, { "source": "actor", "field": "id" }],
  "guard": "Only an admin of this app can make another admin."
}
```

The owner names the first administrator with `database_execute` operation `sql`, or in the editor's Data panel, using the ID `app_users_list` returns. `auth_session` answers `role: "member"` for every signed-in account and `role: "anonymous"` for a visitor, which tells signed in from signed out and nothing more.

## Runtime endpoint

The SDK calls `/_konae/mcp` on the app's host with the app session. [The miniapp SDK](./sdk.md) lists every call with its answer, its refusals and its limits. That page, the `sdk` topic of `platform_guide` and the build agent's prompt come from one contract, `src/sdk/contract.ts`.

On a public app a visitor may call actions declared `anonymous` and read files uploaded as `public`. Uploads, private files and `authenticated` actions need a session. On a private app nothing loads or runs without one.

A signed-out visitor may call `actions_invoke` 30 times per 10 seconds per client address and Deployment, and each app user 120 times per 10 seconds. Past that the call answers `rate-limited` with `Retry-After`. The Kernel counts the socket's peer address and reads `X-Forwarded-For` only from peers in `KONAE_TRUSTED_PROXIES`.

Set `manifest.fileAccess` to `"app-users"` to require an app session for every file, including existing uploads. The uploader keeps deletion rights. Removing the policy restores each original file visibility. Files uploaded under the policy then become private to the uploader.

By default, a file is private to the app user who uploaded it unless the upload set `visibility` to `public`. `files_update { fileId, visibility }` and `files_delete { fileId }` work only for the uploader. A user who may not read the file gets `File not found.`, as for a missing one, and a user who reads it but did not upload it gets `Only the person who uploaded this file may change or delete it.` A deleted file stops counting against the quota. A row that still names it keeps the ID, and the app removes that row with its own action.

### Statement rules

An action runs one SQL statement. A query starts with `SELECT` and a mutation with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`, after any leading comment. Each refusal carries a code from `kernel-rs/kernel/src/refusal-codes.json`, and topic `refusals` of `platform_guide` lists them with their fix. The Gate and the Kernel run the cases in `kernel-rs/kernel/src/sql-rule-cases.json`.

| Statement | Message |
| --- | --- |
| A query that opens with `WITH` | `Action <name> is a query, so it must start with SELECT. Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).` |
| A mutation that opens with `WITH` | `Action <name> is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE. Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.` |
| A `CREATE`, `ALTER` or `DROP` | `Action <name> is a query, so it must start with SELECT. Tables and indexes belong in schema.sql, and Konae runs no triggers or views.`, or the same for a mutation |
| Any other first word | `Action <name> is a query, so it must start with SELECT.` or `Action <name> is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE.` |
| Text after the first `;` other than comments and whitespace | `Action <name> must contain one statement.` |
| `RETURNING` on a mutation | `Action <name> cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.` |
| A query that reads rows with no top-level `LIMIT` | `Action <name> reads rows and has no LIMIT. Add a LIMIT. ...`, with the reason, such as `uses OVER, which keeps one result row per stored row`. See [The Gate and the Kernel](./gate-and-kernel.md) |
| A trigger, a view or a row in `schema.sql` | `schema.sql may only create tables and indexes, but it contains CREATE TRIGGER.`, naming each refused statement |

A `WITH` clause may sit inside the statement: `INSERT INTO bookings (room_id, slot) WITH span(slot) AS (...) SELECT ...` passes. A keyword or a `;` inside a string, a quoted name or a comment does not count.

### Action shape

Before it reads a statement, each door checks the number of actions, then each action in sorted name order: its name, access, kind, bindings and size. `kernel-rs/kernel/src/action-cases.json` holds the cases.

| Declaration | Message |
| --- | --- |
| More than 100 actions | `A project may declare at most 100 actions.` |
| A name that is not lowercase dotted words of letters and digits, or is over 100 characters | `Invalid server action name: <name>` |
| `"access": "granted"` | `Action <name> uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated".` |
| Any other access | `Action <name> has an invalid access. Use "anonymous" or "authenticated".` |
| A kind other than `query`, `mutation`, `signal` or `call` | `Action <name> has an invalid kind. Use "query", "mutation", "signal" or "call".` |
| `"nullable": true` on a binding that is not an input | `Action <name> has an invalid nullable input binding.` |
| A binding of another source, an input with no key or another type, an actor field other than `id`, a kernel field other than `now`, or a literal that is not a string, a number or null | `Invalid binding <index> in <name>.` |
| A string input's `maxLength` outside 1 to 100,000 | `Invalid maxLength in <name>.` |
| A statement over 20,000 bytes, or more than 100 bindings | `Action <name> exceeds statement or binding limits.` |
| A top-level key the kind does not take, checked last | `Action <name> has a key <key> that a <kind> action does not take.` |

| Kind | Keys |
| --- | --- |
| `query` | `kind`, `access`, `statement`, `bindings`, `http` |
| `mutation` | `kind`, `access`, `statement`, `bindings`, `http`, `signal`, `guard`, `notify`, `schedule` |
| `signal` | `kind`, `access`, `bindings`, `signal`, and `statement` only when it is empty |
| `call` | `kind`, `access`, `bindings`, `url`, `method`, `body`, `headers`, `auth`, `then`, and `statement` only when it is empty |

A key set to `null` counts as absent. A key with its own refusal keeps it, so a `guard` on a query gets the guard's sentence. A field of the wrong JSON type, such as `"bindings": {}`, is refused by both doors, but the Kernel answers with its JSON reader's message, `Invalid actions.json: ...`.

## HTTP routes

An action declared `anonymous` may answer one method and path on its Deployment's host, so a project receives a webhook or serves JSON without a server of its own. `kernel-rs/kernel/src/http.rs` implements it, and [ADR-0013](./adr/0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md) records the decision.

```json
"orders.receive": {
  "kind": "mutation",
  "access": "anonymous",
  "http": { "method": "POST", "path": "/hooks/orders", "signature": { "header": "X-Signature", "secret": "orders_webhook" } },
  "statement": "INSERT INTO orders (external_id, total) VALUES (?1, ?2)",
  "bindings": [
    { "source": "input", "key": "id", "type": "string", "maxLength": 64 },
    { "source": "input", "key": "total", "type": "number" }
  ]
}
```

A query binds `GET` and a mutation binds `POST`. A path starts with `/` and has segments of letters, digits, `.`, `_`, `~` and `-`, up to 200 characters, with no `.` or `..` segment and no trailing slash. Paths under `/_konae`, `/oauth`, `/.well-known` and `/mcp` belong to the Kernel. Two actions share a path only with different methods. A route wins over an artifact file at its path for its method, and every other `GET` serves the Build Artifact. An `authenticated` action may not declare `http`. The Gate and the Kernel run the cases in `kernel-rs/kernel/src/http-binding-cases.json`.

A `GET` maps the query string onto the input bindings by type, so `?min=10` binds the number 10. A `POST` reads a JSON object of at most 64 KiB. An input the action does not declare is refused unless the route declares `fields`, described under [Webhooks from outside services](#webhooks-from-outside-services). A route keeps the action's bindings, its query bound and the runtime endpoint's rate limits. A call that succeeds counts in Usage under the route's method and path and records the action's Signal.

| Answer | Status | When |
| --- | --- | --- |
| `{ "result": [rows] }` or `{ "result": { "changes", "lastInsertId" } }` | 200 | The action ran. Every answer carries `Cache-Control: no-store`. A `GET` route that answers anyone also carries `Access-Control-Allow-Origin: *` |
| `{ "error": "<binding message>" }` | 400 | An input is missing, unexpected or of the wrong type, the body is not a JSON object, or a query parameter repeats |
| `{ "error": "invalid-signature" }` | 401 | The signature is missing, malformed, lacks its declared prefix, or does not match |
| `{ "error": "stale-timestamp" }` | 401 | The signature's time is more than 300 seconds off, or outside a `timestamped` signature's `window`. Checked before the MAC |
| `{ "error": "authentication-required" }` | 401 | A route that takes an API key or a session, and no signature, got neither |
| `{ "error": "invalid-api-key" }` | 401 | The bearer token starts with `lbk_` and names no live key of this Deployment |
| `{ "error": "This route does not accept API keys." }` | 403 | The route's `callers` leave out `key` and the request carries a live key |
| `{ "error": "This API key may not call <action>." }` | 403 | The key does not name the action |
| `{ "error": "<guard>" }` | 403 | The mutation's [guard](#guards) refused |
| `{ "error": "No route accepts POST on this path." }` | 405 | No action binds the path for `POST` |
| `{ "error": "A route does not answer HEAD. Send GET." }` | 405 | A `HEAD` reaches a path a query binds, refused before any SQL with `Allow: GET` |
| `{ "error": "replayed-signature" }` | 409 | The same signature arrived on this path in the last 600 seconds, or a provider's delivery arrived before |
| `{ "error": "The request body exceeds 64 KiB." }` | 413 | |
| `{ "error": "rate-limited" }` | 429 | With `Retry-After`. Also after 10 unknown or revoked `lbk_` keys from one client address to one Deployment in a minute; from then on every key from that address is refused until the bucket refills |

### Who may call a route

The Kernel resolves the caller before any SQL runs.

| Caller | `callers` entry | Proof | Actor ID |
| --- | --- | --- | --- |
| A service | `"signature"` | The signature the route declares | None, so an action that binds the actor's ID refuses |
| A program the owner wrote | `"key"` | `Authorization: Bearer lbk_...`, an API key that names the action | The key's ID, `key_...` |
| A person | `"session"` | The app session cookie, sent from the app's own origin | The app user's ID |

Without `"callers"`, a signed route takes `["signature", "key"]`, an unsigned route on a private Deployment takes `["key", "session"]`, and an unsigned route on a public Deployment answers anyone. `"signature"` belongs in the list exactly when the route declares a signature. A key the route does not take answers 403. A session the route does not take counts for nothing. The Kernel uses the cookie only when the request comes from the app's own origin, so a request whose `Sec-Fetch-Site` is `same-site` or `cross-site`, or whose `Origin` is another host, runs as a visitor.

```json
"events.feed": {
  "kind": "query", "access": "anonymous",
  "http": { "method": "GET", "path": "/api/events", "callers": ["key"] },
  "statement": "SELECT id, title FROM events WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT 100",
  "bindings": [{ "source": "input", "key": "after", "type": "number", "nullable": true }]
}
```

An action whose route does not answer anyone answers only there. `actions_invoke` refuses it with `Action <name> answers only on its HTTP route, <method> <path>.`, always for a signed route or one that declares `callers`, and on a private Deployment for an unsigned one. The app's own page reads a private route with `fetch`, which sends the session cookie.

| Declaration | Message |
| --- | --- |
| `callers` that is not a non-empty list of distinct `"signature"`, `"key"` and `"session"` | `Action <name> has an invalid callers list. List one or more of "signature", "key" and "session", each once.` |
| `"signature"` without a declared signature | `Action <name> lists "signature" among its callers but declares no signature.` |
| A signature that `callers` leaves out | `Action <name> declares a signature, so its callers must include "signature".` |

### Signatures

The owner sets the secret before publishing. A release whose signature names an unset secret is refused at `release_plan` and at `release_apply` with `Action <name> signs with the secret <secret>, which is not set for this project. Set it with secret_set before publishing.`, and one refusal names every action and secret when several are unset. The Gate makes the same refusal before Deploy from `secrets_list`, which answers names and never values. The cases are in `kernel-rs/kernel/src/release-cases.json`.

```text
secret_set    { "projectId", "name": "orders_webhook", "value": "<16 to 512 printable characters, no spaces>", "use"?: "route" | "call" | "google" }
secrets_list  { "projectId" }           names, uses, update times and svixKey, never values
secret_delete { "projectId", "name" }   refused while the published Deployment signs with it or a call uses it
```

A signature names no app, so a request signed for one project would pass on another holding the same value. `secret_set` therefore refuses a route secret that another project of the same owner holds, with `Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.`

Secrets live in the app folder's `data/secrets.sqlite`, which exists from `project_create`, so a first publish finds them. A scheduled copy carries them. They never enter `actions.json`, the Build Artifact, a listing, the audit log or an export. On another installation the owner runs `project_create`, then `secret_set`, then the release.

The sender puts this in the declared header:

```text
t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256, keyed with the secret, of:
<t>\n<nonce>\n<METHOD>\n<path>\n<canonical query string>\n<raw request body>>
```

The nonce is 1 to 64 letters, digits, `-` and `_`, fresh for each request, so a retry is signed again. The canonical query string is the request's pairs sorted by key, then value, written as `application/x-www-form-urlencoded`: spaces as `+`, and every byte but letters, digits, `*`, `-`, `.` and `_` as `%XX`. It is empty when the request has no query.

The Kernel refuses a `t` more than 300 seconds from its clock with `stale-timestamp` before it looks at the MAC. It then recomputes the MAC over the exact bytes it received, compares in constant time, and remembers each accepted signature for 600 seconds in the `route_signatures` table of `identity.sqlite`, so a replay answers 409 across a restart and on an imported app. Two `v1=` values may be sent while a secret rotates; more answers `invalid-signature` before any MAC is computed. `src/lib/route-signature.ts` signs the same way, and `kernel-rs/kernel/src/signature-cases.json` holds vectors both sides check. A sender with OpenSSL:

```sh
body='{"id":"A-1","total":12}'
t=$(date +%s)
n=$(openssl rand -hex 12)
sig=$(printf '%s\n%s\n%s\n%s\n%s\n%s' "$t" "$n" POST /hooks/orders "" "$body" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')
curl -X POST "https://orders-1a2b3c.example.net/hooks/orders" \
  -H 'Content-Type: application/json' -H "X-Signature: t=$t,n=$n,v1=$sig" --data-binary "$body"
```

### Webhooks from outside services

A service the owner does not control signs its webhook its own way and posts its own nested JSON. A route takes it as it arrives when its signature names the provider's `"scheme"` and its `"fields"` say where each input sits in the body. `kernel-rs/kernel/src/provider.rs` implements it.

```json
"issues.receive": {
  "kind": "mutation", "access": "anonymous",
  "http": {
    "method": "POST", "path": "/hooks/github",
    "signature": { "scheme": "hmac-body", "header": "X-Hub-Signature-256", "secret": "github_webhook", "algorithm": "sha256", "encoding": "hex", "prefix": "sha256=" },
    "fields": { "id": "$.issue.node_id", "title": "$.issue.title", "repo": "$.repository.full_name" }
  },
  "statement": "INSERT INTO issues (node_id, title, repo) VALUES (?1, ?2, ?3) ON CONFLICT (node_id) DO NOTHING",
  "bindings": [
    { "source": "input", "key": "id", "type": "string", "maxLength": 100 },
    { "source": "input", "key": "title", "type": "string", "maxLength": 300 },
    { "source": "input", "key": "repo", "type": "string", "maxLength": 200 }
  ]
}
```

| Scheme | Keys | What the sender puts in the request | Replay |
| --- | --- | --- | --- |
| `konae`, the default | `header`, `secret` | The header [above](#signatures) | The MAC, for 600 seconds |
| `hmac-body` | `header`, `secret`, `algorithm` (`sha256` or `sha1`), `encoding` (`hex` or `base64`), optional `prefix` | The prefix, then the HMAC of the raw body. GitHub: `X-Hub-Signature-256`, `sha256`, `hex`, prefix `sha256=`. Shopify: `X-Shopify-Hmac-Sha256`, `sha256`, `base64` | The MAC, for 72 hours |
| `timestamped` | `header`, `secret`, `prefix` (the time field, such as `t=`), `tag` (the MAC field, such as `v1=`), `window` (1 to 3600 seconds) | Comma-separated fields: the Unix time after `prefix` and the hex HMAC-SHA256 of `<t>.<body>` after `tag`. Stripe: `Stripe-Signature`, `t=`, `v1=`, `300` | A `t` outside the window answers `stale-timestamp`; inside it the MAC is accepted once |
| `svix` | `secret` | `svix-id`, `svix-timestamp` and `svix-signature`, the base64 HMAC-SHA256 of `<id>.<t>.<body>` keyed with the base64 after `whsec_`. Resend and Svix send it | A timestamp more than 300 seconds off answers `stale-timestamp`; the id is accepted once in 72 hours |

A `timestamped` or `svix` header may carry up to four MAC values while the provider rotates a secret. A svix secret is the `whsec_` value the provider shows; a release whose svix route names any other value is refused with `Action <name> verifies svix signatures with the secret <secret>, which is not a whsec_ signing secret. Set it to the value the provider shows, starting with whsec_.` `secrets_list` answers `svixKey`, so the Gate makes the same refusal without seeing the value.

The Kernel records an accepted delivery before the action runs, in `route_deliveries` of `identity.sqlite`, so two copies sent at once do not both run. When the action then fails, for a missing input, a SQL error or a guard, the Kernel forgets the delivery and the provider's retry runs. `hmac-body` signs the body alone, so give each provider route its own secret. The provider's event ID in a `UNIQUE` column, with `ON CONFLICT DO NOTHING`, is the lasting guard against a delivery replayed after 72 hours.

`fields` maps an input key to a JSON path into the POST body: `$`, then 1 to 16 steps of `.name` or `[index]`, up to 200 characters, such as `$.data.object.id`. The route reads each mapped input from its path, every other input from the body's key of the same name, and ignores every other key. A path the body lacks leaves its input out, so a required input answers `Missing action input: <key>` and a nullable one binds `NULL`. `fields` works on any POST route, signed or not.

| Declaration | Message |
| --- | --- |
| A scheme other than the four | `Action <name> names an unknown signature scheme. Use "konae", "hmac-body", "timestamped" or "svix".` |
| A key the scheme does not take | `Action <name> has an invalid <scheme> signature. It takes "scheme", "header" and "secret".`, with the scheme's own list |
| A provider's scheme on a `GET` route | `Action <name> binds GET, so its signature must use the konae scheme. A provider's scheme signs a POST body.` |
| No header, or one the Kernel reads itself | `Action <name> has an invalid signature header.` |
| `hmac-body` with no or another `algorithm` | `Action <name> has an invalid signature algorithm. Use "sha256" or "sha1".` |
| `hmac-body` with no or another `encoding` | `Action <name> has an invalid signature encoding. Use "hex" or "base64".` |
| `hmac-body` with a `prefix` of other characters or over 32 | `Action <name> has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256=".` |
| `timestamped` with no `prefix`, or one that is not a name and `=` | `Action <name> has an invalid timestamp prefix. Use the field's name and "=", such as "t=".` |
| `timestamped` with no `tag`, one that is not a name and `=`, or the prefix again | `Action <name> has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's.` |
| `timestamped` with no `window`, or one outside 1 to 3600 | `Action <name> has an invalid signature window. Use a whole number of seconds from 1 to 3600.` |
| `fields` on a `GET` route | `Action <name> binds GET, so it cannot declare fields. Fields read a POST body.` |
| `fields` that is not an object of 1 to 32 keys | `Action <name> has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id".` |
| A key no input binding declares | `Action <name> maps <key>, which is not one of its input bindings.` |
| A path that is not text, lacks `$`, is `$` alone or has another step | `Action <name> maps <key> to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id".` |

`src/lib/provider-signature.ts` signs the way each provider does, and `kernel-rs/kernel/src/provider-signature-cases.json` holds vectors both sides check. A form-encoded body, as Slack and Twilio send, answers 400.

### API keys

```text
api_keys_create { "deploymentId", "name", "actions": ["orders.receive"], "files": ["private"] }
api_keys_list   { "deploymentId" }               metadata, revocation times and reasons, never the key
api_keys_revoke { "deploymentId", "keyId" }
```

`api_keys_create` returns the key once. A key belongs to one Deployment, grants no owner tool, and works only on HTTP routes and the file read path. The runtime endpoint refuses it with 403 `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.` A key names actions that have an HTTP route and the file visibilities `public` and `private`; a key that names `private` reads every private file of its Deployment. A Deployment holds at most 50 live keys. The Kernel keeps the key's SHA-256 hash in `identity.sqlite`.

An export carries each key's name, actions, file visibilities and hash, and an import marks every live key revoked. `api_keys_list` on the destination shows those with `revokedReason: "import"`, and Auth > Integrations lists them under "Revoked at import", so the owner knows which programs need a new key. A key the owner revoked has `revokedReason: null`. A restore keeps keys live. Auth > Integrations in the editor also sets and deletes secrets and shows a new key once with a curl for each route it names.

### The file read path

`GET /_konae/files/<id>` on the Deployment host returns a stored file with its content type, except that a JavaScript, HTML, XML or SVG type is sent as `application/octet-stream`. It sends `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; sandbox`, `Cross-Origin-Resource-Policy: same-origin` and `Cache-Control: private, no-store`. PNG, JPEG, GIF, WebP and AVIF answer `inline`, every other type `attachment`. `miniapp.files.url(fileId)` returns this path, so an `img` or a `fetch` on the app's own origin reads it.

A person reads by the file rules above. With `fileAccess: "app-users"`, only a session for that app grants access. Otherwise a key reads the visibilities it names, and on a public Deployment also public files. A visitor with no session or key on a private Deployment gets 401 `authentication-required` before any file is looked up. Every other caller who may not read the file gets 404 `File not found.`, the same as for a missing file. There is no listing at `/_konae/files/`, and nothing is served from the bucket.

## Calls between apps

An action of `"kind": "call"` sends one HTTP request to another app or an outside service. The Kernel sends it from its own process, so a signing secret or an API key the request needs stays in the Kernel. `kernel-rs/kernel/src/calls.rs` implements it.

```json
"tasks.escalate": {
  "kind": "call",
  "access": "authenticated",
  "method": "POST",
  "url": "https://desk-4f2a9c.apps.example.org/hooks/tickets",
  "bindings": [
    { "source": "input", "key": "taskId", "type": "number" },
    { "source": "input", "key": "title", "type": "string", "maxLength": 200 }
  ],
  "body": { "ref": "taskId", "title": "title" },
  "auth": { "secret": "desk_tickets", "header": "X-Desk-Signature" },
  "then": { "action": "tasks.escalated", "inputs": { "id": "/input/taskId", "status": "/status", "note": "/body/error" } }
}
```

`miniapp.actions.call("tasks.escalate", { taskId, title })` checks the caller and the inputs, queues the call and answers `{ callId, queued: true }`. A call binds only inputs. `body` maps each key to an input or to a constant written `{ "value": "tasks" }`. A `POST` sends a JSON object and a `GET` sends query parameters, and an input that is null or missing leaves its key out. Usage counts each queued call under `call`.

| `auth` | What the request carries |
| --- | --- |
| `{ "secret": "<name>" }` | A signature in Konae's [route scheme](#signatures), in `X-Konae-Signature` or the header `"header"` names, signed again for each attempt |
| `{ "key": "<name>" }` | `Authorization: Bearer <value>`, where the secret holds an API key the other app's owner issued |

A secret that signs a call to another app of the same owner holds that app's route secret, which `secret_set` refuses as a reused value. Set it with `"use": "call"`. Such a secret verifies no route of its own app, and a route that names it is refused at publish with `Action <name> signs its route with the secret <secret>, which is set for calls only. Set a route secret with a value of its own.`

### Where a call may go

A URL is `https`, or `http` only to an app of this installation.

| Rule | Checked | Refusal |
| --- | --- | --- |
| An absolute `http` or `https` URL, up to 2048 characters, with no user, password, fragment, space or control character | Both doors | `Action <name> has an invalid url. Use an absolute https:// URL without credentials or a fragment, up to 2048 characters.` |
| No IP address as the host, in any notation | Both doors | `Action <name> calls the IP address <host>. Name a host instead.` |
| No `localhost`, `metadata`, `instance-data` or name under `.internal` | Both doors | `Action <name> calls <host>, which names this machine or a cloud metadata service.` |
| No first path segment `_konae`, `oauth`, `.well-known` or `mcp` | Both doors | `Action <name> calls a path under /_konae, /oauth, /.well-known or /mcp, which belong to the Kernel.` |
| Not the installation's public or MCP host | The Kernel, at publish and before each attempt | `Action <name> calls <host>, the editor's host. A call reaches an app or an outside service, never the editor.` |
| `http` only to a host under `KONAE_HOST_SUFFIX` | The Kernel, at publish and before each attempt | `Action <name> calls http://<host>, which is not an app of this installation. Use https.` |
| Every address an outside host resolves to is public | The Kernel, before each attempt | The call fails with `address-refused` |

Before each attempt to a host outside the installation, the Kernel resolves the name and refuses the call when any address is loopback, private, link-local, shared, documentation, benchmarking, multicast, reserved, unspecified, unique-local, or one of those carried in IPv6. It connects to the address it checked, follows no redirect and uses no proxy from the environment. A URL whose host is a Deployment of this installation does not leave the process: the Kernel answers it with the route code an outside request meets, with its callers, signature check, rate limit, guard, Usage and Signal. A host under the suffix that no app holds fails with `app-not-found`, and an app in maintenance answers 503.

### Delivery and the outbox

The worker in `calls.sqlite` sends each call within seconds. An attempt has 10 seconds, and an answer over 64 KiB ends the call with `answer-too-large`. A network failure, a timeout, a DNS failure, 408, 425, 429 or a 5xx is retried after 30 seconds, then 1, 2 and 4 minutes, up to 5 attempts. Any other answer is final. An app queues at most 20 calls in any 60 seconds, or `KONAE_CALLS_PER_MINUTE`; the next is refused with `This app reached its limit of calls in the last minute. Try again in a minute.` An app in maintenance sends nothing until it leaves maintenance.

When a call ends, its `then` mutation runs once, with no actor. Each of its inputs is read by a JSON pointer from this document:

| Pointer | Value |
| --- | --- |
| `/ok` | `true` for a 2xx answer |
| `/status` | The HTTP status, or null when no answer arrived |
| `/error` | The Kernel's error kind, such as `timeout`, `address-refused` or `app-not-found`, or null |
| `/body` | The answer parsed as JSON, or null; `/body/error` is another Konae app's refusal |
| `/input` | The call's own inputs |

A pointer that finds nothing leaves its input out, so it binds NULL when the input is nullable. A string input takes a number, a boolean or a structure as its JSON text. The `then` mutation must be a mutation with no HTTP route and no actor binding, and every required input must be filled. `miniapp.actions.call` refuses it with `Action <name> runs only after the call <call> answers.` It counts in Usage as an action, or under `refused` when its guard refuses. Its `notify` sends as any mutation's does; a notice to `actor` records `no-account`.

`calls_read { "deploymentId", "cursor"? }` needs `read` and lists the app's calls newest first, 50 at a time: the action, `pending`, `delivered` or `failed`, attempts, method, URL, status, error kind, the first 1 KiB of the answer and the `then` mutation's outcome. The outbox keeps the newest 1000 finished calls per app. An export does not carry pending calls. A scheduled copy carries `calls.sqlite`.

### A page that reads another app

`connects` in the project manifest, `manifest.connects` in `konae.json`, such as `["https://desk-4f2a9c.apps.example.org"]`, adds those origins to the page's `connect-src`, so its `fetch` may read another app's public route. A `GET` route that answers anyone answers with `Access-Control-Allow-Origin: *`. A route that takes a key, a session or a signature sends no CORS header, and a request that needs a key is a call action. `connects` lists at most 10 origins, each exactly `https://host` or `https://host:port`, and `http` only under `localhost`.

| Declaration | Message |
| --- | --- |
| Not a list of at most 10 strings | `connects must be a list of at most 10 origins, such as ["https://api.example.com"].` |
| A path, a trailing slash, a wildcard, capitals, the default port, or `http` outside `localhost` | `connects lists <entry>, which is not an origin. Write it as https://host or https://host:port, with no path; http only for a host under localhost.` |
| An origin twice | `connects lists <entry> twice.` |

### Declarations both doors refuse

The Gate and the Kernel parse calls and `connects` from `kernel-rs/kernel/src/call-cases.json`, with these messages besides the URL rules above:

| Declaration | Message |
| --- | --- |
| A query, mutation or signal action with `url`, `method`, `body`, `headers`, `auth` or `then` | `Action <name> is a <kind>, so it cannot declare url, method, body, headers, auth or then. Those keys belong to an action of "kind": "call".` |
| A call with a statement, signal, guard or http | `Action <name> is a call, so it runs no SQL and cannot declare a statement, a signal, a guard or an http route.` |
| A call that binds the actor, the time or a literal | `Action <name> is a call, so its bindings may only be inputs.` |
| A method other than `"GET"` or `"POST"` | `Action <name> must call with "method": "GET" or "POST".` |
| A `body` that is not an object of at most 32 keys, each naming an input or a constant | `Action <name> has an invalid body. Map each body key to one of the call's input keys or to a constant written as an object with "value", at most 32.` |
| A body value that names no input | `Action <name> sends body key <key> from <input>, which is not one of its input bindings.` |
| An `auth` with both or neither of `secret` and `key`, or another key | `Action <name> has an invalid auth. Name a secret with "secret" to sign the request, or with "key" to send the API key it holds.` |
| A secret that is not a secret name | `Action <name> names an invalid auth secret. Use lowercase letters, digits and "_", starting with a letter.` |
| A header the Kernel sets itself, or not 1 to 64 letters, digits and `-` | `Action <name> has an invalid signature header.` |
| A `then` that is not `{ "action", "inputs" }` of strings | `Action <name> has an invalid then. Name a mutation and map its input keys to JSON pointers into the answer.` |
| A pointer outside the document | `Action <name> reads <pointer> for then, which is not a JSON pointer into /ok, /status, /error, /body or /input.` |
| `/input/<key>` for an input the call lacks | `Action <name> reads <pointer> for then, but <key> is not one of its input bindings.` |
| A `then` action that is missing, not a mutation, routed, or binds the actor | `Action <name> names <target> as its then, which is not declared.`, `... which is not a mutation.`, `... which answers on an HTTP route. A then mutation runs only after its call.`, `... which binds the actor. A then mutation runs with no caller.` |
| A `then` input the mutation lacks, or a required one left out | `Action <name> fills <key>, which is not an input of <target>.`, `Action <name> leaves <key> of <target> unfilled. Fill it from the answer or make the input nullable.` |

The editor preview answers a call with `{ callId, queued: true }` and sends nothing. Try calls on `konae dev` or a Deployment.

## Usage and Signals

Each Deployment keeps Usage and Signals in its own `data/usage.sqlite`. Only the owner reads them; the runtime endpoint does not list the tools, so an app cannot read its own Usage. `kernel-rs/kernel/src/usage.rs` implements it, and [ADR-0010](./adr/0010-the-kernel-keeps-usage-and-signals.md) records the decision. The editor's Usage page at `/usage` reads every project the owner holds.

```text
usage_read     { "deploymentId", "from": "2026-11-01", "to": "2026-11-06" }
signals_read   { "deploymentId", "name"?: "order.placed", "cursor"? }
usage_summary  { "deploymentId", "from", "to" }
usage_overview { "from", "to", "cursor"? }
```

All four need `read` and the project grant. Dates are UTC `YYYY-MM-DD`, both included, at most 400 days apart. `usage_read` returns one row per day, kind and name, the totals, and the newest 500 [scheduled runs](#scheduled-work) in the range:

```json
{ "from": "2026-11-01", "to": "2026-11-06",
  "days": [{ "day": "2026-11-06", "kind": "action", "name": "notes.add", "count": 12 }],
  "totals": [{ "kind": "action", "name": "notes.add", "count": 12 }],
  "scheduled": [{ "action": "holds.release", "at": 1762423200000, "outcome": "ok", "changes": 2 }] }
```

| Kind | Name | Counted when |
| --- | --- | --- |
| `session` | | `auth_session` answers. The SDK calls it when an app opens |
| `page_view` | the HTML file served, such as `index.html` | a browser navigation received one of the artifact's HTML pages with 200 |
| `action` | the action | `actions_invoke` runs the action |
| `route` | `POST /hooks/orders` | an HTTP route runs its action, which is not counted again under `action` |
| `upload` | | `files_upload` stores a file |
| `file_read` | | `files_read` or `GET /_konae/files/<id>` returns a file |
| `signup` | | an account becomes active |
| `login` | | a password sign-in or an answered recovery link |
| `refused` | the action | a mutation's [guard](#guards) refused |
| `notify` | `<action>: queued`, or `<action>: <reason>` | a mutation with [`notify`](#notices) changed a row |
| `scheduled` | the action | the timer ran a [scheduled](#scheduled-work) mutation, whether it succeeded or failed |
| `call` | the action | a call action queued a call |

Nothing else the Kernel refuses is counted. The Kernel writes Usage off the request path, and the read tools wait up to five seconds for the queue to drain, so they include every call that finished before them.

`usage_summary` returns the counters, the published actions, daily execution and Signal totals by origin (`runtime`, `route`, `schedule`, `continuation`) and outcome (`ok`, `refused`, `failed`, `unchanged`), the states of retained calls and email, and `coverage`, which says whether queued records finished writing and how far back the store reaches. `usage_overview` summarizes up to 50 projects per page with `nextCursor`; an unpublished project has `summary: null`. Neither returns Signal properties or request bodies.

A Signal is a named occurrence a Server Action declares. It is recorded when the action succeeds, and for a mutation only when its statement changed a row, with properties from the input:

```json
"newsletter.join": {
  "kind": "signal",
  "access": "anonymous",
  "bindings": [{ "source": "input", "key": "source", "type": "string", "maxLength": 40 }],
  "signal": { "name": "newsletter.joined", "properties": ["source"] }
}
```

The property `actor` records the caller's ID: the app user's, the API key's, or `null` for a visitor or a signed request. An action of `"kind": "signal"` runs no SQL and returns `{ "recorded": true }`. The app records a Signal with `miniapp.actions.call`, through the same access check, input check and rate limit as any action. `signals_read` returns 100 at a time, newest first, as `{ "signals": [{ "name", "at", "properties" }], "nextCursor" }`.

| Refusal | Message |
| --- | --- |
| A Signal on a query | `Action <name> is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action.` |
| A property that is not an input binding's key | `Action <name> names signal property <key>, which is not one of its input bindings.` |
| A property listed twice | `Action <name> names signal property <key> twice.` |
| `actor` listed while an input is named `actor` | `Action <name> names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input.` |
| More than 8 properties | `Action <name> names more than 8 signal properties.` |
| A name that is not dot-separated lowercase words of up to 64 characters | `Action <name> names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters.` |
| `signal` that is not `{ name, properties? }` | `Action <name> has an invalid signal declaration.` |
| A signal action without `signal` | `Action <name> is a signal action, so it must declare a signal.` |
| A signal action with a statement | `Action <name> is a signal action, so it runs no SQL and cannot declare a statement.` |
| A signal action with `http` | `Action <name> is a signal action, so it takes no route. To record a signal from an HTTP route, declare the signal on a mutation with a POST route.` |
| `usage_read` with a date that is not `YYYY-MM-DD` | `from and to must be dates written YYYY-MM-DD.` |
| `usage_read` with `from` after `to` | `from must not be after to.` |
| `usage_read` over more than 400 days | `Usage covers at most 400 days at a time.` |
| `signals_read` with a cursor it did not return | `Invalid cursor. Pass back the nextCursor of an earlier page.` |

The Gate and the Kernel run the cases in `kernel-rs/kernel/src/signal-cases.json`. The store's bounds are on [the limits page](./limits.md#usage-store). An export and a scheduled copy carry it. The editor preview keeps no store.

### Guards

A mutation writes its rule into its `WHERE`. When the rule says no, the statement changes no row, and without a guard the caller gets `{ "changes": 0, "lastInsertId": 0 }`. A guard turns that into a refusal:

```json
"members.promote": {
  "kind": "mutation",
  "access": "authenticated",
  "guard": "Only an organizer can promote a member.",
  "statement": "UPDATE members SET role = 'organizer' WHERE user_id = ?1 AND EXISTS (SELECT 1 FROM members WHERE user_id = ?2 AND role = 'organizer')",
  "bindings": [{ "source": "input", "key": "member", "type": "string", "maxLength": 64 }, { "source": "actor", "field": "id" }]
}
```

When the statement changes no row, `actions_invoke` answers a tool error with the guard's text, so `miniapp.actions.call` rejects with it, and the route answers `403 { "error": "<guard>" }`. Usage counts it under `refused` and no Signal is recorded. The statement ran and changed nothing, so nothing is rolled back. The editor preview refuses the same way, and `conform` probes the guard.

| Declaration | Message |
| --- | --- |
| `guard` on a query | `Action <name> is a query, so it cannot declare a guard. Declare it on a mutation.` |
| `guard` on a signal action | `Action <name> is a signal action, so it cannot declare a guard. Declare it on a mutation.` |
| A guard that is not 1 to 200 characters on one line | `Action <name> has an invalid guard. Use a message of 1 to 200 characters on one line.` |

The cases are in `kernel-rs/kernel/src/guard-cases.json`.

### Scheduled work

A mutation that takes no input may declare when the Kernel's own timer runs it ([ADR-0011](./adr/0011-the-kernel-runs-a-declared-schedule.md)):

```json
"holds.release": {
  "kind": "mutation",
  "access": "authenticated",
  "schedule": { "every": "15m" },
  "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL AND until < ?1",
  "bindings": [{ "source": "kernel", "field": "now" }]
}
```

`"every"` takes whole minutes or hours from `"5m"` to `"24h"`, counted from 00:00 UTC, so `"15m"` runs at :00, :15, :30 and :45. `"daily": "HH:MM"` runs once a day at that UTC time. The Kernel looks once a minute and runs an action at the first look after its period opens. `kernel.now` is the time of the run and an `actor` binding is null. `actions_invoke` and the editor preview refuse a scheduled action with `Action <name> runs only on its schedule.` A scheduled mutation that also declares an `http` route answers there as its callers say.

| Rule | What the Kernel does |
| --- | --- |
| Once a period | The period it last ran for is kept in `usage.sqlite`, which a restart, a copy and an export carry. A Kernel that was down for several periods runs the action once |
| Publishing | The first period an action is seen in counts as done, so publishing never runs it |
| One run at a time | A run takes the Deployment's data lock and the 2-second statement budget. Scheduled actions run in name order |
| Maintenance | An app in maintenance runs nothing, and runs each due action once when it leaves maintenance |
| Failure | A failed run is logged, recorded with `outcome: "failed"` and its error, and tried at the next look |
| Guard and Signal | A run that changes no row is recorded with `changes: 0`, and the guard does not apply. A run that changes a row records the Signal with `actor` null |
| Notice | A run that changes a row sends its `notify`. `to: "owner"` sends to the app's contact email once per run. `to: "actor"` records `no-account` |
| Usage | Each run counts under `scheduled`, and `usage_read` returns the newest 500 runs with `action`, `at`, `outcome`, `changes` and, for a failed run, `error` |

The editor preview runs no schedule; `konae dev` does. `KONAE_SCHEDULE_MIN_SECONDS` runs every schedule every few seconds for a test, and the Kernel refuses it unless `KONAE_BIND` is a loopback address. Nothing is pushed to a page, so a page polls a query with an `after` cursor.

| Declaration | Message |
| --- | --- |
| `schedule` on a query | `Action <name> is a query, so it cannot declare a schedule. Declare it on a mutation.` |
| `schedule` on a signal action | `Action <name> is a signal action, so it cannot declare a schedule. Declare it on a mutation.` |
| `schedule` on a mutation with an input binding | `Action <name> declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values.` |
| A schedule that is not one of `every` and `daily`, such as `"0 8 * * *"` | `Action <name> has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00".` |
| `every` below 5 minutes | `Action <name> runs every 4m, more often than every 5 minutes. Use "5m" or longer.` |
| `every` above 24 hours | `Action <name> runs every 25h, less often than once a day. Use "daily" or a period of 24 hours or less.` |
| `daily` that is not a time from `00:00` to `23:59` | `Action <name> has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59.` |
| `subscribe` on any action | `Action <name> declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead.` |

The cases are in `kernel-rs/kernel/src/schedule-cases.json`.

### Notices

A mutation may send one email when its statement changes a row. `kernel-rs/kernel/src/notify.rs` implements it, and [ADR-0012](./adr/0012-email-leaves-through-one-queue-to-addresses-the-app-has-proved.md) records the decision.

```json
"bookings.create": {
  "kind": "mutation",
  "access": "authenticated",
  "statement": "INSERT INTO bookings (user_id, room, day) VALUES (?1, ?2, ?3)",
  "bindings": [{ "source": "actor", "field": "id" }, { "source": "input", "key": "room", "type": "string", "maxLength": 40 }, { "source": "input", "key": "day", "type": "string", "maxLength": 10 }],
  "notify": { "to": "actor", "subject": "Booked: {input.room} on {input.day}", "body": "Your booking of {input.room} on {input.day} is confirmed." }
}
```

`to` is `owner`, the app's [contact email](#contact-email), or `actor`, the calling account's email once that account answered a verification, invitation or recovery link. The Kernel never sends to an address from the input. The placeholders are `{input.<key>}` for an input binding, `{actor.id}`, `{kernel.now}` as `2026-11-08 07:00 UTC`, `{changes.count}` and, in the body only, `{changes}`. A value loses its line breaks and keeps 200 characters.

`{changes}` lists the changed rows, one line each, with the columns named in `"changes"`. It lists at most 50 lines, then `- and <n> more`. `"to": { "column": "assignee_id" }` sends one notice to each account whose ID a changed row holds in that column, listing only that account's rows. The Kernel reads the rows back by appending its own `RETURNING`, and the answer stays `{ changes, lastInsertId }`. One run reads at most 1,000 rows and writes to at most 50 accounts.

```json
"tasks.remind": {
  "kind": "mutation", "access": "authenticated", "schedule": { "daily": "07:00" },
  "statement": "UPDATE tasks SET reminded_at = ?1 WHERE due_day = date(?1 / 1000, 'unixepoch') AND reminded_at IS NULL",
  "bindings": [{ "source": "kernel", "field": "now" }],
  "notify": { "to": { "column": "assignee_id" }, "changes": ["title"],
    "subject": "{changes.count} tasks due today", "body": "Due on {kernel.now}:\n{changes}" }
}
```

The body keeps the template's line breaks, the HTML part escapes every value, and the Kernel adds a last line naming the app and why the recipient gets it. A guard's refusal, an `ON CONFLICT DO NOTHING` that stored nothing and a failed statement send nothing, and the mutation's answer never depends on the notice. Usage counts every notice under `notify`, as `bookings.create: queued` or with the reason:

| Reason | What happened |
| --- | --- |
| `no-transport` | The installation has no mail transport. Nothing is stored |
| `owner-has-no-email` | `to` is `owner` and the app has no contact email |
| `no-account` | `to` is `actor` and the caller is a visitor, a signed request, an API key or a timer |
| `unverified-email` | `to` is `actor` and the account has not answered a link, or is not active |
| `not-a-verified-account` | A row's value in the `to` column is not an active account of this Deployment that answered a link |
| `recipient-cap` | The run already wrote to 50 accounts |
| `app-limit`, `creator-limit`, `recipient-limit`, `installation-limit` | A day's budget is used up. Nothing is stored |
| `sender-not-ready` | The sender preflight was not ready. The job is stored as failed and never sent |
| `storage-error` | The queue could not be written. The Kernel logs it |

A queued notice is a job in the account email queue with purpose `notify`, held while its app is in maintenance, and failed as `notice-expired` when it has not left within a day. `notices_read { "deploymentId", "cursor"? }` needs `read` and lists the app's notice jobs newest first, 50 at a time, with the action, the state, attempts, the failure kind and the times; it names no recipient. The editor's What happened panel reads `usage_summary`, `calls_read`, `notices_read` and `signals_read`. Notices count apart from account email, 200 a day per app, 500 per owner and 50 per recipient from one app, and the installation's 1,000 a day counts every email.

| Declaration | Message |
| --- | --- |
| `notify` on a query | `Action <name> is a query, so it cannot declare notify. Declare it on a mutation.` |
| `notify` on a signal action | `Action <name> is a signal action, so it cannot declare notify. Declare it on a mutation.` |
| Not an object of `to`, `subject` and `body`, or another key such as `cc` | `Action <name> has an invalid notify declaration. Give "to", "subject", "body" and, to list changed rows, "changes", and nothing else.` |
| `to` that is not `owner`, `actor` or `{ "column": "<name>" }` | `Action <name> has an invalid notify recipient. Use "owner", "actor" or {"column": "<name>"} for a column of account IDs.` |
| `changes` that is not 1 to 8 different column names | `Action <name> has an invalid notify changes list. Name 1 to 8 columns of the changed table, each once, in letters, digits and underscores.` |
| A subject that is blank, over 120 characters or holds a line break | `Action <name> has an invalid notify subject. Use 1 to 120 characters on one line.` |
| A body that is blank, over 2000 characters or holds a control character other than a line break | `Action <name> has an invalid notify body. Use 1 to 2000 characters, with line breaks and no other control characters.` |
| An unknown placeholder, such as `{actor.email}` | `Action <name> names an unknown placeholder {<placeholder>} in its notify <subject or body>. Use {input.<key>} for one of its input bindings, {actor.id}, {kernel.now}, {changes} or {changes.count}.` |
| `{changes}` in the subject | `Action <name> puts {changes} in its notify subject. Put {changes} in the body, and {changes.count} in the subject.` |
| `{changes}` in the body with no `changes` list | `Action <name> uses {changes} in its notify body without a changes list. Name the columns to show in notify "changes".` |
| A `changes` or `to` column the changed table lacks | `Action <name> names a notify column its statement cannot return. Name columns of the table the statement changes.` |
| A `{` with no `}` after it | `Action <name> has an unclosed placeholder in its notify <subject or body>. Close it with "}".` |

The cases are in `kernel-rs/kernel/src/notify-cases.json` and `kernel-rs/kernel/src/notify-column-cases.json`.

## Build and publish

```sh
bun run konae -- build --dir /tmp/my-app
bun run konae -- publish --dir /tmp/my-app
```

`publish` sends `dist/` and the text files of `app/` over MCP. It uploads a transfer, calls `release_plan`, which checks the release against the project's revision, then `release_apply`, and saves the plan and the receipt in `.konae/`. A stale plan is refused, and repeating an applied plan returns its receipt. A republish keeps the address and the database and stops serving files the new artifact lacks. [The CLI](./cli.md#publish) has the flags, and the `publishing` topic of `platform_guide` the same steps as tool calls.

A published app runs under the Kernel's Content Security Policy ([the architecture](./architecture.md#isolation-of-hosted-code)). The inline scripts of each HTML page run because the Kernel allows each by its hash. Inline event handler attributes, scripts from other origins and requests to other origins do not.

`signupPolicy` in `konae.json` names who may create an app account when the first release goes out, and `publish` sends it as `initialSignupPolicy`. `init` writes `"visibility": "private"` and `{ "mode": "invitation" }`. On a later release `publish` sends no policy; when the app's policy differs from the file, it prints one warning and still publishes, such as `konae.json asks for signup Open, but this app has Invitation only. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Auth > Users view, or call signup_policy_set with a credential that holds manage_users.`

### The schema change

`release_plan` loads `schema.sql` into a scratch database, compares tables, columns and indexes with a copy of the live database, and answers the schema change under `schema`:

```json
{ "statements": ["DROP TABLE \"notes\""],
  "losses": [{ "table": "notes", "column": null, "sentence": "Drops table notes and its 2 rows." }],
  "confirmation": "<64 hex characters>" }
```

A new table, column or index applies and keeps every row, with `losses` empty and `confirmation` null. A dropped table, a dropped column or a changed column type fills `losses`, and `release_apply` runs it only with `confirmSchema` set to that `confirmation`. Without it, or with another plan's, `release_apply` refuses with `schema.confirmation-required` and changes nothing. Where SQLite cannot change a table in place, the Kernel rebuilds it in one transaction, copying every column that still exists, and checks that no row breaks a foreign key. A copy that a new constraint refuses, such as a NOT NULL column without a default on a table with rows, stops the release. A renamed column is a dropped column and a new one. The editor shows the losses before it publishes, and `publish` and `dev` print them and ask.

`schema.sql` may only create tables and indexes and must be safe to run twice. A CHECK constraint, an expression or partial index and a generated column may call SQL functions except `load_extension`, `readfile`, `writefile`, `edit` and `fts3_tokenizer`. Applying it caps each value at 2 MiB and the whole schema at 30 seconds.

### What the build publishes

`konae build`, the editor's Publish and an app folder's `npm run build` run one set of rules, `src/lib/static-artifact.js`, so the same Capsule gives the same artifact from every door.

- An npm package is imported by name once `package.json` lists it under `dependencies` with an exact version, such as `"preact": "10.24.3"`. The editor's build fetches it from esm.sh; `konae build` and `npm run build` read it from `node_modules`, so run `npm install` first. The package goes into the bundle, and the published app loads nothing from another origin. A version range, a `node:` module or a bare import the list lacks fails the build with a sentence that names the fix.
- A page is every `.html` file at the Capsule root and under `pages/`. `pages/guide/install.html` answers at `/guide/install.html` and `/guide/install`.
- Every file under `public/` is served as it is, without the prefix: `public/robots.txt` answers at `/robots.txt`.
- Any other Capsule file ships only when a page, or a stylesheet a page loads, names it: `<script src>`, `<link href>` with rel `stylesheet`, `icon`, `apple-touch-icon`, `mask-icon`, `manifest`, `preload`, `modulepreload` or `prefetch`, `src`, `srcset` and `poster` on media elements, and `url()` and `@import` in CSS. Links, other origins, `data:` URLs and paths under `/_konae` are not followed. `schema.sql`, `actions.json` and every file no page names stay out.
- A page with no doctype and no `<html>` is a fragment, which the build puts in the Capsule document with `styles.css`, the app bundled from `src/main.ts` or `app.js`, and the SDK. A page that is a whole document keeps its own head, and the build loads `/runtime.js` first in it. A `<script type="module" src>` is bundled with its imports.
- A fragment page names its title and icon by opening with a `<title>` and any icon `<link>`, which the build moves into the head.
- A Capsule without `index.html` gets the fragment `<main id=app></main>`.
- The same Capsule built from two folders gives byte-identical files, because the build drops the comments that name the build folder.
- A binary file travels as base64 and the artifact names it in `encodings`, such as `"encodings": { "logo.png": "base64" }`. The editor's saved sources hold text files only, so an icon that must work from every door is SVG, as a file or a `data:` URL.

| Refusal | When |
| --- | --- |
| `about.html references /team.jpg, but the Capsule holds no team.jpg.` | A page or stylesheet names a file the Capsule lacks |
| `guide/a.html references ../../x.js, which leaves the Capsule.` | A reference climbs above the Capsule root |
| `about.html and pages/about.html would both publish at /about.html. Keep one.` | Two files land on one path |
| `public/_konae/x.txt would publish at /_konae/x.txt, under /_konae, which the Kernel keeps.` | A file lands under `/_konae`, `/oauth`, `/.well-known` or `/mcp` |
| `The Capsule holds runtime.js, the name the build gives the Konae SDK. Rename it.` | The Capsule has its own `runtime.js` |
| `index.html loads /src/main.ts as a module. A page loads JavaScript, so name a .js or .mjs file and let the build compile it.` | A module script names a file that is not `.js` or `.mjs` |
| `app.js imports CSS. Link the stylesheet from the page instead.` | A document page's module imports a stylesheet |
| `Routing must be "pages" or "spa".` | `routing` has another value. `release_plan` refuses it with the same words |

### How the Kernel answers a path

`manifest.routing` in `konae.json` chooses how a path the artifact does not hold is answered. It is `"pages"` when absent; use `"spa"` for an app that routes in the browser.

| Request | `"pages"` | `"spa"` |
| --- | --- | --- |
| A file the artifact holds | The file with its content type | The same |
| `/about`, with no extension | `about.html` when the artifact holds it | `index.html` |
| `/guide/` | `guide/index.html` when the artifact holds it | `index.html` |
| Anything else | 404 with the artifact's `404.html`, or a small Kernel page | `index.html` with 200 |
| A path under `/_konae` | Never an app file | Never an app file |

An HTTP route wins over an artifact file at its path for its method. A file the Kernel does not know the type of is `application/octet-stream`, and every file carries `X-Content-Type-Options: nosniff`. A file sent with 200 carries `Cache-Control: public, no-cache` on a public Deployment and `private, no-cache` on a private one, with an `ETag` of the artifact version, so the Kernel answers 304 until the next publish. Every HTML page gets the Content Security Policy with the hashes of its own inline scripts. The artifact limit counts the bytes stored after base64 is decoded.

## Custom domain

A Deployment can answer on one hostname its owner controls, such as `notes.example.org`, next to its generated hostname, which keeps answering ([ADR-0014](./adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md)).

1. Point the hostname at the installation's proxy, with a CNAME or an A record. The proxy terminates TLS for it and forwards `Host` unchanged. Konae issues no certificate.
2. Call `domain_set { deploymentId, hostname }`, which needs `publish`. It answers with the record to add:

   ```json
   { "deploymentId": "…", "hostname": "notes.example.org", "status": "pending",
     "url": "https://notes.example.org", "generatedUrl": "https://notes-1a2b3c.apps.example.com",
     "record": { "type": "TXT", "name": "_konae.notes.example.org", "value": "konae-verify-3f0c…" },
     "dns": "No TXT record at _konae.notes.example.org yet.",
     "next": "Add a TXT record _konae.notes.example.org holding konae-verify-3f0c…, then call domain_set again with notes.example.org." }
   ```

3. Add the TXT record and call `domain_set` again with the same hostname. The Kernel looks the record up over UDP, from `KONAE_DNS_RESOLVER` or the first `nameserver` in `/etc/resolv.conf`. When one of its values is the token, the answer is `status: "active"` and the alias serves.

`bun run konae -- domain --deployment <id> --set <hostname>` runs steps 2 and 3. On the alias the app has its own origin. Cookies are host-only, so a session made there stays there, and `'self'` in the Content Security Policy is the alias. Account email links use the alias once it serves. `domain_read { deploymentId }` returns the state and the installation's `rules`, and `domain_clear { deploymentId }` removes the alias and its token. An app in maintenance refuses both changes. The Kernel checks DNS only when `domain_set` runs.

| Refusal | Message |
| --- | --- |
| A name that is not a DNS name, such as a URL, a port, `_` or `*` | `<name> is not a valid DNS name. Use labels of letters, digits and hyphens, 1 to 63 characters each, that do not start or end with a hyphen.` |
| One label, an IP address, a local name | `<name> needs at least two labels, such as notes.example.org.` / `<name> is an IP address. Name a hostname under a domain you control.` / `<name> is a local name. …` |
| A name under the installation's suffix | `<name> is under this installation's app suffix <suffix>, where Konae names apps itself. Name a hostname outside it.` |
| The editor's or the MCP host | `<name> is this installation's own host. Name a hostname no Konae host uses.` |
| A TXT record that holds another value | `The TXT record at _konae.<name> holds "<value>", not this Deployment's token. Set it to <token> and call domain_set again.` |
| A name another Deployment serves | `<name> already serves another Deployment on this installation. Clear it there with domain_clear first.` |

An export carries the hostname without its token. The import creates a pending claim with a token the destination issues, and `domain_read` there prints the record to add. `kernel-rs/kernel/src/domain-cases.json` lists every rule.

## Path address

A Deployment can answer at `<domain>/p/<id>`, and at `<domain>/p/<name>` once named, with no DNS of its own. `KONAE_DOMAIN` names the domain.

1. `path_set { deploymentId, enabled: true, name?, acceptSharedOrigin? }` turns the path on and needs `publish`. A name is 3 to 40 letters, digits and hyphens, does not start or end with a hyphen, is not a word Konae uses such as `app` or `mcp`, and does not look like an ID. No other Deployment may hold it while its path is on.
2. An app with accounts needs `acceptSharedOrigin: true`, or the call refuses with `path.shared-origin`. An app has accounts when it is private, when its signup is not Closed, when it declares `fileAccess: "app-users"`, or when an action is not `anonymous`. Every path address shares the origin `<domain>`, so any other app there can use the sessions of this app's users.
3. `path_read { deploymentId }` returns `enabled`, `name`, `acceptSharedOrigin` and the `addresses`. Read it back after each change.
4. `path_set` with `enabled: false` turns the path off and frees the name. `path_clear` also removes the name and the acceptance.

The app whose path holds the name `konae-home` also answers `<domain>/`. An omitted `name` or `acceptSharedOrigin` keeps the stored value, and `name: null` removes the name. Without `KONAE_DOMAIN`, `path_read` answers `domainConfigured: false` and `path_set` with `enabled: true` refuses. An export carries `enabled`, `name` and `acceptSharedOrigin`. On import a name another app holds is dropped, and the receipt's `notices` says so. `kernel-rs/kernel/src/path-cases.json` and `kernel-rs/kernel/src/path-state-cases.json` list the rules.

## Action Contract

```sh
bun run konae -- contract --dir /tmp/my-app
bun run konae -- conform --url http://my-app-ab12.localhost:8796 --contract /tmp/my-app/action-contract.json --token <app session token>
```

`contract` reads `app/actions.json` and `app/schema.sql` and writes `action-contract.json`, its JSON Schema, `action-contract.md` and `actions.d.ts` beside `konae.json`, or into `--out`. `dev` writes `actions.d.ts` on every start and change, and the editor shows `action-contract.md` in Files, read-only. `actions.d.ts` types `miniapp.actions.call` by action name, input and returned columns, and leaves out actions a page cannot call: scheduled mutations, `then` mutations and actions whose route is their only door.

For each Server Action the contract states:

| Field | What it holds |
| --- | --- |
| `kind`, `access`, `actorBound` | The kind, the access, and whether a binding passes the caller's ID |
| `inputs` | Each input with its type, whether it is required or nullable, and a string's `maxLength` |
| `output` | The columns a query returns with their SQLite affinity, nullability and source column, or `changes` for a mutation |
| `signal` | The Signal's name and the input keys in its properties |
| `guard` | The refusal it answers when its statement changes no row |
| `runtime` | The endpoint, `/_konae/mcp`, the `actions_invoke` tool and the callers it accepts |
| `http` | The route's method, path, input reading, signature, callers and `onlyDoor` |
| `errors` | The exact messages the Kernel answers for this action, and the HTTP status on a route |
| `limits` | The top-level `LIMIT`, the 1000-row and 2 MiB cap, the 2-second deadline, the rate limits and a route's 64 KiB body |
| `statement`, `bindings` | The SQL and its binding order, so the contract converts back to the same `actions.json` |

An expression column without `AS`, such as `count(*)`, is listed under `unnamed`, and `contract` and the Gate warn about it. Types come from `schema.sql`, which SQLite does not enforce.

`conform` is what a replacement backend runs. It calls every action through the runtime endpoint with generated inputs and checks the answer's shape, then probes each refusal the contract lists: undeclared actions and inputs, missing, mistyped, empty or overlong inputs, callers without a session, and on a route every signature, key and body refusal. A guarded mutation must answer its guard when its statement changes no row. A refusal passes only when the contract lists the message and the backend answers with it. `--token` carries an app session token, or `KONAE_APP_EMAIL` and `KONAE_APP_PASSWORD` sign in for one. `--secret <name>=<value>` signs a route and may repeat. `--key` sends an API key. `--inputs <file>` gives real inputs keyed by action name, such as `{ "items.jsonin": { "ids": "[1, 2]" } }`, for a statement that reads a string as more than text. A check that needs a credential it was not given is reported as skipped. `conform` runs every mutation once, so point it at a test Deployment. It exits with 1 when any check fails.

## Export and import an app

`project_export { deploymentId }` is the one export of an app and needs `export_data`. It hands over the app folder: `folder`, one download of a ustar archive with `konae.json`, `app/` and `dist/`, and one download per part of `data/`, captured under the app's locks so no write lands halfway. `sourcesAhead` is true when `app/actions.json` differs from the actions the release runs. Secrets, owners, credentials, sessions and `.konae/` never leave.

```json
{
  "manifest": {
    "format": "konae-package", "version": 1, "packageId": "…", "createdAt": 1762430000000,
    "deploymentId": "…", "projectId": "…", "visibility": "private",
    "signupPolicy": { "mode": "closed", "allowlist": [] },
    "publishedRevision": 4, "artifactVersion": "v…", "sourcesRevision": 9,
    "contents": [
      { "name": "database", "size": 268435456, "sha256": "…" },
      { "name": "identity", "size": 110592, "sha256": "…" },
      { "name": "files", "size": 20987904, "sha256": "…" },
      { "name": "usage", "size": 61440, "sha256": "…" }
    ],
    "excluded": ["app sessions", "verification, recovery and invitation links", "…"]
  },
  "downloads": { "database": { "transferId": "…", "size": 268435456, "sha256": "…", "chunkBytes": 65536, "expiresAt": 0 } },
  "folder": { "transferId": "…", "size": 40960, "sha256": "…", "chunkBytes": 65536, "expiresAt": 0 },
  "sourcesAhead": false
}
```

| Component | What it is | Limit |
| --- | --- | --- |
| `database` | The app database, copied with `VACUUM INTO` | 256 MiB |
| `identity` | `identity.sqlite` without sessions or email links, with every API key revoked at import | 64 MiB |
| `files` | A ustar archive of `index.json` and one `<id>.blob` per file | The destination's per-file limit and file quota |
| `usage` | `usage.sqlite`, present when the app recorded Usage or a Signal | 128 MiB |

`publishedRevision` counts the releases applied, and `sourcesRevision` the editor's saves. Read each download with `transfer_read` until `nextOffset` equals `size`, check its SHA-256, and cancel it. The downloads expire after an hour and need `export_data`. The audit log gets one `project_export` row and one `package_download` row per download.

To import into a new app on another installation, create the project there with `project_create` and the same `projectId`, and set every signing secret with `secret_set`. Upload each component with `transfer_begin { size, sha256, projectId, component }`, so the upload may reach that component's limit; `McpClient.upload(bytes, projectId, component)` in `src/mcp/client.ts` does the three calls. Then send the release payload, the one `release_plan` always takes, with `initialPackage`:

```json
{ "project": {}, "visibility": "private", "artifact": {},
  "initialPackage": { "manifest": {}, "components": { "database": "transferId", "identity": "transferId", "files": "transferId" } } }
```

`release_plan` checks, before any Deployment exists:

- the manifest's format and version, and every component's limit;
- each upload is complete, belongs to the caller and the project, was started for that component and matches the manifest's size and SHA-256;
- every SQLite file passes `integrity_check`, and the accounts file has `app_users`;
- every file has its bytes at the indexed size, within the quotas, and every file owner is an account in the export;
- the volume has room for the components, a copy of the database for the schema check and 64 MiB to spare;
- the release's `schema.sql` and actions run against the imported database.

`release_apply` copies each component into the new app folder's `data/`, checking its SHA-256, deletes sessions and email links, revokes live API keys and unpacks the files. Pending invitations arrive without their links, so each needs `app_invitation_resend` on the destination. A failed apply can be retried with a new plan, and repeating an applied plan returns its receipt. The Signup Policy comes from the manifest unless the payload sets `initialSignupPolicy`. [The walkthrough](./mcp-walkthrough.md#14-import-on-a-second-kernel) shows the calls.

| Refusal | Message |
| --- | --- |
| Export without `export_data` | `Unknown tool or insufficient permission. project_export needs a credential with export_data. Run konae login with --export-data, or use a token from token_create that includes it.` |
| Reading an export's download without `export_data` | `Permission denied for export_data.` |
| `initialPackage` for a project that has a Deployment | `Portable state can only be imported on the first deployment.` |
| Upload bytes that differ from the declared SHA-256 | `Upload size or SHA-256 mismatch.` |
| An upload that differs from the manifest | `The database component does not match the size and SHA-256 in the manifest.` |
| Another manifest version | `Package version 2 is not supported. This Kernel reads version 1.` |
| A custom domain another Deployment on the destination serves | `Package import failed: <name> already serves another Deployment on this installation. …` |
| A component over its limit | `Transfer size is outside the limit of 67108864 bytes for the identity component.` |
| Not enough disk at `transfer_begin`, `project_export` or `release_plan` | `Not enough free space on the Kernel volume to import this app: 340 MiB needed, 120 MiB free.` |
| A database that holds a trigger or a view | `The database holds CREATE TRIGGER <name>, and Konae runs no triggers or views. Drop it before importing.` |

### Import into an existing app

`project_import { deploymentId, replace, transferId?, package?, confirmSchema? }` replaces a published app with an exported folder and needs `publish`. `replace` is `sources`, `data` or `both`:

- `sources` takes `transferId`, an upload of the release payload built from the folder's `app/` and `dist/`, publishes it and makes it the sources the editor opens.
- `data` takes `package { manifest, components }`, uploaded as for a new app, and replaces the database, the accounts, the files and Usage. The database takes the live schema.
- `both` takes both, and the database takes the release's schema.

It never merges rows. It first writes a backup of what it replaces, which `database_execute` operation `list-backups` lists as `Before import`, with `holds` naming `data`, `sources` or both, and `restore-backup` brings back. The app keeps its Signup Policy, sign-in methods, addresses, secrets, sessions and API keys. A schema change that loses data answers `{ imported: false, replace, schema }` and changes nothing; call again with `schema.confirmation` as `confirmSchema` once the owner agrees. Otherwise the answer is `{ imported: true, replace, backup, schema }`. `project_import` works while the app is in maintenance. In the editor, App settings has an Import section that reads the `.tar` that Export downloads.

`bun run konae -- export --dir <folder> --out <empty folder> --with-data` writes the same app folder on disk. `bun run konae -- pull` exports the data into `.konae/pull/` and `dev --reset` imports it into the local folder, as [the CLI](./cli.md#pull) describes. On a `kernel` binary without Konae, `kernel import <folder>` replaces the app's data after a backup.

## Contact email

`"notify": { "to": "owner" }` sends to the app's contact email, `contactEmail` in `konae.json`, so an export carries it and a `kernel` binary sends there too. A first publish from Konae fills it with the owner's email when Konae knows it; an owner who signs in through Clerk has none in Konae and sets it. `konae publish` sends the one `konae.json` names.

`contact_email_set { deploymentId, email }` sets it, or clears it with `email: null`, and needs `publish`. `contact_email_read { deploymentId }` answers `{ deploymentId, contactEmail, ownerNotices }`, where `ownerNotices` names the actions that send to it. With `contactEmail` null and `ownerNotices` listed, those notices are not sent and Usage counts each as `owner-has-no-email`. App settings in the editor has the same field.

## Maintenance

An app in maintenance takes no writes. Its host answers a page request, or any `GET` outside `/_konae/`, with a 503 maintenance page, and every other request with 503, `Retry-After: 300` and:

```json
{ "error": "This app is in maintenance. It accepts no writes, signups, recovery, administrative changes or email until it comes out of maintenance.", "maintenance": true }
```

That covers actions, uploads, file reads, signup, sign-in, recovery, invitations, HTTP routes and the runtime endpoint. Owner tools that would change the app answer with the same message. Tools ending in `_list` or `_read`, `project_export`, `release_plan`, `database_execute` with `inspect`, `browse` or `list-backups`, `project_import` and `maintenance_set` keep working. The app's email, calls and schedules wait.

`maintenance_set { deploymentId, on }` needs `publish`. `maintenance_read { deploymentId }` answers `{ maintenance, since, unreadable, installation }`, where `installation` names a hold on the whole installation after a restore, which taking the app out does not lift. The state is `apps/<id>/.konae/maintenance.json`, which a restart and a copy keep. App settings in the editor has the same switch.

An owner moves an app with no lost write in three steps: put it in maintenance, export it, and import the folder on the other installation.

`app_user_suspend`, `api_keys_revoke` and `konae creator-account-suspend` write each change to every copy target before they answer, so a restore cannot bring back what was revoked. When a target cannot be written the tool answers `The change holds on this server, but it could not be recorded at the copy target, so a restore could undo it …`, and repeating the request records it. [Operate an installation](./operate.md#data-directory-and-copies) covers copies and restore.

## Transfers

A client starts a transfer by declaring the size and SHA-256, writes chunks of at most 64 KiB in base64, and completes it, which checks the size and hash and freezes the bytes. The server accepts an identical retry at the same offset and refuses different content. `transfer_status` lets a client resume from the last offset. Cancel transfers you no longer need. [The limits page](./limits.md#transfers-and-publishing) lists how many a caller holds, how long they last and how large they may be.

The audit log in Konae's `mcp.sqlite` records each owner tool call with its project or Deployment, and never an email, a token or a secret's value. An export download or a component upload writes one row at its first block, not one per chunk. The log keeps the newest 10,000 rows per owner and drops rows older than 30 days. For app users the Kernel records only an upload that stores a new file and a delete that removes one.
