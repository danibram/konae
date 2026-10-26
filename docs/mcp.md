# Develop with authenticated MCP

Littlebox exposes its operations through MCP over Streamable HTTP. The editor, the app SDK and external agents use that protocol. The supported revision is `2025-11-25`.

## Startup and owner account

```sh
bun install
bun run dev:local
```

Open the editor and go to **Platform access**. The direct link is `http://localhost:4173/_littlebox/owner/login`. To create the first owner, enter an email, a password and the setup key. In the default local environment that key is `local-development-key`; `LITTLEBOX_ADMIN_KEY` sets it. The first account claims the Deployments from the earlier prototype. Every later creator enters an invitation code instead, described under [creator admission](#creator-admission).

Each owner administers their own projects. The page creates read-only or read, edit and publish tokens, optionally limited to specific projects, and lists and revokes active credentials. A token's secret appears only once. Tokens from this page expire after 24 hours; `token_create` accepts any subset of `read`, `write`, `publish`, `manage_users`, `export_data` and `manage_lifecycle` and a lifetime of up to 30 days, and `littlebox login` asks for `read`, `write` and `publish` plus one permission per flag, `--manage-users`, `--export-data` and `--manage-lifecycle`. OAuth clients may request `read`, `write`, `publish` and `manage_users`; the consent screen does not offer `export_data` or `manage_lifecycle`, and a request for either answers `Invalid scope.` with the scopes it takes. Only `export_data` downloads an app's accounts and data, only `manage_lifecycle` ejects or migrates a project, and the owner's browser session holds both.

The setup key does not authenticate MCP tools and is not included in the editor bundle. The server listens on `127.0.0.1` by default. The Kernel refuses to start on a non-loopback address, or with a `LITTLEBOX_PUBLIC_ORIGIN` whose host is not `localhost`, a `*.localhost` name or a loopback IP, while the setup key is unset or still `local-development-key`, unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`. For a remote installation, configure the setup key, the HTTPS origin and the proxy before listening on another interface.

## Connect an agent

Configure the endpoint `http://localhost:4173/mcp` in your MCP client. Clients with OAuth can discover the server and open the consent screen in the browser. The flow uses PKCE S256 and single-use codes. OAuth tokens last one hour; when one expires, the client must authorize again. The server issues no refresh tokens.

Clients that support headers can also use a delegated token.

```http
Authorization: Bearer YOUR_TOKEN
```

Call `platform_describe` and `projects_list` after connecting. `platform_describe` states the yardstick, a short first run and the workflow. `platform_guide` takes `rules`, `pieces`, `credentials`, `development`, `actions`, `sdk`, `publishing`, `users`, `portability` and `refusals`. `refusals` lists every code a refused Capsule answers, with who fixes it and a one-line remedy. Start with `rules` (same checks in the Gate and the Kernel, first run, read-back after every write, named preview-vs-publish gaps) and `pieces` (which Capsule declaration to use). The other topics are short forms of sections on this page. `session_inspect` returns `can` for the scopes this credential holds. After a write, call the read-back named in `rules` before retrying or telling the owner it worked. [The walkthrough](mcp-walkthrough.md) runs every step from a credential to a migration against a Kernel, with the exact answers. An agent needs filesystem and local execution tools to create files and build an app. The MCP server does not read paths on the user's machine.

If MCP is published on an origin other than the hosting origin, set `LITTLEBOX_MCP_ORIGIN` to its public origin, without a path. The proxy must preserve `Host`. Owner signup answers on that host too, so the Kernel refuses to start with the default setup key when this host is not local. Browser calls to the editor and to the runtime use their own origin; there is no open CORS.

## Tools

| Group | Tools |
| --- | --- |
| Discovery | `platform_describe`, `platform_guide`, `session_inspect`, `mail_status`, `projects_list`, `project_inspect` |
| Projects | `project_create`, `project_sources_read`, `project_sources_save`, `project_delete` |
| Eject, with `manage_lifecycle` | `eject_plan`, `eject_apply` |
| Migration, with `manage_lifecycle` | `migration_begin`, `migration_confirm`, `migration_cancel`, `migration_activate`; `migration_status` with `read` |
| Owner schema and data | `schema_plan`, `database_execute`, `storage_test` |
| App export, with `export_data` | `package_export`, `project_export`, `database_export` |
| Publishing | `release_plan`, `release_apply` |
| Signup policy | `signup_policy_read`, `signup_policy_set` |
| Sign-in methods | `sign_in_methods_read`; `sign_in_methods_set` with `manage_users` |
| Signing secrets | `secrets_list`, `secret_set`, `secret_delete` |
| API keys | `api_keys_list`, `api_keys_create`, `api_keys_revoke` |
| Custom domain | `domain_read`; `domain_set`, `domain_clear` with `publish` |
| Usage and Signals | `usage_read`, `signals_read`, `usage_summary`, `usage_overview` |
| Calls between apps | `calls_read` |
| Email notices | `notices_read` |
| App users and invitations | `app_users_list`, `app_user_suspend`, `app_user_resume`, `app_invitations_create`, `app_invitations_list`, `app_invitation_resend`, `app_invitation_revoke` |
| Transfers | `transfer_begin`, `transfer_status`, `transfer_write`, `transfer_complete`, `transfer_read`, `transfer_cancel` |
| Credentials from the owner session | `tokens_list`, `token_create`, `token_revoke` |
| App runtime | `auth_session`, `auth_logout`, `actions_invoke`, `files_list`, `files_upload`, `files_read`, `files_update`, `files_delete` and transfers |
| App accounts, also before sign-in | `auth_signup`, `auth_verify`, `auth_recover`, `auth_reset` |

The catalog depends on the credentials. The Kernel checks the permission again on execution. A call to a tool the credential does not list answers `Unknown tool or insufficient permission.` and, for a tool another credential would list, the permission it needs and how to get it, such as `app_invitations_create needs a credential with manage_users. Run littlebox login with --manage-users, or use a token from token_create that includes it.` A tool of the runtime endpoint called here says so. A token limited to projects cannot create other projects. For `schema_plan`, that token must pass a `projectId` it has access to. Reading the signup policy needs the `read` scope. Changing it, and every `app_users_*` and `app_invitation*` tool, needs `manage_users`. The owner's browser session holds it. A delegated token holds it only when it was created with it, so tokens issued before this permission existed do not gain it. `read` alone never lists accounts or invitations.

This changes the contract. `signup_policy_set` needed `publish` before `manage_users` existed, so a client that changed the policy with a `read`, `write` and `publish` token now gets "Unknown tool" and needs a new token that includes `manage_users`. `bun run littlebox -- login --manage-users` asks for one, and `bun run littlebox -- dev` asks for one on its Development Kernel.

Listing signing secrets and API keys needs the `read` scope; setting, creating and revoking them needs `publish`. `domain_read` needs `read`; `domain_set` and `domain_clear` need `publish`. `usage_read`, `signals_read`, `usage_summary`, `usage_overview`, `calls_read` and `notices_read` need `read`.

`project_create` returns a template and an External identity by default. Later sources stay in the user's folder; `project_inspect` returns metadata and the Deployment, without claiming to synchronize that code with the editor.

The editor uploads a new app's JSON without a `projectId`, then calls `project_create` with `sourceAuthority: "managed"` and `sourcesTransferId`. The Kernel validates the files and saves the project registry entry and files in one transaction, returning `sourcesRevision: 1`. A failed upload or validation leaves no project entry. Repeating the same initial files returns the saved revision; different files cannot overwrite an existing save through `project_create`. Subsequent edits use `project_sources_save` with that revision.

`project_delete { projectId, name }` needs `write` and deletes a project for good. `name` must equal the project's name; any other value is refused with `The name does not match this project. Type the project's name exactly to delete it.` The Kernel removes the project's Deployments from routing first, so their addresses stop answering, then deletes each Deployment folder (database, app accounts, uploaded files, backups, custom domain) and its stored artifacts, then the registry entry, saved editor files, plans and signing secrets. The answer lists the deleted Deployment IDs. Read back with `projects_list`. A paused app refuses it like any other change. Export a package first to keep the data. The build agent does not get this tool.

## Signup policy

Each Deployment has a signup policy that decides who may create an app account. `signup_policy_set` takes its `mode`:

- `open`: anyone can create an account.
- `invitation`: nobody can sign up; an invited person creates the account from the invitation link.
- `closed`: nobody can sign up; a person the owner invites can still accept the invitation. Pending invitations stay acceptable when the owner closes signup, so revoke the ones that should end.
- `allowlist`: only exact emails or `@domain` entries can create an account, up to 100 entries. A domain entry matches that exact domain, not its subdomains. Entering an address sends no invitation.

Public Deployments start Open. Private Deployments start Closed, so the owner decides who may join before anyone can sign in. A Deployment record written before signup policies existed gets the same default from its visibility, and one that names no visibility is private and starts Closed. A first release may set any mode in `initialSignupPolicy`, with the `publish` permission, and the Kernel installs it before serving the first request; an agent publishing an app whose access the request leaves open proposes private and `invitation`. Republishing keeps the current policy, except that republishing a public app as private turns Open into Closed; `initialSignupPolicy` on a republish answers "Portable state can only be imported on the first deployment." Changing an existing app's policy needs `manage_users`, through `signup_policy_set`, so no publish credential changes who may join. The owner changes it in the editor's Auth > Users view or with the admin MCP tools above. The editor's publish dialog sends `initialSignupPolicy` on a first publish, proposing Invitation only for a private app, and on a republish shows the current policy read-only.

## Sign-in methods

Each Deployment lists how its app accounts sign in. `sign_in_methods_set { deploymentId, methods }` takes `["password"]` or `["password", "google"]` and needs `manage_users`; `sign_in_methods_read` reads it back with `read`. Password cannot be turned off, so every account keeps a way back in.

Each app has its own Google OAuth client, which the owner makes in their own Google Cloud console as a Web application client. The owner sets it as two project secrets with `secret_set { projectId, name, value, use: "google" }`: `google_client_id` and `google_client_secret`. Those two names take only `use: "google"`, the values are never returned, and no route signature or call reads them. `sign_in_methods_read` is the read-back: `googleClient { clientIdSet, clientSecretSet }` says whether each half is set, and `redirectUris` lists the callbacks to register in that client, `<app address>/_littlebox/auth/google/callback` for the generated host and one more once a custom domain is verified. `sign_in_methods_set` refuses `google` while either half is unset, with "Set this app's Google client ID and client secret before turning Google on.", and `secret_delete` refuses either half while Google is on, so turning Google off stays the owner's own step. The editor's Auth > Users view shows the same redirect URIs with a copy button, the two fields and the switch.

The whole flow stays on the app's host. `/_littlebox/auth/google/start` records the flow (state, nonce, PKCE verifier, Deployment, origin) behind a browser cookie and sends the browser to Google with the app's client ID and the callback of the host it arrived on, which must be one of `redirectUris`. Google returns to `/_littlebox/auth/google/callback`, which takes the flow once on the same Deployment, origin and browser, exchanges the code with the app's client and checks the ID token: Google as issuer, the app's client as audience, expiry, the flow's nonce, a key from Google's fixed JWKS URL and a verified email. It then opens the app's own session.

A Google sign-in reaches the app account that holds the same email, compared without case, whether that account was made with a password or with Google. Google proves only an address it manages: a `gmail.com` address, or an address whose domain the token names in its `hd` claim as a Google Workspace domain. Any other address is refused with "This Google account uses an email address Google does not manage, so it cannot sign in here. Sign in with email and password." A suspended account is refused and a pending one keeps waiting for its email link. For an email without an account, the Signup Policy decides as it does for a password, in the same transaction and with the same words: Open creates the account, Allowlist checks the Google email, Invitation only and Closed create nobody, and an invitation accepted with Google needs the invited email. An account created through Google has no password a person chose; recovery by email sets one.

A package carries the methods in its manifest as `signInMethods` and the accounts in its identity database, never the client. On another installation the owner sets the two secrets again and registers that host's redirect URI in the Google client; the same person then reaches the same account, because accounts are matched by email and not by anything a client issues. Until then the Deployment keeps the setting, its sign-in page offers only email and password, and `sign_in_methods_read` returns a `notice` that says so.

## Users and invitations

All of these tools take a `deploymentId`, check `manage_users` and the token's project grants, and write an audit row naming the Deployment. The editor's Auth > Users view calls the same tools with the owner's session, so a person who does not write code invites, resends, revokes, suspends and resumes through the same checks an agent meets. It generates one idempotency key per batch of addresses and reuses it when the same batch is retried, and it disables invite and resend, with the reason, while `mail_status` reports `sending: false`.

| Tool | Arguments | Result |
| --- | --- | --- |
| `app_users_list` | `limit` (1 to 100, default 50), `cursor`, `state` | `{ users: [{ userId, email, state, verified, verifiedAt, createdAt, role }], nextCursor }`, ordered by email. `role` is `owner` for the Project Owner's own account and `member` otherwise. |
| `app_user_suspend`, `app_user_resume` | `userId` | `{ userId, email, state }`. Suspension ends sessions and open links and keeps ID, password and rows. Resume returns `active` if the account ever verified, otherwise `pending`. |
| `app_owner_enter` | none besides `deploymentId` | Listed and accepted only for the owner's browser session; a delegated token, even with `manage_users`, does not see it and gets `Unknown tool or insufficient permission.` Creates or links the app account for the owner's creator email with role `owner` and answers `{ userId, email, role: "owner", account: "created" or "existing", passwordCleared, url, expiresAt }`. `url` is `<app>/_littlebox/owner-entry#entry=<token>`, works once for two minutes and signs that account in. See [the owner's account in an app](#the-owners-account-in-an-app). |
| `app_invitations_create` | `emails` (1 to 50), `idempotencyKey` (8 to 128 characters) | `{ policy, results: [...] }` with one result per distinct address: `invited` with `invitationId`, `expiresAt`, `acceptable` and `delivery`; `already-invited` with the pending `invitationId`; `already-member`; `suspended`; or `invalid-email`. |
| `app_invitations_list` | `limit`, `cursor` | `{ invitations: [{ invitationId, email, status, invitedBy, createdAt, expiresAt, acceptedAt, revokedAt, delivery, sends }], nextCursor }`, newest first. |
| `app_invitation_resend` | `invitationId`, `idempotencyKey` | A new seven-day link for a pending invitation; the earlier link stops working. Refused for an accepted or revoked invitation, an active account or a suspended one. |
| `app_invitation_revoke` | `invitationId` | Ends a pending invitation. Revoking twice answers the same; an accepted invitation is refused. |

Inviting sends email, so an agent calls `app_invitations_create` only when the owner asked for those invitations. The email names the app and the owner who invited, by the email of the owner's account: the subject reads `owner@example.com invited you to Tasks`, and a resend names the owner who resends. An owner who signs in through Clerk has no email stored in the Kernel, so that email names only the app, `You are invited to Tasks`. Both values have line breaks removed and are escaped in the HTML part; no text from the agent's call reaches the message. Without a mail transport the call refuses before creating anything. A retry with the same `idempotencyKey` from the same owner returns the first answer and creates and sends nothing; the same key with other arguments is refused. Answers are kept seven days. `acceptable` is false when the current policy would refuse the acceptance, which only an Allowlist without that address does. Closed accepts an invitation, because the owner chose the person.

`status` is `pending`, `expired`, `needs-resend`, `already-member`, `accepted` or `revoked`. `needs-resend` means the link is gone because the invitation came with a copy or its account was suspended. `already-member` means the address became an active account another way, such as signing up while the app was Open. `delivery` is `queued` when the mail transport took the message, `failed` when it refused it, and `not-sent` when no message left this installation. `queued` says nothing about the recipient's inbox; delivery states come with the mail queue. A tighter policy blocks new accounts only; existing accounts can still sign in. A refused signup gets the same answer whether signup is closed or the address is not listed.

In local mode Littlebox does not verify email addresses, so whoever signs up first with a listed address or an address in a listed domain gets that account. Use Closed for apps that hold private data, or require verification.

## Verified accounts

`LITTLEBOX_ACCOUNT_VERIFICATION=required` makes every new account, creator or app user, prove control of its email before it is active. The default `local` keeps unverified signup for development; the README explains both modes, the switch for existing accounts and the operator's suspension command.

An email address is a plain `local@domain`: one `@`, a dotted domain, no spaces or control characters, and none of `<>()[],;:\"`, which mail libraries read as a display name, a comment or a list, so `x<a@b.example>` cannot become a second account for `a@b.example`. Such an address answers `Enter a valid email address.` One account holds at most 20 open sessions; a sign-in past that ends its oldest session.

An account is `pending`, `active` or `suspended`. Only `active` accounts get a session, and every runtime and administrative check reads the state: the app session, the owner session, a delegated MCP credential and the OAuth code exchange. Suspending an account deletes its sessions and open links and keeps its ID, password and rows.

A challenge has a purpose, `verification`, `recovery` or `invitation`, and a scope, `creator` or one Deployment. The Kernel stores only its SHA-256 hash. A token works once, expires (verification after 24 hours, invitation after seven days, recovery after 30 minutes), and does nothing for another purpose, on another Deployment or on the creator scope. A new link for the same purpose replaces the earlier one.

In required mode the signup request carries only an email. The password is chosen on the page the link opens, so a password typed by whoever registered the address first never survives verification. Signing up again with a pending email keeps the account ID and sends a fresh link. Links put the token in the URL fragment, `#verification=` or `#recovery=`, so it reaches neither server logs nor a Referer. Opening a link with GET returns the form and changes nothing; the form posts the token with the new password.

| Route | Host | Body | Result |
| --- | --- | --- | --- |
| `POST /_littlebox/auth/signup` | app | `{ "email" }` | 202 `{ "result": { "status": "link-sent" } }`, the same for new, pending, active and suspended emails, with the signup hint cookie described below. A signup the policy refuses gets 403 as before. |
| `POST /_littlebox/auth/verify` | app | `{ "token", "password" }` | 200 with the app session cookie. Rechecks the Deployment's current signup policy: under Closed, or for an email no longer listed, it answers 403 and keeps the link usable. |
| `POST /_littlebox/auth/recover` | app | `{ "email" }` | 202, the same answer whether or not the account exists. Active accounts get a recovery link, pending ones a new verification link, suspended and unknown ones nothing. |
| `POST /_littlebox/auth/reset` | app | `{ "token", "password" }` | 200 with a new session; every earlier session of the account ends. |
| `GET /_littlebox/account` | app | | The page links open. Without a token it offers recovery. |
| `GET /_littlebox/invitation` | app | | The page an invitation link opens, `#invitation=<token>`. It changes nothing. |
| `POST /_littlebox/auth/invitation` | app | `{ "token" }` | 200 `{ "result": { "email", "app", "status" } }` with `status` `new-account`, `existing-account` or `signup-closed`. Reads only; the page shows the email read-only from it. |
| `POST /_littlebox/auth/accept` | app | `{ "token", "password" }` | 200 with the app session cookie. One transaction consumes the token, rechecks the current policy and the account's state, sets the password, activates the account and marks the invitation accepted. 403 under an Allowlist without the address, and for a suspended account; 409 for an active account, which must sign in. Every refusal keeps the token usable. |
| `POST /_littlebox/owner/signup` | editor | `{ "email", "code", "returnTo"? }`, or `"setupKey"` for the first owner | 202 in required mode, after the code or key is consumed. See [creator admission](#creator-admission). |
| `POST /_littlebox/owner/verify`, `/_littlebox/owner/reset`, `/_littlebox/owner/recover` | editor | as above | The creator scope; links open `/_littlebox/owner/login`. |

Every 202 from an app signup, over HTTP or the runtime tool `auth_signup`, sets an `HttpOnly`, `SameSite=Lax` cookie, `littlebox_signup` (`__Host-littlebox_signup` on HTTPS), that lives as long as a verification link and holds the SHA-256 of the Deployment ID and the email typed, never the email. A sign-in to `POST /_littlebox/auth/login` that fails with a wrong password, or with no password because the account is pending, still answers 401, and when the request carries that cookie for the email typed the message is `Email or password is incorrect. If you signed up with this email in this browser, open the link we emailed to choose your password first.` Without the cookie it stays `Email or password is incorrect.` The hint depends on the cookie alone and never on the account's state, so a person who missed the email learns what to do, and someone testing addresses learns nothing: their own signup request sets the same cookie for an address with an active account as for a new one, and the hint then follows every wrong password for that address.

A used, expired, replaced, revoked, foreign or wrong-purpose token answers 410. A pending account that signs in with a correct legacy password, or a suspended one, answers 403. A weak password answers 400 before the token is read. Without a mail transport, signup and recovery answer 503. The creator routes are unavailable when the optional hosted identity adapter is configured.

The runtime MCP tools `auth_signup`, `auth_verify`, `auth_recover` and `auth_reset` run the same flows for the app on the request's host. A signed-out visitor may call them on public and private apps; on a private app they are the only tools a signed-out caller reaches, and `tools/list` and every other call still answer 401. In local mode `auth_signup` also takes `password` and signs in at once. A tool that signs someone in sets the session cookie on the response and never returns a token. A full copy drops sessions and challenges and clears the invitations' token hashes; an import does the same to what it finds. Invitation history and pending recipients travel, and a pending invitation needs a resend at the destination.

## Creator admission

A creator account needs an invitation code, except the installation's first owner, which the setup key creates. The operator issues codes on the server with `littlebox-kernel creator-code-create [--expires-in 14d] [--note TEXT]`, lists them with `creator-code-list` and stops an open one with `creator-code-revoke <id>`; the README's operator section has the details. A code is single use, expires after 14 days by default, and is stored as a SHA-256 hash in `owners.sqlite`.

`POST /_littlebox/owner/signup` takes `{ "email", "code" }`, plus `"password"` in local mode. The Kernel consumes the code in the transaction that creates the account: a pending account with a verification link in required mode, an active account with a session in local mode. A code for an email whose account is pending is consumed and sends a fresh link to that account. A code for an active or suspended email is consumed and creates nothing, and the answer is the same 202 a new email gets. In local mode a taken email answers 409 `An account with that email already exists.` only to a valid code, which stays unused; a made-up code gets the same 403 a new email gets, so it says nothing about who has an account. When both `code` and `setupKey` are sent, the code decides.

The setup key creates an account only while `owners.sqlite` holds no account, or only the first owner's pending account asking for a new link, and no code has ever been issued. The default `local-development-key` keeps creating owners until the first code is issued, so local development and the browser tests still work; the Kernel accepts that key only on loopback unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`.

`GET /_littlebox/owner/config` returns `"admission": "setup-key"` while the key can still create an account and `"code"` otherwise. The owner page and **Platform access** ask for the field it names and never mention the key in `code` mode.

| Request | Status | Message |
| --- | --- | --- |
| Used, expired, revoked, unknown or mistyped code | 403 | `This invitation code is not valid. It may be mistyped, expired, revoked or already used.` |
| Installation key once any code exists or any other account exists, or no admission at all | 403 | `An invitation code is required to create a creator account. Ask the operator of this installation for one.` |
| Wrong setup key while the key can still create the first owner | 403 | `A valid installation setup key is required to create the first owner.` |

A refused signup creates no account and sends no mail. Codes count in the same per-address and per-email sign-in limits as the key did.

An MCP client that starts OAuth while the creator is signed out sends them to `/_littlebox/owner/login?returnTo=/oauth/authorize?...`. The page passes `returnTo` with the signup. The Kernel keeps it only when it starts with `/oauth/authorize?`, has no fragment or control character and fits in 4096 bytes, and puts it in the verification link's query: `/_littlebox/owner/login?returnTo=...#verification=TOKEN`. After the password is set the page opens the consent screen, and approving returns the code to the client's registered redirect. In local mode the signup answers with a session and the page returns to the consent screen at once.

Suspending a creator with `creator-account-suspend` deletes their sessions, open verification and recovery links, delegated MCP credentials and OAuth codes not yet exchanged, and every later use of an OAuth token or credential reads the account state. A creator admitted with a code but suspended before verifying cannot use the link. The code stays used.

## Account email

Every account link, and every notice a mutation declares with [`notify`](#notices), leaves through the queue in `mail.sqlite` (`kernel-rs/src/mail.rs`). The README's [Account email](../README.md#account-email) section lists the transports and their variables. What a client sees:

- Signup and recovery store the challenge and its email job in one transaction and answer at once. The worker sends the job on its own task.
- A job is `pending`, `accepted` (the provider took it), `delivered` (the provider reported delivery to the recipient's server) or `failed`. No job state activates an account; only the person answering the link does.
- A refusal stores nothing. 503 means no transport, or a sender the preflight refused. 429 means a limit: per address 3 a minute across the installation and 10 a day in each scope, so requests on one app cannot use up an address's allowance on another app or in the editor; per client address 20 an hour, where one IPv6 /64 is one client; per creator 100 a day, per app 100 a day, per installation 1,000 a day. The address and client limits count requests, so an address with no account reaches them after the same number of requests as one with an account. The creator and app limits count account email only; notices have their own, and the installation's 1,000 counts both.

`mail_status` is an admin tool for any owner credential. It returns the transport, the sender, `sending`, the preflight with its state (`checking`, `ready` or `refused`), reason and time, the queue depth and the count per state, the same counts for notices alone under `queue.notices`, the last failure as `{ kind, at, purpose }` where `purpose` is `verification`, `recovery`, `invitation` or `notify`, the limits, and whether provider events are configured. `refresh: true` runs the preflight again. It names no recipient. Without a transport it returns `sending: false` and how to configure one.

`/_littlebox/health` carries a `mail` field when a transport is set: `transport`, `sending`, `preflight`, `depth` and `lastFailure` with its kind and time only.

A failure kind is one of `provider-unavailable` (retrying, or out of retries), `rejected` (the provider refused that message), `sender-refused` (the provider refused the sender; sending stops until a preflight passes), `superseded` (a newer link replaced it, or its challenge ended), `link-expired`, `secret-unavailable` (the sealing secret changed), `bounced`, `provider-failed`, `suppressed`, `restored` (a restore ended it), and for a notice `sender-not-ready` (the preflight was not ready when the mutation ran, so the notice was recorded and never sent) and `notice-expired` (not sent within a day).

### Resend

The Resend transport calls `POST /emails` with the job ID in the `Idempotency-Key` header. Resend keeps a key for 24 hours and accepts up to 256 characters ([send email](https://resend.com/docs/api-reference/emails/send-email)); the worker's last retry runs about two hours after the first attempt. The worker treats 429, 5xx and 409 `concurrent_idempotent_requests` as retryable, 401 and 403 as a refused sender (403 `validation_error` is Resend's answer for an unverified domain), and any other 4xx as a rejected message ([errors](https://resend.com/docs/api-reference/errors)).

The preflight calls `GET /domains` and requires the sender's domain with status `verified` ([list domains](https://resend.com/docs/api-reference/domains/list-domains), [domain statuses](https://resend.com/docs/dashboard/domains/manage-domains)). A key created with `sending_access` "can only send emails" ([API keys](https://resend.com/docs/api-reference/api-keys/create-api-key)), so it cannot list domains; `LITTLEBOX_RESEND_DOMAINS_KEY` supplies one that can.

`POST /_littlebox/mail/resend-events` takes Resend's webhooks. Resend signs them with Svix: the `svix-id`, `svix-timestamp` and `svix-signature` headers, an HMAC-SHA256 over `{svix-id}.{svix-timestamp}.{raw body}` keyed with the base64 part of the `whsec_` secret, and a space-separated list of `v1,<signature>` entries ([verify webhooks](https://resend.com/docs/dashboard/webhooks/verify-webhooks-requests), [Svix manual verification](https://docs.svix.com/receiving/verifying-payloads/how-manual)). The Kernel compares in constant time, accepts a timestamp within five minutes of its clock, the window Svix's libraries use, and answers 401 otherwise. Without `LITTLEBOX_RESEND_WEBHOOK_SECRET` the route answers 404.

Each `svix-id` is recorded once, so a replay answers 200 with `duplicate: true` and changes nothing. `email.sent` moves a job to accepted, `email.delivered` to delivered, and `email.bounced`, `email.failed` and `email.suppressed` to failed ([event types](https://resend.com/docs/dashboard/webhooks/event-types)); the job is found by `data.email_id`. A state only moves forward and the first final state wins, so events in any order end in the same state. An event for an ID the worker has not recorded yet is kept and applied when it does. Other event types are recorded and change nothing.

## The owner's account in an app

An app decides what its owner may do with the same rules it applies to everyone, so it needs to recognise the owner's account. The Project Owner's app account on a Deployment has the role `owner` in that Deployment's `identity.sqlite`; every other account has `member`. `auth_session` returns it, `app_users_list` lists it, and an action reads it with `{ "source": "actor", "field": "role" }`, which binds `owner` or `member` for a person and null for a visitor, an API key or a signed request. Unlike the ID binding it never refuses on its own; write the check into the statement and declare a guard:

```json
{
  "kind": "mutation",
  "access": "authenticated",
  "statement": "UPDATE profiles SET role = 'admin' WHERE user_id = ?1 AND role = 'member' AND ?2 = 'owner'",
  "bindings": [{ "source": "input", "key": "userId", "type": "string", "maxLength": 64 }, { "source": "actor", "field": "role" }],
  "guard": "Only the app's owner can make a member a team admin."
}
```

The role comes from the owner's own credential, never from signup or an input:

- `app_owner_enter`, which the editor's Users view calls from Open as owner, takes the owner's browser session. It finds the account whose email is the owner's creator email, or creates it active with no password, sets its role to `owner`, and returns a link that works once for two minutes. Opening the link shows a page that posts the token from the fragment to `POST /_littlebox/owner-entry`; GET changes nothing. The POST answers 200 with the app session cookie, and 410 `This link is invalid, expired or already used.` for a used, expired or foreign token. Usage counts the entry as a login.
- Under required verification the creator scope already proved the address, so the account counts as verified. A pending account under that email, which may be a stranger's unanswered signup, is taken over: its links and sessions end. An active account that never verified, possible in local mode where anyone may sign up with any address, loses its password and sessions, and the answer says `passwordCleared: true`; the owner sets a password with recovery when they want one. A suspended account is refused with `Your app account on this app is suspended. Resume it in the Users view first.`
- An owner signed in through the optional hosted identity adapter has no email in the Kernel and gets `Your owner account signs in through the hosted identity provider, so this installation has no email to give your app account. Sign up in the app instead.`
- `app_invitations_create` for the owner's own creator email marks the pending account `owner`, since only the owner's mailbox can answer the link.

The role travels with the identity file in a package, a portable export and a migration, so after an import to another owner's installation the previous owner's account is still `owner` and the new owner's Open as owner adds a second; no tool removes the role, and `app_user_suspend` is the lever until one does. The editor's preview has no owner account: Alice and Bob are members.

## Local development cycle

From this repository, get a credential that can create and publish projects with `login` (described at the end of [Build and publish](#build-and-publish)), or set `LITTLEBOX_MCP_TOKEN` to one.

```sh
LITTLEBOX_EMAIL=you@example.com LITTLEBOX_PASSWORD=... bun run littlebox -- login --dir /tmp/my-app --url http://localhost:4173
bun run littlebox -- init --dir /tmp/my-app --name "My app" --url http://localhost:4173
bun run littlebox -- dev --dir /tmp/my-app --port 8796
```

`dev` compiles and starts the Rust Kernel with data under `/tmp/my-app/.littlebox/dev-data`. It generates a separate development identity, removes the remote configuration from its environment and publishes the build to that local Kernel. That Deployment is private, so `dev` then sets its signup policy to Open and you can create test accounts. It prints the URL and rebuilds when the sources change. Reload the app to see the new build. When the command exits, the Kernel stops and the data stays in that folder.

The Runtime Actor creates its account inside the local app. That account is neither the owner's nor a production account. The SDK injected into the build offers the usual calls.

```js
const user = await miniapp.auth.session();
const items = await miniapp.actions.call("items.list");
const files = await miniapp.files.list();
```

[The miniapp SDK](sdk.md) lists every call with what it returns, the exact messages it refuses with and the limits that apply. That page, the `sdk` topic of `platform_guide` and the SDK section of the build agent's prompt are rendered from one contract, `src/sdk/contract.ts`, by `bun run sdk:render`. A test fails when `src/mcp/runtime.js`, the preview client or a rendered file disagrees with it.

Without a session, `miniapp.auth.session()` returns an anonymous state so the app can show a sign-in button. On a public app, a signed-out visitor may call actions declared with `"access": "anonymous"`, and may list and read files uploaded with visibility `public`. Uploads, private files, and actions declared `authenticated` require sign-in. On a private app, visitors must sign in before the app loads, and nothing runs without a session.

Signed-out visitors may call `actions_invoke` 30 times per 10 seconds per client address and Deployment, and each signed-in app user 120 times per 10 seconds. Further calls return a tool error with the text `rate-limited` and a `Retry-After` header. The Kernel uses the socket peer address and reads `X-Forwarded-For` only from peers listed in `LITTLEBOX_TRUSTED_PROXIES`. Such a proxy must append the address it received the request from, never forward the client's header unchanged. Owner calls are audited. For app users the Kernel records only a signed-in `files_upload` that stores a new file and a `files_delete` that removes one. Failed uploads and deletes, receipt replays, visibility changes, action calls, reads and session checks write no audit row. The log keeps the newest 10,000 rows of each owner and the newest 10,000 rows of each Deployment's app users together, and drops rows older than 30 days, so an app user who uploads and deletes in a loop replaces only their Deployment's rows and never an owner's.

A file is private to the app user who uploaded it unless that upload set `visibility` to `public`. Only that user can list or read a private file. On a public app, a signed-out visitor can list and read public files. On a private app, any signed-in user of that app can read public files, and private files stay with the uploader. Files stored before this rule have no owner and stay unreadable to app users. A portable export still includes every file for the owner. The URLs `miniapp.files.list()` and `miniapp.files.upload()` return are Blob URLs created after an MCP download, and the page's `connect-src 'self'` keeps `fetch` from reading them again. `miniapp.files.url(fileId)` returns the file's path, `GET /_littlebox/files/<id>`, described under [HTTP routes](#http-routes), which applies the same rules and which an `img`, a link or a `fetch` on the app's own origin may read.

The uploader, and nobody else, changes a file afterwards. `files_update` with `{ fileId, visibility }` sets `private` or `public` and returns the file without a `url`; the ID and its path stay the same, so a published draft is the same file and counts once against the quota. `files_delete` with `{ fileId }` removes the file's entry and bytes and returns `{ id, deleted: true }`; its size stops counting against the Deployment's file quota, and from then on `files_read`, `files_list` and the file path answer as for a file that never existed. Both need a session. A user who may not read the file gets `File not found.`, the same as for a missing one, and a user who reads it but did not upload it gets `Only the person who uploaded this file may change or delete it.` A file stored before files had owners has no uploader, so the app cannot change or delete it. A row that still names a deleted file ID keeps it; the app removes that row with its own action. Neither tool counts in Usage. Each visitor's downloads use a transfer bound to that client address, so anonymous callers do not share one transfer bucket.

Input bindings in `actions.json` require a value by default. Add `"nullable": true` to a string, number or boolean input binding to bind an omitted value or explicit `null` as SQL `NULL`. Supplied values still follow the declared type and length limit, and empty strings remain invalid. A search action can use the same parameter for its optional filter.

```json
{
  "kind": "query",
  "access": "authenticated",
  "statement": "SELECT id, title FROM notes WHERE user_id = ?1 AND (?2 IS NULL OR title LIKE '%' || ?2 || '%') LIMIT 100",
  "bindings": [
    { "source": "actor", "field": "id" },
    { "source": "input", "key": "q", "type": "string", "maxLength": 100, "nullable": true }
  ]
}
```

Calling this action with `{}` returns the actor's rows up to the limit; `{ "q": "trip" }` filters them. Mutation actions return `{ "changes", "lastInsertId" }`. They cannot use SQL `RETURNING`; publication and the Validation Gate reject it. Use a separate query to read the changed rows.

### Statement rules

An action runs one SQL statement. A query starts with `SELECT` and a mutation with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`, after any leading comment. The Validation Gate and the Kernel refuse a statement that breaks a rule with the same message, from one case file, `kernel-rs/src/sql-rule-cases.json`, that both test suites run. `kernel-rs/src/sql_rules.rs` and `src/kernel/sql-rules.ts` hold the checks.

| Statement | Message |
| --- | --- |
| A query that opens with `WITH` | `Action <name> is a query, so it must start with SELECT. Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).` |
| A mutation that opens with `WITH` | `Action <name> is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE. Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.` |
| A `CREATE`, `ALTER` or `DROP`, such as a trigger or a view | `Action <name> is a query, so it must start with SELECT. Tables and indexes belong in schema.sql, and Littlebox runs no triggers or views.`, or the same rule for a mutation |
| Any other first word | `Action <name> is a query, so it must start with SELECT.` or `Action <name> is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE.` |
| Anything after the first `;` but comments and whitespace, where a character at or above U+0080, such as a no-break space, counts as text as SQLite reads it | `Action <name> must contain one statement.` |
| `RETURNING` on a mutation | `Action <name> cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.` |
| A query that reads rows with no top-level `LIMIT`; `:limit`, `@limit` and `$limit` are named parameters, not the clause | `Action <name> reads rows and has no LIMIT. Add a LIMIT. ...`, with the reason the [query bound](./adr/0009-the-gate-and-the-kernel-share-one-query-bound.md) gives, such as `uses OVER, which keeps one result row per stored row` |
| A trigger, a view or a row in `schema.sql` | `schema.sql may only create tables and indexes, but it contains CREATE TRIGGER.`, naming each refused statement as `CREATE VIEW`, `INSERT` and so on |

A `WITH` clause is allowed inside the statement: `INSERT INTO bookings (room_id, slot) WITH span(slot) AS (...) SELECT ...` and `SELECT ... FROM (WITH ... SELECT ...) LIMIT 50` both pass. A keyword or a `;` inside a string, a quoted name or a comment does not count.

### Action shape

Before it reads a statement, each door checks the action's name, access, kind, bindings and size, in that order, and the number of actions. Both check the actions in sorted name order, so with two bad actions both name the same one. `kernel-rs/src/action-cases.json` holds the cases both test suites run, and `kernel-rs/src/action_shape.rs` and `parseServerActionSource` in `src/kernel/actions.ts` hold the checks.

| Declaration | Message |
| --- | --- |
| More than 100 actions | `A project may declare at most 100 actions.` |
| A name that is not lowercase dotted words of letters and digits, each starting with a letter, or is longer than 100 characters | `Invalid server action name: <name>` |
| `"access": "granted"` | `Action <name> uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated".` |
| Any other access | `Action <name> has an invalid access. Use "anonymous" or "authenticated".` |
| A kind other than `query`, `mutation`, `signal` or `call` | `Action <name> has an invalid kind. Use "query", "mutation", "signal" or "call".` |
| `"nullable": true` on a binding that is not an input | `Action <name> has an invalid nullable input binding.` |
| A binding of another source, an input with no key or another type, an actor field other than `id` or `role`, a kernel field other than `now`, a literal that is not a string, a number or null | `Invalid binding <index> in <name>.` |
| A string input's `maxLength` outside 1 to 100,000 | `Invalid maxLength in <name>.` |
| A statement longer than 20,000 bytes, or more than 100 bindings | `Action <name> exceeds statement or binding limits.` |
| A top-level key the action's kind does not take, checked after every other check on the action | `Action <name> has a key <key> that a <kind> action does not take.` |

Each kind takes its own top-level keys, and no other:

| Kind | Keys |
| --- | --- |
| `query` | `kind`, `access`, `statement`, `bindings`, `http` |
| `mutation` | `kind`, `access`, `statement`, `bindings`, `http`, `signal`, `guard`, `notify`, `schedule` |
| `signal` | `kind`, `access`, `bindings`, `signal`, and `statement` only when it is empty |
| `call` | `kind`, `access`, `bindings`, `url`, `method`, `body`, `headers`, `auth`, `then`, and `statement` only when it is empty |

A misspelled key such as `"notfy"`, or `notify` on a call, is refused instead of passing and doing nothing. A key set to `null` counts as absent on both doors. A key that has its own refusal keeps it: a `guard` on a query gets the guard's sentence, not this one. With several keys out of place, both doors name the first in sorted order.

A field of the wrong JSON type, such as `"bindings": {}` or a missing `kind`, is refused by both, but the Kernel answers with its JSON reader's message, `Invalid actions.json: ...`, where the Gate names the action.

## HTTP routes

An action declared `anonymous` may answer one method and path on its Deployment's host. This is how a project receives a webhook or serves a JSON endpoint without a server of its own. `kernel-rs/src/http.rs` implements it; the [ADR-0011 updates of 2026-11-06](./adr/0011-one-verified-account-core-for-creators-and-app-users.md) record the decisions.

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

A query binds `GET` and a mutation binds `POST`. An optional `"callers"` list, described under [Who may call a route](#who-may-call-a-route), names the proofs the route takes. A path starts with `/` and has segments of letters, digits, `.`, `_`, `~` and `-`, up to 200 characters, with no `.` or `..` segment and no trailing slash. Paths under `/_littlebox`, `/oauth`, `/.well-known` and `/mcp` belong to the Kernel. Two actions may share a path only with different methods. A binding wins over an artifact file at the same path for its method; every other `GET` still serves the Build Artifact, and a `POST` to a path no action binds answers 405. A `HEAD` on a `GET` route answers 405 rather than running the query to drop its rows. The Kernel reads a Deployment's routes from `actions.json` once per release, so a request for a static file does not parse it. The Validation Gate and the Kernel refuse the same declarations with the same messages, from one case file, `kernel-rs/src/http-binding-cases.json`, that both test suites run. An `authenticated` action may not declare `http`.

The route answers to the callers it takes, described below, and on a private app an unsigned route takes an API key or a session, never a caller with neither. The action keeps its bindings, its query bound and the runtime endpoint's rate limits. A call that succeeds counts in [Usage](#usage-and-signals) under the route's method and path, and records the action's Signal if it declares one. A `GET` maps the query string onto the input bindings by their declared type, so `?min=10` binds the number 10; a `POST` reads a JSON object of at most 64 KiB. Inputs the action does not declare are refused, as with `miniapp.actions.call`, unless the route declares `fields`, described under [Webhooks from outside services](#webhooks-from-outside-services): then each input is read from its JSON path or its own key and the body's other keys are left out.

| Answer | Status | When |
| --- | --- | --- |
| `{ "result": [rows] }` or `{ "result": { "changes", "lastInsertId" } }` | 200 | The action ran. Every answer carries `Cache-Control: no-store`. A `GET` route that answers anyone also carries `Access-Control-Allow-Origin: *`, so a page on another origin may read it ([Calls between apps](#a-page-that-reads-another-app)) |
| `{ "error": "<binding message>" }` | 400 | An input is missing, unexpected or of the wrong type, the body is not a JSON object, or a query parameter repeats |
| `{ "error": "invalid-signature" }` | 401 | A route that needs the signature from this caller got none, a malformed one, one without its declared prefix, or one whose MAC does not match |
| `{ "error": "stale-timestamp" }` | 401 | The signature's `t` is more than 300 seconds from the Kernel's clock, or outside a `timestamped` signature's `window`. Checked before the MAC, so it says nothing about the secret |
| `{ "error": "authentication-required" }` | 401 | A route that takes an API key or a session, and no signature, got neither |
| `{ "error": "invalid-api-key" }` | 401 | The bearer token starts with `lbk_` and names no live key of this Deployment |
| `{ "error": "This route does not accept API keys." }` | 403 | The route's `callers` leave out `key` and the request carries a live key |
| `{ "error": "This API key may not call <action>." }` | 403 | The key does not name the action |
| `{ "error": "No route accepts POST on this path." }` | 405 | No action binds the path for `POST`. `Allow` is `GET` on a path a query binds, `GET, HEAD` elsewhere |
| `{ "error": "A route does not answer HEAD. Send GET." }` | 405 | A `HEAD` reaches a path a query binds. It is refused before the caller is resolved and before any SQL, with `Allow: GET`. `HEAD` on every other path serves the Build Artifact's headers as before |
| `{ "error": "replayed-signature" }` | 409 | The same signature arrived before, on this path, in the last 600 seconds, before or after a Kernel restart. A sender signs a retry again, with a new nonce. For a provider's scheme, the delivery was accepted before, as described under [Webhooks from outside services](#webhooks-from-outside-services) |
| `{ "error": "The request body exceeds 64 KiB." }` | 413 | |
| `{ "error": "rate-limited" }` | 429 | With `Retry-After`. 30 calls per 10 seconds per client address without a key or session, 120 per user or key, shared with `actions_invoke`. Also after 10 unknown or revoked `lbk_` keys from one client address to one Deployment in a minute: from then on any key from that address, a valid one included, is refused without a lookup until the bucket refills, on routes and on the file read path |

### Who may call a route

The Kernel resolves the caller before any SQL runs.

| Caller | `callers` entry | Proof | Actor ID for an `actor` binding |
| --- | --- | --- | --- |
| A service | `"signature"` | The signature the binding declares | None, so an action that binds the actor's ID refuses; the role binds null |
| A program the owner wrote | `"key"` | `Authorization: Bearer lbk_...`, an API key that names the action | The key's ID, `key_...`; the role binds null |
| A person | `"session"` | The app session cookie, sent from the app's own origin | The app user's ID; the role binds `owner` or `member` |

`"callers"` lists one or more of the three, each once. `"signature"` belongs in it exactly when the binding declares a signature. Without the list:

| Route | Callers |
| --- | --- |
| Signed | `["signature", "key"]` |
| Unsigned, on a private Deployment | `["key", "session"]` |
| Unsigned, on a public Deployment | anyone, a visitor with no proof included |

A key the route does not take is refused with 403. A session the route does not take counts for nothing, so the person is a visitor and needs the signature like any sender. The Kernel uses the cookie only when the request comes from the app's own origin. Every app host is same-site with the others, so `SameSite=Lax` alone would let one app's page call another app's route as its signed-in user; a request whose `Sec-Fetch-Site` is `same-site` or `cross-site`, or whose `Origin` is another host, runs as a visitor.

```json
"events.feed": {
  "kind": "query", "access": "anonymous",
  "http": { "method": "GET", "path": "/api/events", "callers": ["key"] },
  "statement": "SELECT id, title FROM events WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT 100",
  "bindings": [{ "source": "input", "key": "after", "type": "number", "nullable": true }]
}
```

An action whose route does not answer anyone answers only there. `actions_invoke`, and so `miniapp.actions.call`, refuses it with `Action <name> answers only on its HTTP route, <method> <path>.`: always for a signed route or one that declares `callers`, and on a private Deployment for an unsigned one. A webhook action is therefore not a second door for members or visitors, and the app's own page reads a private route with `fetch`, which sends the session cookie. The editor preview has no visibility and refuses the first two cases only.

The Validation Gate and the Kernel refuse the same `callers` declarations with the same messages, from `kernel-rs/src/http-binding-cases.json`:

| Declaration | Message |
| --- | --- |
| `callers` that is not a non-empty list of distinct `"signature"`, `"key"` and `"session"` | `Action <name> has an invalid callers list. List one or more of "signature", "key" and "session", each once.` |
| `"signature"` without a declared signature | `Action <name> lists "signature" among its callers but declares no signature.` |
| A signature that `callers` leaves out | `Action <name> declares a signature, so its callers must include "signature".` |

### Signatures

The owner sets the secret for the project before publishing. A release whose signature names an unset secret is refused when planned and again when applied, with `Action <name> signs with the secret <secret>, which is not set for this project. Set it with secret_set before publishing.` When several are unset, one refusal names every action and every secret: `Actions events.feed and events.receive sign with the secrets events_feed and events_webhook, which are not set for this project. Set each with secret_set before publishing.` The Validation Gate makes the same refusals before Deploy from `secrets_list`, which answers names and never values, and, once the app is published, the installation rules of [calls](#where-a-call-may-go) from `domain_read`. The cases are in `kernel-rs/src/release-cases.json`, which `cargo test` and `bun test` both run.

```text
secret_set   { "projectId", "name": "orders_webhook", "value": "<16 to 512 printable characters, no spaces>", "use"?: "route" | "call" }
secrets_list { "projectId" }                      names, uses, update times and svixKey, never values
secret_delete { "projectId", "name" }             refused while the published Deployment signs with it or a call uses it
```

A signature names no app or host, so a request signed for one project would be accepted by another that holds the same value. `secret_set` therefore refuses a value that another project of the same owner already holds as a route secret, with `Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.` A copy of an app for staging gets its own secret, and its sender signs with that one.

Secrets belong to the project and live in `mcp.sqlite`, next to the project registry, because a first publish must find them before its Deployment exists. They never enter `actions.json`, the Build Artifact, a listing, the audit log or a portable export. On another installation the owner runs `project_create`, then `secret_set`, then the release.

The sender puts this in the declared header:

```text
t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256, keyed with the secret, of:
<t>\n<nonce>\n<METHOD>\n<path>\n<canonical query string>\n<raw request body>>
```

The nonce is 1 to 64 letters, digits, `-` and `_`, fresh for each request, so two requests in one second differ and a retry is signed again rather than refused as a replay. The canonical query string is the request's query pairs sorted by key, then value, and written as `application/x-www-form-urlencoded`: spaces as `+`, and every byte but letters, digits, `*`, `-`, `.` and `_` as `%XX`. It is empty for a request with no query. The method and the path are the route's, so a signature for one route or one query does not fit another.

The Kernel refuses a `t` more than 300 seconds before or after its clock with `stale-timestamp` before it looks at the MAC. It then recomputes the MAC over the exact bytes it received, compares it in constant time, and remembers each accepted signature for 600 seconds so a replay inside the window answers 409. `src/lib/route-signature.ts` signs the same way, and `kernel-rs/src/signature-cases.json` holds vectors both sides check. The memory is a `route_signatures` table in the Deployment's `identity.sqlite`, so a Kernel restart does not forget it, and an export carries it with the accounts, so the destination refuses a request captured on its way to the origin. `t` and the nonce are inside the MAC, so changing either invalidates the signature. Two `v1=` values may be sent while a secret rotates. A header with more than two 32-byte values answers `invalid-signature` before any MAC is computed, and a value that is not 32 bytes of hex is ignored, so a sender cannot make the Kernel compute one MAC per value. A shell sender with OpenSSL:

```sh
body='{"id":"A-1","total":12}'
t=$(date +%s)
n=$(openssl rand -hex 12)
sig=$(printf '%s\n%s\n%s\n%s\n%s\n%s' "$t" "$n" POST /hooks/orders "" "$body" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')
curl -X POST "https://orders-1a2b3c.example.net/hooks/orders" \
  -H 'Content-Type: application/json' -H "X-Signature: t=$t,n=$n,v1=$sig" --data-binary "$body"
```

A service with its own fixed signing scheme declares it instead, as described next. `littlebox-kernel restore` brings back the table as the copy held it, so a signature accepted after that copy is accepted once more if it is replayed while its `t` is still within 300 seconds of the Kernel's clock.

### Webhooks from outside services

A service the owner does not control, such as a code host, a shop or a payment provider, signs its webhook its own way and posts its own nested JSON. A route takes it as it arrives when its signature names the provider's `"scheme"` and its `"fields"` say where each input sits in the body. `kernel-rs/src/provider.rs` implements it and [ADR-0018](./adr/0018-a-route-verifies-a-providers-own-signature.md) records the decision.

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
| `littlebox`, the default | `header`, `secret` | The header [above](#signatures) | The MAC, for 600 seconds |
| `hmac-body` | `header`, `secret`, `algorithm` (`sha256` or `sha1`), `encoding` (`hex` or `base64`), optional `prefix` | In `header`, the prefix, then the HMAC of the raw body keyed with the secret. GitHub: `X-Hub-Signature-256`, `sha256`, `hex`, prefix `sha256=`. Shopify: `X-Shopify-Hmac-Sha256`, `sha256`, `base64` | The MAC, for 72 hours |
| `timestamped` | `header`, `secret`, `prefix` (the timestamp field, such as `t=`), `tag` (the MAC field, such as `v1=`), `window` (1 to 3600 seconds) | In `header`, comma-separated fields: the Unix time after `prefix` and the hex HMAC-SHA256 of `<t>.<body>` after `tag`. Fields with another name, such as Stripe's `v0=`, are ignored. Stripe: `Stripe-Signature`, `t=`, `v1=`, `300` | `t` outside the window answers `stale-timestamp`; inside it, the MAC is accepted once |
| `svix` | `secret` | `svix-id`, `svix-timestamp` and `svix-signature`, the base64 HMAC-SHA256 of `<id>.<t>.<body>` keyed with the base64 after `whsec_`, after `v1,`. Resend and Svix send it | A timestamp more than 300 seconds off answers `stale-timestamp`; the id is accepted once in 72 hours |

The Kernel decodes the MAC from hex, in either case, or base64, recomputes it over the exact bytes it received and compares it in constant time. A `timestamped` or `svix` header may carry up to four MAC values while the provider rotates a secret; one with more answers `invalid-signature` before any MAC is computed, and a value that does not decode to 32 bytes is ignored. The secret is set with `secret_set` as for Littlebox's scheme. A svix secret is the `whsec_` value the provider shows, and a release whose svix route names a secret that is not `whsec_` and base64 is refused with `Action <name> verifies svix signatures with the secret <secret>, which is not a whsec_ signing secret. Set it to the value the provider shows, starting with whsec_.` `secrets_list` answers `svixKey`, whether each value decodes as such a key, so the Validation Gate makes the same refusal before Deploy without seeing the value. The `secret_set` rule for values applies, so a provider secret with a space cannot be set; give the provider one without.

The Kernel records an accepted delivery before the action runs, in a `route_deliveries` table of the Deployment's `identity.sqlite` that a restart keeps and an export carries, so two copies sent at once do not both run. When the action then fails, for a missing input, a SQL error or a guard, the Kernel forgets the delivery, so the provider's retry of the same body runs. `hmac-body` signs the body alone, so the same body to another route with the same secret verifies there too; give each provider route its own secret. A body-only MAC cannot tell a delivery replayed after 72 hours from a new one. The provider's event ID in a `UNIQUE` column, with `ON CONFLICT DO NOTHING`, is the lasting guard, and every example above uses one.

`fields` maps an input key to a JSON path into the POST body: `$`, then 1 to 16 steps of `.name` (letters, digits, `_` and `-`) or `[index]`, up to 200 characters, such as `$.data.object.id` or `$.commits[0].id`. With it the route reads each input fields names from its path and every other input from the body's key of the same name, and leaves out every other key of the body. A path the body does not have leaves its input out, so a required input answers `Missing action input: <key>` and a nullable one binds `NULL`. A value of the wrong type is refused as any input is, such as `id must be a non-empty string.` for a number; declare a number input for a provider's numeric ID. `fields` works on any POST route, signed or not, and a key or a session caller sends the same nested body. Without `fields` a route keeps refusing undeclared keys.

The Validation Gate and the Kernel refuse the same declarations with the same messages, from `kernel-rs/src/http-binding-cases.json`, checking the scheme, the keys it takes, the method, the header, the secret and the scheme's settings in that order, then `fields` in sorted key order:

| Declaration | Message |
| --- | --- |
| A scheme other than the four | `Action <name> names an unknown signature scheme. Use "littlebox", "hmac-body", "timestamped" or "svix".` |
| A key the scheme does not take, such as `header` on `svix` or a secret value | `Action <name> has an invalid <scheme> signature. It takes "scheme", "header" and "secret".`, with the scheme's own list |
| A provider's scheme on a `GET` route | `Action <name> binds GET, so its signature must use the littlebox scheme. A provider's scheme signs a POST body.` |
| No header, or one the Kernel reads itself | `Action <name> has an invalid signature header.` |
| `hmac-body` with no or another `algorithm` | `Action <name> has an invalid signature algorithm. Use "sha256" or "sha1".` |
| `hmac-body` with no or another `encoding` | `Action <name> has an invalid signature encoding. Use "hex" or "base64".` |
| `hmac-body` with a `prefix` of other characters or over 32 | `Action <name> has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256=".` |
| `timestamped` with no `prefix`, or one that is not a name and `=` | `Action <name> has an invalid timestamp prefix. Use the field's name and "=", such as "t=".` |
| `timestamped` with no `tag`, one that is not a name and `=`, or the prefix again | `Action <name> has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's.` |
| `timestamped` with no `window`, or one that is not a whole number from 1 to 3600 | `Action <name> has an invalid signature window. Use a whole number of seconds from 1 to 3600.` |
| `fields` on a `GET` route | `Action <name> binds GET, so it cannot declare fields. Fields read a POST body.` |
| `fields` that is not an object, is empty or has more than 32 keys | `Action <name> has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id".` |
| A key no input binding declares | `Action <name> maps <key>, which is not one of its input bindings.` |
| A path that is not text, lacks `$`, is `$` alone or has another step | `Action <name> maps <key> to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id".` |

`src/lib/provider-signature.ts` signs the way each provider does, and `kernel-rs/src/provider-signature-cases.json` holds vectors both sides check, GitHub's and Svix's published examples among them. A body is read up to 64 KiB, so a large delivery such as a push with many commits answers 413. A form-encoded body, as Slack's slash commands and Twilio send, still answers 400, and a scheme with the timestamp in a header of its own, as Slack's, is not one of the four.

### API keys

```text
api_keys_create { "deploymentId", "name", "actions": ["orders.receive"], "files": ["private"] }
api_keys_list   { "deploymentId" }               metadata, revocation times and reasons, never the key
api_keys_revoke { "deploymentId", "keyId" }
```

The editor's Auth > Integrations view sets, lists and deletes secrets by name and creates, lists and revokes keys, showing a new key once with a curl for each route it names and for the file read path.

`api_keys_create` returns the key once. A key belongs to one Deployment and never to the owner, grants no administrative tool, and works only on HTTP routes and the file read path. The runtime MCP endpoint refuses a request that carries one with 403 `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.`, so a misconfigured program learns why instead of getting a visitor's answers. It may name actions that have an HTTP route and the file visibilities `public` and `private`. A key that names `private` reads every private file of its Deployment, because the owner who issues it can already read them through `project_export`. A Deployment holds at most 50 live keys. Each unknown or revoked key a client presents counts against that client address, as a wrong password does: after 10 in a minute on one Deployment, the Kernel answers `rate-limited` to every key from that address without looking it up, so guessing keys costs no more than one bucket per address. A valid key never uses the bucket up. The Kernel stores the SHA-256 hash in the Deployment's `identity.sqlite`, so `package_export` and `project_export` carry the keys' metadata and hashes with the accounts. A package leaves with every live key already revoked, and an import of either format marks any live key revoked, as it deletes sessions and challenges. `api_keys_list` on the destination shows each such key with `revokedReason: "import"`, its name, actions and file visibilities, so the owner knows which programs to issue a new key for; the editor's Integrations view lists them under "Revoked at import". A key the owner revoked has `revokedReason: null` and keeps its time. `littlebox-kernel restore` keeps keys live, since it restores the same installation.

### The file read path

`GET /_littlebox/files/<id>` on the Deployment host returns a stored file with its own content type, except that a JavaScript, HTML, XML or SVG type is sent as `application/octet-stream` so a page on the app's origin cannot load an upload as a script or a worker under `script-src 'self'`; `files_list` still reports the type the uploader gave. It sends `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; sandbox`, `Cross-Origin-Resource-Policy: same-origin` and `Cache-Control: private, no-store`. PNG, JPEG, GIF, WebP and AVIF answer `inline`; every other type answers `attachment`. A key reads the visibilities it names and, like a visitor, public files of a public Deployment, so a key that names no file visibility still reads a public avatar there; on a private Deployment it reads only what it names. A person reads by the file rules above, and a visitor reads public files of a public Deployment. A visitor with no session or key on a private Deployment gets 401 `authentication-required` before any file is looked up, as from every other runtime door of that Deployment, so the answer is the same for a public, a private and a missing file. Every other caller who may not read the file gets 404 `File not found.`, the same as for a missing or deleted file, so the answer never tells whether a private file exists. There is no listing at `/_littlebox/files/`, no unauthenticated object URL, and nothing is served from the bucket. A signature does not apply here. The app's policy allows `img-src 'self'`, so a page can embed its own files; `Cross-Origin-Resource-Policy` keeps other apps' pages from embedding them.

The editor preview serves no routes. Try them on a Development Kernel with `littlebox dev` or on a Deployment.

## Calls between apps

An action of `"kind": "call"` sends one HTTP request to another app or an outside service. The Kernel sends it from its own process, never from the browser, so a signing secret or an API key the request needs stays in the Kernel. [ADR-0019](./adr/0019-an-app-calls-another-from-the-kernel.md) records the decision; `kernel-rs/src/calls.rs` implements it.

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

`miniapp.actions.call("tasks.escalate", { taskId, title })` checks the caller and the inputs as for any action, queues the call and answers `{ callId, queued: true }`. The call's bindings may only be inputs. `body` maps each body key to one of them, or to a constant written `{ "value": "tasks" }`, a string of at most 1000 characters, a number, a boolean or null, which the caller cannot change: a `POST` sends a JSON object, a `GET` sends query parameters after the URL's own, and an input that is null or missing leaves its key out. Usage counts each queued call under `call`, by action.

`auth` is optional:

| `auth` | What the request carries |
| --- | --- |
| `{ "secret": "<name>" }` | A signature in Littlebox's [route scheme](#signatures), keyed with the secret's value, in `X-Littlebox-Signature`, or in the header `"header"` names. Each attempt signs again with a new nonce |
| `{ "key": "<name>" }` | `Authorization: Bearer <value>`, where the secret holds an API key the other app's owner issued |

A secret that signs a call to another app of the same owner holds that app's route secret, which `secret_set` refuses as a [reused value](#signatures). Set it with `"use": "call"`: such a secret verifies no route of its own app, so it may repeat another project's value, and a route that names it is refused at publish with `Action <name> signs its route with the secret <secret>, which is set for calls only. Set a route secret with a value of its own.` `secrets_list` shows each secret's `use`.

```text
secret_set { "projectId", "name": "desk_tickets", "value": "<the desk's tickets_hook value>", "use": "call" }
```

### Where a call may go

A URL is `https`, or `http` only to an app of this installation. The rules below keep a call from reaching the machine the Kernel runs on, its network, a cloud metadata service, the editor or the Kernel's own administration:

| Rule | Checked | Refusal |
| --- | --- | --- |
| An absolute `http` or `https` URL, up to 2048 characters, without a user, a password, a fragment, a space or a control character | Both doors | `Action <name> has an invalid url. Use an absolute https:// URL without credentials or a fragment, up to 2048 characters.` |
| No IP address as the host, in any notation, so `http://127.1/` and `http://[::1]/` are refused | Both doors | `Action <name> calls the IP address <host>. Name a host instead.` |
| No `localhost`, `metadata`, `instance-data` or name under `.internal` | Both doors | `Action <name> calls <host>, which names this machine or a cloud metadata service.` |
| No first path segment `_littlebox`, `oauth`, `.well-known` or `mcp`, in any case | Both doors | `Action <name> calls a path under /_littlebox, /oauth, /.well-known or /mcp, which belong to the Kernel.` |
| Not the installation's public host or its MCP host | The Kernel, at publish and before each attempt | `Action <name> calls <host>, the editor's host. A call reaches an app or an outside service, never the editor.` |
| `http` only to a host under `LITTLEBOX_HOST_SUFFIX` | The Kernel, at publish and before each attempt | `Action <name> calls http://<host>, which is not an app of this installation. Use https.` |
| Every address an outside host resolves to is public | The Kernel, before each attempt | The call fails with `address-refused` |

The Validation Gate cannot see the installation or its secrets, so the Kernel alone makes the editor-host, `http` and unset-secret refusals, as it alone refuses an unset route secret. Before each attempt to a host outside the installation, the Kernel resolves the name and refuses the call when any address is loopback, private (10/8, 172.16/12, 192.168/16), link-local (169.254/16, where cloud metadata answers), shared (100.64/10), documentation, benchmarking (198.18/15), multicast, reserved, unspecified, unique-local (fc00::/7), IPv6 link-local, or an IPv4 address of any of those carried in IPv6 (`::ffff:0:0/96`, `64:ff9b::/96`). It then connects to the address it checked, so a second DNS answer cannot move the request. It follows no redirect: a 3xx is the answer, so a redirect cannot lead past these rules. It uses no proxy from the environment.

A URL whose host is a Deployment of this installation does not leave the process. The Kernel answers it with the route code a request from outside meets, `http::call_route`: the route's callers, the signature check and its replay memory, the rate limit, the guard, Usage and the Signal, with no cookie and a client address of its own for each calling app. A host under the suffix that no app holds fails with `app-not-found`. A paused app answers 503.

### Delivery and the outbox

The worker in `calls.sqlite` sends each call within seconds. An attempt has 10 seconds from connecting to the last byte, and an answer larger than 64 KiB ends the call with `answer-too-large`. A network failure, a timeout, a DNS failure, 408, 425, 429 or a 5xx is retried, waiting 30 seconds, then 1, 2 and 4 minutes, up to 5 attempts. Any other answer is final. An app queues at most 20 calls in any 60 seconds, or `LITTLEBOX_CALLS_PER_MINUTE`; the next is refused with `This app reached its limit of calls in the last minute. Try again in a minute.` A paused app sends nothing until its pause ends.

When a call ends, delivered or failed, its `then` mutation runs once, with no actor, under the app's database lock. Each of its inputs is read by a JSON pointer from this document:

| Pointer | Value |
| --- | --- |
| `/ok` | `true` for a 2xx answer |
| `/status` | The HTTP status, or null when no answer arrived |
| `/error` | The Kernel's error kind, such as `timeout`, `address-refused` or `app-not-found`, or null |
| `/body` | The answer parsed as JSON, or null; `/body/error` is another Littlebox app's refusal |
| `/input` | The call's own inputs, so `/input/taskId` names the row the call was about |

A pointer that finds nothing, or null, leaves its input out, so it binds NULL when the input is nullable. A string input takes a number, a boolean or a structure as its JSON text. The `then` mutation must exist, be a mutation, have no HTTP route and no actor binding, and every required input must be filled; both doors refuse anything else at publish. Only the Kernel runs it: `miniapp.actions.call` refuses it with `Action <name> runs only after the call <call> answers.` It counts in Usage as an action, or under `refused` when its guard refuses, and records its Signal with `actor` null.

A call's `then` mutation processes its declared `notify` when it changes a row, using the inputs mapped from the answer. It uses the same queue, budgets and Usage outcomes as other mutations. A notice to `owner` reaches the project owner. A notice to `actor` records `no-account` because the answer has no caller. A refused or unchanged mutation sends nothing.

`calls_read { "deploymentId", "cursor"? }` needs `read` and lists the app's calls newest first, 50 at a time: the action, `pending`, `delivered` or `failed`, attempts, method, URL, status, error kind, the first 1 KiB of the answer and the `then` mutation's outcome (`done`, `refused` with the guard, or `failed` with the reason). The outbox keeps the newest 1000 finished calls per app. Pending calls do not travel in a package or a migration; on the destination, the page invokes them again, and a retired origin ends its pending calls with `app-moved`. Scheduled copies carry `calls.sqlite`, so a restore brings the outbox back as the copy held it.

### A page that reads another app

`"connects": ["https://desk-4f2a9c.apps.example.org"]` in the project manifest, or `project.manifest.connects` in `littlebox.json`, adds those origins to the page's `connect-src`, so its `fetch` may read another app's public route directly. A `GET` route that answers anyone, an unsigned route on a public Deployment with no `callers`, answers with `Access-Control-Allow-Origin: *`, since every caller already gets the same rows. A route that takes a key, a session or a signature sends no CORS header. A key never goes to the browser: a request that needs one is a call action. `connects` lists at most 10 origins, each exactly `https://host` or `https://host:port`, and `http` only for a host under `localhost`, which a local installation serves over http. Both doors refuse anything else with the same words:

| Declaration | Message |
| --- | --- |
| Not a list of at most 10 strings | `connects must be a list of at most 10 origins, such as ["https://api.example.com"].` |
| A path, a trailing slash, a wildcard, capitals, the default port written out, or `http` outside `localhost` | `connects lists <entry>, which is not an origin. Write it as https://host or https://host:port, with no path; http only for a host under localhost.` |
| An origin twice | `connects lists <entry> twice.` |

### Declarations both doors refuse

The Validation Gate and the Kernel parse calls and `connects` from one case file, `kernel-rs/src/call-cases.json`, with these messages besides the URL rules above:

| Declaration | Message |
| --- | --- |
| A query, mutation or signal action with `url`, `method`, `body`, `headers`, `auth` or `then` | `Action <name> is a <kind>, so it cannot declare url, method, body, headers, auth or then. Those keys belong to an action of "kind": "call".` |
| A call with a statement, signal, guard or http | `Action <name> is a call, so it runs no SQL and cannot declare a statement, a signal, a guard or an http route.` |
| A call that binds the actor, the time or a literal | `Action <name> is a call, so its bindings may only be inputs.` |
| A method other than `"GET"` or `"POST"` | `Action <name> must call with "method": "GET" or "POST".` |
| A `body` that is not an object of at most 32 keys of 1 to 64 letters, digits, `_`, `.` and `-`, each naming an input or a constant | `Action <name> has an invalid body. Map each body key to one of the call's input keys or to a constant written as an object with "value", at most 32.` |
| A body value that names no input | `Action <name> sends body key <key> from <input>, which is not one of its input bindings.` |
| An `auth` with both or neither of `secret` and `key`, or another key | `Action <name> has an invalid auth. Name a secret with "secret" to sign the request, or with "key" to send the API key it holds.` |
| A secret that is not a secret name | `Action <name> names an invalid auth secret. Use lowercase letters, digits and "_", starting with a letter.` |
| A header the Kernel sets itself, or not 1 to 64 letters, digits and `-` | `Action <name> has an invalid signature header.` |
| A `then` that is not `{ "action", "inputs" }` of strings | `Action <name> has an invalid then. Name a mutation and map its input keys to JSON pointers into the answer.` |
| A pointer outside the document, or with a `~` not followed by `0` or `1` | `Action <name> reads <pointer> for then, which is not a JSON pointer into /ok, /status, /error, /body or /input.` |
| `/input/<key>` for an input the call lacks | `Action <name> reads <pointer> for then, but <key> is not one of its input bindings.` |
| A `then` action that is missing, not a mutation, routed, or binds the actor | `Action <name> names <target> as its then, which is not declared.`, `... which is not a mutation.`, `... which answers on an HTTP route. A then mutation runs only after its call.`, `... which binds the actor. A then mutation runs with no caller.` |
| A `then` input the mutation lacks, or a required one left out | `Action <name> fills <key>, which is not an input of <target>.`, `Action <name> leaves <key> of <target> unfilled. Fill it from the answer or make the input nullable.` |

The editor preview answers a call with `{ callId, queued: true }` and sends nothing. Try calls on a Development Kernel with `littlebox dev`, or on a Deployment. The [Action Contract](#action-contract) lists a call with its request and its `then`, and `conform` checks its refusals without sending one.

## Usage and Signals

The owner's Usage page at `/usage` reads all accessible projects from the Kernel, including external projects and unpublished drafts. Open one published app for feature activity, execution outcomes, business event totals and retained delivery states. The page offers 7, 30 and 90 days and refreshes every minute while visible. A notice the Kernel could not queue shows as not sent, with its reason, on the overview and on the app.

```text
usage_summary  { "deploymentId", "from", "to" }
usage_overview { "from", "to", "cursor"? }
```

Both tools accept inclusive UTC dates and the same 400-day range limit as `usage_read`. The overview returns at most 50 projects and `nextCursor`. Its summaries aggregate operation kinds and execution outcomes across the period; per-action daily detail belongs to `usage_summary`. Totals carry an empty name, except `notify`, which keeps one row per notice outcome named `*: <reason>`, so the overview can tell a queued notice from one that was never sent. It checks the account owner and token project grants before reading summaries. Unpublished projects have `summary: null`; an unreadable published store also has an `error`, rather than appearing unused.

`usage_summary` returns `usage` with the existing counters, `actions` from the published Deployment, `metrics` with daily execution and Signal aggregates, retained `calls` and `mail` state counts, `coverage` and `updatedAt`. Execution origins are `runtime`, `route`, `schedule` and `continuation`; outcomes are `ok`, `refused`, `failed` and `unchanged` for scheduled work without changes. A route retains the action's name and counts once in this execution table. The existing operation table still counts it only under `route`.

Coverage includes `queueFlushed` to say whether earlier queued records finished writing before the read. A false value means recent activity may be missing. Coverage also names when new aggregates began, the earliest retained metric day, and the last day affected by pruning as `prunedThrough`. A null capture start means the old store has not received a new write. Earlier operation counters may exist. Dropped-entry and write-error counts cover the installation since restart. Retained call and mail states describe jobs created in the requested period; records already removed cannot contribute. These tools return neither Signal properties nor request bodies.

Each Deployment keeps Usage and Signals in its own `usage.sqlite`, next to its database and accounts. [ADR-0010](./adr/0010-kernel-owns-usage-and-signals.md) records the decision; `kernel-rs/src/usage.rs` implements it. Only the owner reads them, through two tools that need `read` and the project grant. The runtime endpoint does not list them, so an app cannot read its own Usage.

```text
usage_read   { "deploymentId", "from": "2026-11-01", "to": "2026-11-06" }
signals_read { "deploymentId", "name"?: "order.placed", "cursor"? }
```

`usage_read` returns one row per UTC day, kind and name between two dates, both included, at most 400 days apart, the totals over the range, and the newest 500 [scheduled runs](#scheduled-work) in it:

```json
{ "from": "2026-11-01", "to": "2026-11-06",
  "days": [{ "day": "2026-11-06", "kind": "action", "name": "notes.add", "count": 12 }],
  "totals": [{ "kind": "action", "name": "notes.add", "count": 12 }],
  "scheduled": [{ "action": "holds.release", "at": 1762423200000, "outcome": "ok", "changes": 2 }] }
```

| Kind | Name | Counted when |
| --- | --- | --- |
| `session` | | `auth_session` answers. The SDK calls it when an app opens |
| `action` | the action | `actions_invoke` runs the action |
| `route` | `POST /hooks/orders` | an HTTP route runs its action, which is not counted again under `action` |
| `upload` | | `files_upload` stores a file |
| `file_read` | | `files_read` or `GET /_littlebox/files/<id>` returns a file |
| `signup` | | an account becomes active: a signup in local mode, an answered verification link or an accepted invitation |
| `login` | | a password sign-in or an answered recovery link |
| `refused` | the action | a mutation with a [guard](#guards) changed no row and the Kernel answered the guard, on `actions_invoke` or on its route |
| `notify` | `<action>: queued`, or `<action>: <reason>` | a mutation with [`notify`](#notices) changed a row; the name says whether its email was queued or why it was not |
| `scheduled` | the action | the Kernel's timer ran a [scheduled](#scheduled-work) mutation, whether it succeeded or failed |
| `call` | the action | a [call action](#calls-between-apps) queued a call; the target app counts the request as its own `route` |
| `page_view` | the HTML file served, as `index.html` | a browser navigation (`Sec-Fetch-Dest: document`, not a prefetch) received one of the artifact's HTML pages with 200. A single-page app counts its first load under `index.html` |

Nothing else the Kernel refuses is counted: a call without the session it needs, an input the bindings reject, a statement SQLite refuses, a bad signature or a wrong password. A guard's refusal is the app's own rule saying no, which the owner wants to see next to the calls that ran. A scheduled run is the other exception: it counts under `scheduled` whether it succeeded or SQLite refused it, because nobody called it and the owner is the only one who can see that it failed. The Kernel writes Usage off the request path. A response never waits for it, and when more than 16,384 entries are waiting the newest are dropped and logged. The two read tools wait up to five seconds for the queue to drain, so they include every call that finished before them.

A Signal is a named occurrence a Server Action declares. It is recorded when that action succeeds, and for a mutation only when its statement changed at least one row, with properties taken from the input the action ran with:

```json
"newsletter.join": {
  "kind": "signal",
  "access": "anonymous",
  "bindings": [{ "source": "input", "key": "source", "type": "string", "maxLength": 40 }],
  "signal": { "name": "newsletter.joined", "properties": ["source"] }
}
```

A mutation may declare `signal` too, and records it after its statement changes a row; a retry that stored nothing, or a refused attempt, records none. The property `actor` records the caller's ID instead of an input: the app user's ID, the API key's ID, or `null` for a visitor or a signed request, so the owner can count per person. An action that lists `actor` may not also have an input named `actor`. An action of `"kind": "signal"` runs no SQL and returns `{ "recorded": true }`; it has no statement and no `http`. There is no `miniapp.track`. The app records a Signal with `miniapp.actions.call`, so it goes through the same access check, input check and rate limit as any action. `signals_read` returns 100 Signals at a time, newest first, as `{ "signals": [{ "name", "at", "properties" }], "nextCursor" }`; pass `nextCursor` back as `cursor` for the next page.

| Refusal | Message |
| --- | --- |
| A Signal on a query | `Action <name> is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action.` |
| A property that is not an input binding's key | `Action <name> names signal property <key>, which is not one of its input bindings.` |
| A property listed twice | `Action <name> names signal property <key> twice.` |
| `actor` listed while an input is named `actor` | `Action <name> names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input.` |
| More than 8 properties | `Action <name> names more than 8 signal properties.` |
| A name that is not dot-separated lowercase words of up to 64 characters | `Action <name> names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters.` |
| `signal` that is not `{ name, properties? }` with a list of strings | `Action <name> has an invalid signal declaration.` |
| A signal action without `signal` | `Action <name> is a signal action, so it must declare a signal.` |
| A signal action with a statement | `Action <name> is a signal action, so it runs no SQL and cannot declare a statement.` |
| A signal action with `http` | `Action <name> is a signal action, so it takes no route. To record a signal from an HTTP route, declare the signal on a mutation with a POST route.` |
| `usage_read` with a date that is not `YYYY-MM-DD` | `from and to must be dates written YYYY-MM-DD.` |
| `usage_read` with `from` after `to` | `from must not be after to.` |
| `usage_read` over more than 400 days | `Usage covers at most 400 days at a time.` |
| `signals_read` with a cursor it did not return | `Invalid cursor. Pass back the nextCursor of an earlier page.` |

The Validation Gate and the Kernel refuse the same declarations with the same messages, from one case file, `kernel-rs/src/signal-cases.json`, that both test suites run. A signal action counts toward the action limit.

### Guards

An app writes its rules into a mutation's `WHERE`: the caller is an organizer, the slot is free, the event is still open. When the rule says no, the statement changes no row, and without more the caller gets `{ "changes": 0, "lastInsertId": 0 }`, the same answer as a request that had nothing to do. A mutation may declare the refusal:

```json
"members.promote": {
  "kind": "mutation",
  "access": "authenticated",
  "guard": "Only an organizer can promote a member.",
  "statement": "UPDATE members SET role = 'organizer' WHERE user_id = ?1 AND EXISTS (SELECT 1 FROM members WHERE user_id = ?2 AND role = 'organizer')",
  "bindings": [{ "source": "input", "key": "member", "type": "string", "maxLength": 64 }, { "source": "actor", "field": "id" }]
}
```

When the statement changes no row, the Kernel answers the guard as a refusal: a tool error with that text on `actions_invoke`, so `miniapp.actions.call` rejects with it, and `403 { "error": "<guard>" }` on the action's route. Usage counts it under `refused` and no Signal is recorded. The statement has run, and it changed nothing, so nothing is rolled back. Without a guard a mutation answers `{ changes: 0 }` as before. The editor preview refuses the same way. The Action Contract lists the guard among the action's errors, and `conform` probes it.

| Declaration | Message |
| --- | --- |
| `guard` on a query | `Action <name> is a query, so it cannot declare a guard. Declare it on a mutation.` |
| `guard` on a signal action | `Action <name> is a signal action, so it cannot declare a guard. Declare it on a mutation.` |
| A guard that is not a string of 1 to 200 characters on one line | `Action <name> has an invalid guard. Use a message of 1 to 200 characters on one line.` |

The Validation Gate and the Kernel parse the same cases from `kernel-rs/src/guard-cases.json`; `kernel-rs/src/guard.rs` holds the Kernel's side.

The store keeps 400 days and at most 100,000 rows of totals, the newest 50,000 Signals and the newest 10,000 scheduled runs; past those bounds the oldest rows go first. A string property keeps its first 128 bytes. The scheduled copy and `package_export` carry the store; `project_export` does not. A Development Kernel started with `bun run littlebox -- dev` records the same way, in `.littlebox/dev-data/deployments/<id>/usage.sqlite`. The editor preview has no store: a signal action there returns `{ "recorded": true }` and nothing is kept.

### Scheduled work

Some work has to happen when nobody asks: a hold that lapsed is released so the hour shows free, a day is closed so tomorrow's page starts from a total. A mutation that takes no input may declare when the Kernel's own timer runs it ([ADR-0017](./adr/0017-the-kernel-runs-a-declared-schedule.md)):

```json
"holds.release": {
  "kind": "mutation",
  "access": "authenticated",
  "schedule": { "every": "15m" },
  "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL AND until < ?1",
  "bindings": [{ "source": "kernel", "field": "now" }]
}
```

`"every"` takes a whole number of minutes or hours, `"5m"` to `"24h"`, and periods count from 00:00 UTC, so `"15m"` runs at :00, :15, :30 and :45. `"daily": "HH:MM"` runs once a day at that UTC time. The Kernel looks once a minute and runs an action at the first look after its period opens, in the Kernel's one process. Nobody calls it, so `kernel.now` is the time of the run, literals bind as written and an `actor` binding, of the `id` or of the `role`, is null. Only the timer runs it: `actions_invoke`, the page's `miniapp.actions.call` and the editor preview refuse it with `Action <name> runs only on its schedule.`, whoever calls. A scheduled mutation that also declares an `http` route answers on that route as its callers say.

| Rule | What the Kernel does |
| --- | --- |
| Once a period | The period it last ran for is kept in the Deployment's `usage.sqlite`. A restart, a copy, a package and a migration carry it, so a daily job does not run twice in a day. A Kernel that was down for several periods runs the action once when it is back |
| Publishing | The first period an action is seen in counts as done, so publishing never runs it. A republish that changes the schedule starts again from the next period |
| One run at a time | A run takes the Deployment's data lock and has the 2-second statement budget. A Deployment's scheduled actions run one after another in name order, and its next look waits until they finish |
| Maintenance | A Deployment in maintenance runs nothing, and runs each due action once when it leaves maintenance |
| Failure | A run SQLite refuses is logged, recorded with `outcome: "failed"` and its error, and tried again at the next look. Runs never pile up |
| Guard and Signal | A run that changes no row had nothing to do: it is recorded with `changes: 0` and the guard does not apply. A run that changed a row records the action's Signal, with `actor` null |
| Notice | A run that changes a row processes `notify` through the same queue and budgets as a caller's mutation. `to: "owner"` emails the owner once per changed run, even for several changed rows. `to: "actor"` records `no-account` under Usage because the timer has no caller. Failed runs and runs that change nothing send nothing |
| Usage | Each run counts under `scheduled` by action name, and `usage_read` returns the newest 500 runs of its range with `action`, `at`, `outcome`, `changes` and, for a failed run, `error` |

The editor preview runs no schedule. A Development Kernel started with `bun run littlebox -- dev` runs them as a Deployment does. A test can set `LITTLEBOX_SCHEDULE_MIN_SECONDS` to run every schedule once every few seconds instead of at its declared times; the Kernel refuses to start with it unless `LITTLEBOX_BIND` is a loopback address.

Nothing is pushed to a page. A page that shows new rows polls a query with an `after` cursor, and a `subscribe` key is refused at both doors.

| Declaration | Message |
| --- | --- |
| `schedule` on a query | `Action <name> is a query, so it cannot declare a schedule. Declare it on a mutation.` |
| `schedule` on a signal action | `Action <name> is a signal action, so it cannot declare a schedule. Declare it on a mutation.` |
| `schedule` on a mutation with an input binding | `Action <name> declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values.` |
| A schedule that is not one of `every` and `daily` with a string, or an `every` that is not 1 to 5 digits and `m` or `h`, such as `"0 8 * * *"` | `Action <name> has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00".` |
| `every` below 5 minutes | `Action <name> runs every 4m, more often than every 5 minutes. Use "5m" or longer.` |
| `every` above 24 hours | `Action <name> runs every 25h, less often than once a day. Use "daily" or a period of 24 hours or less.` |
| `daily` that is not a time from `00:00` to `23:59` | `Action <name> has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59.` |
| `subscribe` on any action | `Action <name> declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead.` |

The Validation Gate and the Kernel parse the same cases from `kernel-rs/src/schedule-cases.json`; `kernel-rs/src/schedule.rs` holds the Kernel's side, the timer and the run.

### Notices

A mutation may send one email when its statement changes a row. [ADR-0016](./adr/0016-a-mutation-may-notify-the-owner-or-the-actor.md) records the decision; `kernel-rs/src/notify.rs` implements it.

```json
"bookings.create": {
  "kind": "mutation",
  "access": "authenticated",
  "statement": "INSERT INTO bookings (user_id, room, day) VALUES (?1, ?2, ?3)",
  "bindings": [{ "source": "actor", "field": "id" }, { "source": "input", "key": "room", "type": "string", "maxLength": 40 }, { "source": "input", "key": "day", "type": "string", "maxLength": 10 }],
  "notify": { "to": "actor", "subject": "Booked: {input.room} on {input.day}", "body": "Your booking of {input.room} on {input.day} is confirmed." }
}
```

`to` is `owner`, the email of the Project Owner's active account, or `actor`, the email of the calling app account, only when that account proved it by answering a verification, invitation or recovery link. The Kernel never sends to an address from the input, so an app cannot relay mail to whoever a visitor names. The subject and body are templates. Their placeholders are `{input.<key>}`, for one of the action's input bindings, `{actor.id}`, the caller's ID, `{kernel.now}`, the time of the call or scheduled run as `2026-11-08 07:00 UTC`, `{changes.count}`, the rows the statement changed, and, in the body only, `{changes}`. A value loses its line breaks and keeps 200 characters; a missing or null value fills nothing.

`{changes}` lists the changed rows, one line each, with the columns named in `"changes"` joined by ` · `: at most 50 lines, then `- and <n> more`. `"to": { "column": "assignee_id" }` sends one notice to each account whose ID a changed row holds in that column, listing only that account's rows. The Kernel reads the rows back by appending its own `RETURNING` to the statement, so the columns belong to the table the statement changes, and the answer stays `{ changes, lastInsertId }`. A column value is an account ID, never an address: the Kernel looks it up among this Deployment's active accounts that answered a link, and a value that is not one sends nothing and counts as `not-a-verified-account`. One run reads at most 1,000 rows and writes to at most 50 accounts, in account ID order; each account past the 50th counts as `recipient-cap`. See [ADR-0021](./adr/0021-a-notice-lists-its-rows-and-reaches-the-accounts-they-name.md).

```json
"tasks.remind": {
  "kind": "mutation", "access": "authenticated", "schedule": { "daily": "07:00" },
  "statement": "UPDATE tasks SET reminded_at = ?1 WHERE due_day = date(?1 / 1000, 'unixepoch') AND reminded_at IS NULL",
  "bindings": [{ "source": "kernel", "field": "now" }],
  "notify": { "to": { "column": "assignee_id" }, "changes": ["title"],
    "subject": "{changes.count} tasks due today", "body": "Due on {kernel.now}:\n{changes}" }
}
``` The body keeps the template's own line breaks, and the HTML part escapes every value. The Kernel adds one last line naming the app and why the recipient gets it.

The notice is recorded only when the statement changed a row, after the answer is decided: a guard's refusal, an `INSERT ... ON CONFLICT DO NOTHING` that stored nothing and a failed statement send nothing. The mutation answers `{ changes, lastInsertId }` whether or not the notice can be sent. Usage counts every notice under `notify`: `bookings.create: queued` when it became a job, and otherwise the reason:

| Reason | What happened |
| --- | --- |
| `no-transport` | The installation has no mail transport. Nothing is stored |
| `owner-has-no-email` | `to` is `owner` and the owner signs in through Clerk, so the Kernel has no address |
| `no-account` | `to` is `actor` and the caller is a visitor, a signed request or an API key |
| `unverified-email` | `to` is `actor` and the account has not answered a link, or is not active |
| `not-a-verified-account` | `to` names a column and a row's value is not the ID of an active account of this Deployment that answered a link |
| `recipient-cap` | `to` names a column and the run already wrote to 50 accounts |
| `app-limit`, `creator-limit`, `recipient-limit`, `installation-limit` | A day's budget is used up. Nothing is stored |
| `sender-not-ready` | The sender preflight was not ready. The job is stored as failed with this kind and never sent |
| `storage-error` | The queue could not be written. The Kernel logs it |

A queued notice is a job in the same queue as account email, with purpose `notify`, its text sealed with the queue's key until the provider accepts it. It is sent like any job, retried the same way, held while its app is paused for a migration, and fails as `notice-expired` when it has not left within a day. The owner reads its state with `mail_status`, whose `queue.notices` counts notice jobs per state, and each notice with `notices_read { "deploymentId", "cursor"? }`, which needs `read` and lists that app's notice jobs newest first, 50 at a time: the action, `pending`, `accepted`, `delivered` or `failed`, attempts, the failure kind, and when it was queued, last changed and, while pending, next tried. It names no recipient. A notice the Kernel did not queue is no job; Usage counts it under `notify` with its reason. The editor's What happened panel reads it with `calls_read`, `usage_summary` and `signals_read`. Notices count apart from account email so a busy app never uses up the allowance signup and recovery need: 200 a day per app, 500 per creator and 50 a day per recipient from one app; the installation's 1,000 a day counts every email. The editor preview sends no email. The Action Contract lists the notice on its action.

| Declaration | Message |
| --- | --- |
| `notify` on a query | `Action <name> is a query, so it cannot declare notify. Declare it on a mutation.` |
| `notify` on a signal action | `Action <name> is a signal action, so it cannot declare notify. Declare it on a mutation.` |
| Not an object of `to`, `subject` and `body` strings, or another key such as `cc` | `Action <name> has an invalid notify declaration. Give "to", "subject", "body" and, to list changed rows, "changes", and nothing else.` |
| `to` that is not `owner`, `actor` or `{ "column": "<name>" }`, such as an address or `{input.email}` | `Action <name> has an invalid notify recipient. Use "owner", "actor" or {"column": "<name>"} for a column of account IDs.` |
| `changes` that is not a list of 1 to 8 different column names | `Action <name> has an invalid notify changes list. Name 1 to 8 columns of the changed table, each once, in letters, digits and underscores.` |
| A subject that is blank, over 120 characters or holds a line break | `Action <name> has an invalid notify subject. Use 1 to 120 characters on one line.` |
| A body that is blank, over 2000 characters or holds a control character other than a line break | `Action <name> has an invalid notify body. Use 1 to 2000 characters, with line breaks and no other control characters.` |
| A placeholder other than `{actor.id}`, `{kernel.now}`, `{changes}`, `{changes.count}` and `{input.<key>}` of an input binding, such as `{actor.email}` | `Action <name> names an unknown placeholder {<placeholder>} in its notify <subject or body>. Use {input.<key>} for one of its input bindings, {actor.id}, {kernel.now}, {changes} or {changes.count}.` |
| `{changes}` in the subject | `Action <name> puts {changes} in its notify subject. Put {changes} in the body, and {changes.count} in the subject.` |
| `{changes}` in the body with no `changes` list | `Action <name> uses {changes} in its notify body without a changes list. Name the columns to show in notify "changes".` |
| A `changes` or `to` column the changed table does not have, checked against the schema | `Action <name> names a notify column its statement cannot return. Name columns of the table the statement changes.` |
| A `{` with no `}` after it | `Action <name> has an unclosed placeholder in its notify <subject or body>. Close it with "}".` |

The Validation Gate and the Kernel parse the same cases from `kernel-rs/src/notify-cases.json`, and check columns against a schema from `kernel-rs/src/notify-column-cases.json`.

## Build and publish

```sh
bun run littlebox -- build --dir /tmp/my-app
bun run littlebox -- publish --dir /tmp/my-app
```

A published app runs under the Kernel's Content Security Policy, described in [the architecture](./architecture.md#isolation-of-hosted-code). The inline scripts in an artifact's HTML run because the Kernel allows each by its hash. Inline event handler attributes, scripts from other origins and requests to other origins do not. Attach listeners in JavaScript and serve every script and request from the app's own origin.

The build lives in `.littlebox/artifact.json`. Publishing transfers it over MCP, validates a plan against the project revision and applies it. The tool saves the plan and the receipt in `.littlebox`. The Kernel rejects a stale plan. Repeating a plan that was already applied returns its receipt. `publish` deploys as private unless `littlebox.json` sets `project.manifest.visibility` to `"public"`. A republish keeps the URL and database and stops serving files that the new artifact no longer contains.

`project.manifest.signupPolicy` in `littlebox.json` names who may create an app account when the first release goes out, with the modes of [Signup policy](#signup-policy):

```json
{ "url": "http://localhost:4173", "project": { "id": "app_…", "name": "Tasks", "slug": "tasks",
  "manifest": { "version": 1, "visibility": "private", "signupPolicy": { "mode": "invitation" } } } }
```

`allowlist` is a list of emails and `@domain` entries, needed only with `"mode": "allowlist"`. `init` writes `"visibility": "private"` and `{ "mode": "invitation" }`, so a new app admits only the people its owner invites. On a first release `publish` sends the policy as `initialSignupPolicy`, which needs only `publish`, and the Kernel installs it before the app serves a request. On a later release it sends none, because a publish never changes who may join; it reads the app's policy with `signup_policy_read` and, when it differs, prints one warning and still publishes: `littlebox.json asks for signup Open, but this app has Invitation only. A publish sets the policy only on the first release and never changes who may join afterwards. Change it in the editor's Auth > Users view, or call signup_policy_set with a credential that holds manage_users.` A mode other than the four, or an allowlist that is not a list of strings, stops `publish` before it sends anything. Without `signupPolicy` a first release gets the Kernel's default, Open for a public app and Closed for a private one. `dev` publishes its local copy as Open so you can sign up test accounts. The policy is not part of the Capsule manifest, so `publish` leaves it out of the project it sends.

### What the build publishes

`littlebox build`, the editor's Publish and the exported kit's `npm run build` run one set of rules, `src/lib/static-artifact.js`, so the same Capsule gives the same artifact from every door.

- A page is every `.html` file at the Capsule root and every `.html` file under `pages/`. A page under `pages/` is served without that prefix: `pages/guide/install.html` answers at `/guide/install.html` and `/guide/install`. An `.html` file anywhere else is not a page.
- Every file under `public/` is served as it is, without the `public/` prefix: `public/robots.txt` answers at `/robots.txt`. Put a file there when no page names it, such as `sitemap.xml`, or when a script fetches it.
- Any other Capsule file ships only when a page or a stylesheet a page loads names it: `<script src>`, `<link href>` with rel `stylesheet`, `icon`, `apple-touch-icon`, `mask-icon`, `manifest`, `preload`, `modulepreload` or `prefetch`, `src`, `srcset` and `poster` on `img`, `source`, `video`, `audio`, `track` and `embed`, and `url()` and `@import` in CSS, including `<style>` and `style=""`. A reference resolves against the page's served path, so `../logo.svg` on `/guide/install.html` names `logo.svg`. Links (`<a href>`), other origins, `data:` URLs and paths under `/_littlebox` are not followed. `schema.sql`, `actions.json` and every other file no page names stay out of the artifact.
- A page that is a fragment, with no doctype and no `<html>`, is put in the Capsule document, as `index.html` always was: `styles.css` and the app bundled from `src/main.ts` or `app.js` inline, with the SDK inline before it. A page that is a whole document keeps its own head. The build puts `<script src="/runtime.js"></script>` first in its head unless the page already loads `/runtime.js`, and adds the SDK as `runtime.js`. A `<script type="module" src>` the page names is bundled with its imports and written at the same path.
- A fragment page names its title and icon by opening with them: a `<title>` and any `<link>` whose `rel` names an icon (`icon`, `apple-touch-icon`, `mask-icon`), before its first other element. The build moves them into the head of the Capsule document, where the browser reads them; without an icon the browser asks for `/favicon.ico`, which a Capsule rarely holds. A `<title>` further down, such as one inside an `<svg>`, stays where it is.
- A Capsule without `index.html` gets the fragment `<main id=app></main>`.
- The build drops the comment esbuild writes above each module it bundles, such as `// app.js`, when the line names a Capsule file. That name is relative to the folder the build ran in, so without this the published script would carry the builder's directory layout. The same Capsule built from two folders gives byte-identical files, and the CLI, the editor and the exported kit publish the same `app.js`.
- A binary file, such as an image or a font, travels as base64 and the artifact names it in `encodings`, for example `"encodings": { "logo.png": "base64" }`. The CLI reads any file. A Managed Project holds text files only, so the editor publishes text assets, and a page there that names an image the project does not hold is refused with the same words as the CLI.
- An icon that must work from every door is SVG: a text file such as `favicon.svg`, or a `data:` URL such as `<link rel="icon" href="data:image/svg+xml,...">`. The Content Security Policy allows `data:` images, and the build follows no `data:` URL, so it needs no file. A binary `favicon.ico` or `.png` publishes from the CLI and the kit only.

The build refuses, with the file and the path in the message:

| Refusal | When |
| --- | --- |
| `about.html references /team.jpg, but the Capsule holds no team.jpg.` | A page or stylesheet names a file the Capsule does not hold |
| `guide/a.html references ../../x.js, which leaves the Capsule.` | A reference climbs above the Capsule root |
| `about.html and pages/about.html would both publish at /about.html. Keep one.` | Two Capsule files land on one path |
| `public/_littlebox/x.txt would publish at /_littlebox/x.txt, under /_littlebox, which the Kernel keeps.` | A file lands under `/_littlebox`, `/oauth`, `/.well-known` or `/mcp` |
| `The Capsule holds runtime.js, the name the build gives the Littlebox SDK. Rename it.` | The Capsule has its own `runtime.js` or `public/runtime.js` |
| `index.html loads /src/main.ts as a module. A page loads JavaScript, so name a .js or .mjs file and let the build compile it.` | A module script names a file that is not `.js` or `.mjs` |
| `app.js imports CSS. Link the stylesheet from the page instead.` | A document page's module imports a stylesheet |
| `Routing must be "pages" or "spa".` | `routing` in the manifest has another value. The Kernel refuses it at `release_plan` with the same words |

### How the Kernel answers a path

`project.manifest.routing` in `littlebox.json`, or `routing` in the manifest of a release, chooses how a path the artifact does not hold is answered. It is `"pages"` when absent.

| Request | `"pages"` | `"spa"` |
| --- | --- | --- |
| A file the artifact holds, such as `/about.html` or `/logo.png` | The file with its content type | The same |
| `/about`, with no extension | `about.html` when the artifact holds it | `index.html` |
| `/guide/` | `guide/index.html` when the artifact holds it | `index.html` |
| Anything else | 404 with the artifact's `404.html`, or a small Kernel page when there is none | `index.html` with 200 |
| A path under `/_littlebox` | Never an app file | Never an app file |

An HTTP route still wins over an artifact file at its path for its method. `"spa"` is how every unknown path answered before 2026-11-06; use it for an app that routes in the browser.

The content type follows the extension: HTML, CSS, JavaScript, JSON and source maps, `webmanifest`, text, Markdown, CSV, XML (`application/xml`), RSS, Atom, SVG, PNG, JPEG, GIF, WebP, AVIF, ICO, PDF, MP4, WebM, MP3, WASM, and WOFF2, WOFF, TTF and OTF fonts. Any other file is `application/octet-stream`, and every file is sent with `X-Content-Type-Options: nosniff`. A file sent with 200 carries `Cache-Control: public, no-cache` on a public Deployment and `private, no-cache` on a private one, and an `ETag` that is the artifact version, or a hash of the bytes when the Deployment has no version. A browser keeps the file and asks again with `If-None-Match`; the Kernel answers 304 with no body until the next publish changes the tag. Nothing is marked `immutable`, because a file name need not carry a content hash. A `HEAD` on an artifact file answers its headers with no body. Every HTML page, the second page and `404.html` included, gets the Content Security Policy with the hashes of its own inline scripts, and `/runtime.js` loads under `script-src 'self'`.

The 16 MiB artifact limit counts the bytes the Kernel stores, after base64 is decoded. The Kernel refuses an encoding other than base64, an encoding for a file the artifact does not hold, and a file marked base64 that does not decode, each with the path in the message.

There is also `bun run littlebox -- login --dir /tmp/my-app --url http://localhost:4173`. It reads `LITTLEBOX_EMAIL` and `LITTLEBOX_PASSWORD` from the environment, obtains a 24-hour credential with `read`, `write` and `publish` and stores it with `0600` permissions in `.littlebox/credentials.json`, which `init`, `publish`, `export` and `eject` read when `LITTLEBOX_MCP_TOKEN` is unset. `--manage-users`, `--export-data` and `--manage-lifecycle` add those permissions; a delegated token cannot add one to itself later, so ask for every permission the work needs. `LITTLEBOX_INVITATION_CODE` lets it create the account if it does not exist yet, and `LITTLEBOX_SETUP_KEY` does so for an installation's first owner. On an installation that requires verification, creating the account sends a link, and `login` stops with the two steps: open the link, choose `LITTLEBOX_PASSWORD` as the password, then run `login` again without the code or key, which are used by then. On a development Kernel on the same machine, `--verify-from <data folder>/mailbox.jsonl` does both in one run: after the signup it waits up to 15 seconds for the newest creator verification line for that email in the development mailbox, answers it with `LITTLEBOX_PASSWORD` and goes on to save the credential. The flag refuses any Kernel URL that is not `localhost`, a `.localhost` name or a loopback address, because the mailbox is a file on the Kernel's machine and only the development transport writes it. The command adds `.littlebox/` to `.gitignore`; do not copy credentials into the frontend.

## Custom domain

A Deployment can answer on one hostname its owner controls, such as `notes.example.org`, next to its generated `<slug>-<id>.<suffix>`, which keeps answering. [ADR-0015](./adr/0015-a-deployment-answers-on-one-custom-domain.md) records the decisions.

1. Point the hostname at the installation's proxy, with a CNAME to the installation's host or an A record. The proxy terminates TLS for it and forwards `Host` unchanged. Littlebox issues no certificate.
2. Call `domain_set { deploymentId, hostname }`. It checks the name and answers with the record to add:

   ```json
   { "deploymentId": "…", "hostname": "notes.example.org", "status": "pending",
     "url": "https://notes.example.org", "generatedUrl": "https://notes-1a2b3c.apps.example.com",
     "record": { "type": "TXT", "name": "_littlebox.notes.example.org", "value": "littlebox-verify-3f0c…" },
     "dns": "No TXT record at _littlebox.notes.example.org yet.",
     "next": "Add a TXT record _littlebox.notes.example.org holding littlebox-verify-3f0c…, then call domain_set again with notes.example.org." }
   ```

3. Add the TXT record at the DNS provider and call `domain_set` again with the same hostname. The Kernel looks the record up itself, over UDP, from `LITTLEBOX_DNS_RESOLVER` (an IP address with an optional port) or the first `nameserver` in `/etc/resolv.conf`. When one of the record's values is the token, the answer is `status: "active"` and the alias serves. A record that holds other values refuses the call, except on the call that issued the token, which answers `pending` and names what the record holds.

`bun run littlebox -- domain --deployment <id> --set <hostname>` runs steps 2 and 3 and prints the record; `--clear` removes the alias and no flag shows it. The CLI checks the name with `src/lib/domain-name.ts` first, by the same rules and words as the Kernel.

On the alias the app has its own origin. The runtime endpoint takes requests whose `Origin` is the alias, cookies are host-only, with the `__Host-` name under HTTPS, so a session made on the alias stays there and one made on the generated hostname stays there. The Content Security Policy's `'self'` is the alias. HTTP routes, `GET /_littlebox/files/<id>` and the maintenance page answer on it, and the SDK calls `/_littlebox/mcp` on whichever host the page loaded from. Over HTTPS it gets `Strict-Transport-Security: max-age=31536000`, without the `includeSubDomains` the installation's own hosts get. Verification, recovery and invitation email links use the alias once it serves. Administrative MCP, owner sign-in and OAuth refuse a request whose `Origin` is any claimed name.

`domain_read { deploymentId }` returns the same answer without a lookup, plus the installation's `rules`. The token belongs to the Deployment and stays when the hostname changes, so to switch names, add the record for the new name first; the old name stops serving when `domain_set` names the new one. `domain_clear { deploymentId }` deletes the alias and its token. A paused app refuses both changes. The Kernel checks DNS only when `domain_set` runs: a name stays verified until it is cleared, and a name whose DNS points elsewhere no longer reaches the Kernel.

| Refusal | Message |
| --- | --- |
| A name that is not a DNS name, such as a URL, a port, `_` or `*` | `<name> is not a valid DNS name. Use labels of letters, digits and hyphens, 1 to 63 characters each, that do not start or end with a hyphen.` |
| One label, an IP address, a local name | `<name> needs at least two labels, such as notes.example.org.` / `<name> is an IP address. Name a hostname under a domain you control.` / `<name> is a local name. …` |
| A name under the installation's suffix | `<name> is under this installation's app suffix <suffix>, where Littlebox names apps itself. Name a hostname outside it.` |
| The editor's or the MCP host | `<name> is this installation's own host. Name a hostname no Littlebox host uses.` |
| A TXT record that holds another value | `The TXT record at _littlebox.<name> holds "<value>", not this Deployment's token. Set it to <token> and call domain_set again.` |
| A name another Deployment serves | `<name> already serves another Deployment on this installation. Clear it there with domain_clear first.` |
| `domain_set` or `domain_clear` without `publish` | `Unknown tool or insufficient permission. domain_set needs a credential with publish. …` |

Two Deployments may both wait on a name; the first whose token appears in the record gets it, and the other is then refused. `kernel-rs/src/domain-cases.json` lists every rule with its message.

## Action Contract

```sh
bun run littlebox -- contract --dir /tmp/my-app
bun run littlebox -- conform --url http://my-app-ab12.localhost:8796 --contract /tmp/my-app/action-contract.json --token <app session token>
```

`contract` reads `actions.json` and `schema.sql` and writes three files beside `littlebox.json`, or into `--out`: `action-contract.json`, its JSON Schema `action-contract.schema.json`, and `action-contract.md` for a person to read. The Markdown has a Signal section for each action that records one, naming its properties and when it is recorded. `build` and `publish` never upload them. The export kit carries the same three under `littlebox/`, and the editor shows the Markdown in Files as `action-contract.md`, generated and read-only. `src/lib/action-contract.ts` writes all of them.

For each Server Action the contract states:

| Field | What it holds |
| --- | --- |
| `kind`, `access`, `actorBound` | Query or mutation, anonymous or authenticated, and whether a binding passes the caller's ID |
| `inputs` | Each input key with its type, whether it is required or nullable, and a string's `maxLength` |
| `output` | `rows` with the columns a query returns, each with its SQLite affinity, nullability and the table column it reads; or `changes` for `{ changes, lastInsertId }`. `signal` names the Signal a `signal` action records |
| `signal` | For an action that records a Signal (ADR-0010): the Signal's name and the input keys copied into its properties, with `actor` for the caller's ID |
| `guard` | For a mutation that declares one: the refusal it answers when its statement changes no row, also listed under `errors` with status 403 on a route |
| `runtime` | The endpoint, `/_littlebox/mcp`, the `actions_invoke` tool and the callers it accepts |
| `http` | The route's method and path, how the input is read, the signature header, secret name and scheme, the callers it takes, the declared `callers` list and `onlyDoor`, when the route is the action's only door |
| `errors` | The exact messages the Kernel answers with for this action, where they are seen, and the HTTP status on a route. A `GET` route lists `A route does not answer HEAD. Send GET.` with 405: the Kernel refuses `HEAD` before any SQL with `Allow: GET`, and HTTP drops the body of a `HEAD` answer, so a caller sees only the status and the header |
| `limits` | The top-level `LIMIT`, or that the query returns one row, the 1000-row and 2 MiB cap, the 2-second statement deadline, the two rate limits and a route's 64 KiB body |
| `statement`, `bindings` | The SQL and the order its parameters bind in, so the contract converts back to the same `actions.json` program |

Column names follow SQLite: the alias, or the column's own name. An expression without `AS`, such as `count(*)`, gets a key only SQLite would print, so the contract lists it under `unnamed`, `contract` prints a warning and the Validation Gate passes the Capsule with the same warning. Types come from `schema.sql` and SQLite does not enforce them. A column of a subquery or a table-valued function has a name and the type `unknown`. Error messages come from the SDK contract, whose test checks each one against the Kernel's source.

`conform` is what a replacement backend would run. It calls every action through the runtime endpoint with generated inputs and checks the answer's shape, then probes each refusal the contract lists: an undeclared action, an undeclared, missing, mistyped, empty or overlong input, a caller without a session or without an ID, and on a route an unsigned, stale or replayed signature, a caller with no key or session where the route needs one, an unknown API key, a key the route does not take, an undeclared query or body key, a body that is not JSON, a POST to an unbound path, and a `HEAD` on a `GET` route, checked by its status alone since the answer has no body. A guarded mutation must answer its guard when its statement changes no row: `conform` sends the valid input, and again when the first call changed a row, and fails a backend that answers `{ changes: 0 }` instead; when both calls change a row it reports the check as skipped. A caller without the role a guard checks meets that guard on every call, so a guard refusal passes the valid call and the probes that omit a nullable input: the bindings accepted the call and the app's rule refused it. An action whose route is its only door is checked to refuse `actions_invoke` with `Action <name> answers only on its HTTP route, <method> <path>.`, and its inputs are probed on the route. A refusal passes only when the contract lists the message and the backend answers with it, with the contract's status on a route. `--token` carries an app session token, the value of the app session cookie; `LITTLEBOX_APP_EMAIL` and `LITTLEBOX_APP_PASSWORD` sign in for one instead. `--secret <name>=<value>` signs a route and may repeat, with a fresh nonce for each request. `--key` sends an API key to a route that takes one when no secret signs it, and an action that binds the caller's ID gets the key or, with `--token`, the session cookie. The generated inputs are a short unique string, the number 1 and `true`; a statement that reads a string as more than text, such as `json_each(?1)`, needs a real value, which `--inputs <file>` gives as JSON keyed by action name, for example `{ "items.jsonin": { "ids": "[1, 2]" } }`. A valid call refused with a message the contract does not list fails and says so. A check that needs a credential it was not given is reported as skipped, not passed. It runs every mutation once with real input, so point it at a test Deployment. It exits with 1 when any check fails.

## Eject

Eject moves a Managed Project's source authority to its owner. The project becomes External: the owner builds outside Littlebox and publishes Build Artifacts through `release_plan` and `release_apply`, as for a project created with the CLI. The Deployment does not change. Its URL, accounts, sessions, data, files, invitations and email service continue, and Eject exports no data. [ADR-0005](./adr/0005-build-artifacts-separate-managed-and-external-projects.md) records the decision.

```sh
bun run littlebox -- login --dir /tmp/ejected --url http://localhost:4173 --manage-lifecycle
bun run littlebox -- eject --dir /tmp/ejected --project <project id> --url http://localhost:4173
bun run littlebox -- publish --dir /tmp/ejected
```

Only a Managed Project ejects: one the editor saves. A project made with `littlebox init` or `project_create` is External from the start, and `eject_plan` refuses it. The editor saves sources with `project_sources_save { projectId, transferId, expectedRevision }`, where the transfer holds this JSON; an agent that saves sources over MCP sends the same:

```json
{ "id": "app_notes", "name": "Notes", "slug": "notes", "updatedAt": 1762430000000,
  "files": { "index.html": "<main id=app></main>", "app.js": "...", "schema.sql": "...", "actions.json": "..." },
  "directories": [],
  "databaseModel": { "source": "", "acceptedSource": "", "revision": "r1" },
  "manifest": { "version": 1, "visibility": "private",
    "runtime": { "database": { "mode": "declared-actions", "actions": [{ "name": "notes.list", "kind": "query", "access": "authenticated" }] },
      "auth": { "mode": "optional" }, "files": { "access": "deployment" } } } }
```

A save refuses JSON without one of these parts with the part's name and its expected shape, such as `Invalid database model. Send databaseModel as { source, acceptedSource, revision } strings; an app without a Drizzle model sends empty sources.`

`eject` reads the saved-sources revision from `project_inspect`, or takes `--revision`, and calls `eject_plan`. It downloads the code package, checks its size, SHA-256 and the SHA-256 of every file, writes the files into the directory, and only then calls `eject_apply`. It saves the receipt in `.littlebox/eject-receipt.json`. The directory must be empty apart from dotfiles. A run interrupted after the plan resumes in the same directory and keeps the files it already wrote if they are unchanged.

Both tools need `manage_lifecycle`. The owner's browser session holds it; `token_create` delegates it only when asked, and `read`, `write`, `publish`, `manage_users` and `export_data` do not include it.

`eject_plan { projectId, expectedRevision }` compares `expectedRevision` with the saved-sources revision, the counter `project_sources_save` advances and `project_inspect` returns as `sourcesRevision`. It is not the release revision. A mismatch is refused before any transfer exists. The answer holds a `planId`, the `package` size, SHA-256 and content list, the `effects` Eject has, and a download `transfer`. Planning again at the same revision while the plan is current returns the same `planId` with a new download, so a client can fetch the package again after an interruption. The plan lasts one hour.

The package is JSON with `format: "littlebox-eject"`, `version: 1`, `projectId`, `sourcesRevision`, `contents` (path, size and SHA-256 of each file), `directories` and `files`. `files` holds the saved sources, `schema.ts` from the Database Model when the sources lack it, a `littlebox.json` naming the Kernel's MCP origin and the project with `sourceAuthority: "external"`, and a `README.md` (or `LITTLEBOX.md` when the sources have a README) with the CLI commands. The Kernel builds it from the stored sources, so every download of one revision has the same SHA-256. `src/lib/eject-package.ts` checks it on the client.

`eject_apply { planId }` takes the project lock that `project_sources_save` takes, checks that the plan is current and the saved-sources revision still equals the planned one, sets `sourceAuthority` to `external`, records `ejected: { at, by, via, planId, sourcesRevision }` on the project, and returns a receipt with the Deployment's ID and URL. Applying an applied plan returns its receipt. Planning again at the ejected revision returns the applied plan, its receipt and a new download.

After Eject, `project_sources_read` still returns the frozen sources with `readOnly: true`, so the editor can show them. Every Managed source change is refused with one message, whichever tab, agent or credential sends it. The audit log has one `eject_plan` row with the project as target and one `eject_apply` row per call with the project as target and `sources revision N` as detail.

| Refusal | Message |
| --- | --- |
| `project_sources_save` after Eject, at any expected revision | `This project was ejected: its sources are maintained outside Littlebox, so the Kernel refuses Managed source changes. Publish a Build Artifact with release_plan and release_apply, or create a new project to edit in Littlebox.` |
| `project_create` with `sourceAuthority: "managed"` for an ejected project | The same message |
| `project_create` with `sourceAuthority: "managed"` for a project created External | `This project is External. Littlebox does not take source authority back, so create a new project to edit in Littlebox.` |
| `eject_plan` with a stale revision | `Source revision changed: the saved sources are at revision 2, not 1. Reload the project, then plan the Eject again.` |
| `eject_apply` after a save made the plan stale | `Source revision changed since this plan: the saved sources are at revision 2, not 1. Plan the Eject again.` |
| Eject without `manage_lifecycle` | `Unknown tool or insufficient permission. eject_plan needs a credential with manage_lifecycle. Run littlebox login with --manage-lifecycle, or use a token from token_create that includes it.` A credential limited to other projects gets `Project is outside this token's grants.` |
| `eject_plan` for a project created External | `Project sources are maintained externally. A project created outside the editor is already External and does not pass through Eject.` |
| `eject_plan` at another revision after Eject | `This project was ejected at sources revision 2. Plan again with that revision to download its package.` |
| `eject_plan` with no saved sources | `This project has no saved sources to eject. Save it in the editor, or with project_sources_save, first.` |

Returning to managed authoring is not supported. To edit in Littlebox again, create a new project; production data does not merge into it.

## Deployed database models

A release may include `project.databaseModel` with `source`, `acceptedSource` and `revision`. Publishing requires equal source strings and a nonempty revision. Both planning and applying derive that model through the isolated schema helper and compare it with SQLite after the proposed additive schema runs. A model that describes another schema is refused before the live deployment changes. The Kernel persists the accepted model in `deployment.json` and returns it through `release_apply` and `project_inspect`, so reloading the editor keeps the deployed schema revision.

A release without `databaseModel`, such as a CLI publish, keeps the previous model only when it still matches the resulting schema. Otherwise the receipt has no model. For a legacy deployment without one, `project_inspect` may recover the saved model's `acceptedSource` only after deriving it and comparing it with live SQLite. It does not accept the current editable source or the saved revision label as proof. Recovery records the accepted source's SHA-256 as its revision. When derivation or comparison fails, the model remains absent and the editor keeps schema editing and publishing blocked.

`schema_plan` returns `candidateModelSource` alongside its existing plan fields. Send the complete plan and `recoveryModelSource` to `database_execute` with `operation: "apply-migration"`. The Kernel checks the old accepted source, derives the plan again and compares its hashes and statements before applying it. A successful apply persists the candidate source and `toHash`. Older callers that omit the candidate can still apply SQL, but the Kernel clears the accepted model rather than retaining a stale revision.

Database backups return `databaseModelSource` and `schemaRevision` when the deployment has an accepted model. Restore returns those fields and restores the persisted model with the database. A backup without model metadata clears the model; any later legacy recovery still has to prove it against SQLite.

## Export and import an app

`package_export` takes a `deploymentId` and captures the app under its project, data and identity locks. No action, upload, signup, policy change or release lands halfway; they wait for the capture to finish. A 256 MiB database takes a few seconds. It returns a manifest and one download transfer per component.

```json
{
  "manifest": {
    "format": "littlebox-package", "version": 1, "packageId": "…", "createdAt": 1762430000000,
    "deploymentId": "…", "projectId": "…", "visibility": "private",
    "signupPolicy": { "mode": "closed", "allowlist": [] },
    "publishedRevision": 4, "artifactVersion": "v…", "sourceAuthority": "managed", "sourcesRevision": 9,
    "contents": [
      { "name": "database", "size": 268435456, "sha256": "…" },
      { "name": "identity", "size": 110592, "sha256": "…" },
      { "name": "files", "size": 20987904, "sha256": "…" },
      { "name": "usage", "size": 61440, "sha256": "…" }
    ],
    "excluded": ["app sessions", "verification, recovery and invitation links", "…"]
  },
  "downloads": { "database": { "transferId": "…", "size": 268435456, "sha256": "…", "chunkBytes": 65536, "expiresAt": 0 } }
}
```

| Component | What it is | Limit |
| --- | --- | --- |
| `database` | The app database, copied with `VACUUM INTO` | 256 MiB |
| `identity` | `identity.sqlite` without sessions or email links, with every API key revoked at import; `secure_delete` overwrites the deleted pages | 64 MiB |
| `files` | A ustar archive of `index.json` and one `<id>.blob` per file, in index order | The destination's per-file limit and file quota, 2 MiB and 20 MiB by default |
| `usage` | `usage.sqlite`, copied with `VACUUM INTO`. Present only when the Deployment recorded Usage or a Signal. A Kernel from before this component refuses a package that lists it | 128 MiB |

`publishedRevision` counts the releases applied to the project. `sourcesRevision` counts saves of Managed sources and is absent for External projects; the two count different things, so a package never claims the saved sources produced the published app. Data panel backups, owner accounts and MCP credentials stay behind.

Read each download with `transfer_read` and cancel it when done. The transfers expire after one hour like other owner transfers. They need `export_data`, not `read`, even though every credential of one owner shares the transfer pool. The audit log gets one `package_export` row and one `package_download` row per component at its first block, with the Deployment as target and the component as detail, plus a row for every refused read.

To import on another Kernel, first create the project there with `project_create` and the same `projectId`, and set every signing secret the app's routes sign with through `secret_set`; secrets never travel, and `release_plan` refuses a release that signs with an unset one. Then start each component upload with `transfer_begin { size, sha256, projectId, component }` and `component` set to its name; `McpClient.upload(bytes, projectId, component)` in `src/mcp/client.ts` does that and writes the blocks. A component upload may be as large as its limit in the table, and the Kernel refuses it at `transfer_begin` when the volume lacks that much space plus 64 MiB. Any other upload stays at 32 MiB. Write and complete it as usual, then send a release payload with `initialPackage`:

```json
{ "project": {}, "visibility": "private", "artifact": {},
  "initialPackage": { "manifest": {}, "components": { "database": "transferId", "identity": "transferId", "files": "transferId" } } }
```

`release_plan` checks, before any Deployment exists:

- the manifest format and version, and that every listed component is within its limit;
- each upload is complete, belongs to the caller and the destination project, was started for that component and matches the manifest's size and SHA-256;
- every SQLite file passes `integrity_check` and the accounts file has `app_users`;
- every file has its data at the indexed size, within the file quotas, and every file owner is an account in the package;
- the volume has room for the components, a schema check copy of the database and 64 MiB to spare;
- the release's `schema.sql` and actions run against the imported database.

`release_apply` copies each component into the new Deployment's folder, checking its SHA-256 as it copies, deletes sessions and email links again, marks any live API key revoked at import, and unpacks the files. The uploads stay intact until you cancel them, so a failed apply can be retried with a new plan. Repeating an applied plan returns its receipt and creates nothing. A `release_plan` that names a package this project already imported returns `{ planId, applied: true, packageId, receipt }` with the first plan, so `release_apply` with that plan returns the first receipt. A project that already has a Deployment refuses any other `initialPackage`, as it refuses the small format's initial state. The signup policy comes from the manifest unless the payload sets `initialSignupPolicy`.

| Refusal | Message |
| --- | --- |
| Export without `export_data`, through any of the three export tools | `Unknown tool or insufficient permission. package_export needs a credential with export_data. Run littlebox login with --export-data, or use a token from token_create that includes it.`, naming the tool called. A credential limited to other projects gets `Project is outside this token's grants.` |
| Reading an export's transfer without `export_data` | `Permission denied for export_data.` |
| Package for a project with a Deployment | `Portable state can only be imported on the first deployment.` |
| Upload bytes that differ from the declared SHA-256 | `Upload size or SHA-256 mismatch.` |
| Upload that differs from the manifest | `The database component does not match the size and SHA-256 in the manifest.` |
| Another manifest version | `Package version 3 is not supported. This Kernel reads versions 1 and 2.` |
| Version 2 without a migration stamp, or a stamp on version 1 | `A version 2 package must name its migration.` / `Only a version 2 package may name a migration.` |
| Component over its limit | `Transfer size is outside the limit of 67108864 bytes for the identity component.` |
| Not enough disk at `transfer_begin` for a component, at `package_export` or at `release_plan` | `Not enough free space on the Kernel volume to import this app: 340 MiB needed, 120 MiB free.` |

The release payload is the one `release_plan` always takes, `project`, `visibility` and the `artifact` that `littlebox build` writes to `.littlebox/artifact.json`, plus `initialPackage`. No CLI command imports a package into a Kernel you already run; [the walkthrough](mcp-walkthrough.md#import-on-a-second-kernel) shows the tool calls.

The CLI's `export --with-data` downloads the package into the kit as `littlebox/package/`, and the kit's `deploy.mjs` uploads it to the kit's own Kernel, signing in with that Kernel's setup key, records its progress in `littlebox/import-state.json` and resumes from the Kernel's offset after an interruption. `deploy.mjs` sets no signing secret, so for an app whose routes sign it stops at `release_plan` with the refusal that names each secret; `deploy.mjs` creates the kit Kernel's owner as `owner@selfhost.local` with the setup key as password, so `LITTLEBOX_EMAIL=owner@selfhost.local LITTLEBOX_PASSWORD=<key> bun run littlebox -- login` gets a credential for `secret_set` there, and the next `deploy.mjs` run goes through. `src/lib/portable-package.ts` checks the manifest on the client with the same limits as `kernel-rs/src/package.rs`.

`project_export`, the small format, and `database_export`, the raw database file, now need `export_data` too. It returns the database, accounts and files as base64 in one transfer of at most 16 MiB, and a release still imports it through `initialDatabase`, `initialIdentity`, `initialFiles` and `initialSignupPolicy`.

## Migrate an app

A migration moves one app to another installation with its accounts, rows, files, Usage and Signals, policy and published artifact, and leaves exactly one installation taking its writes. [ADR-0014](./adr/0014-an-app-migrates-with-one-writer-at-a-time.md) records the decisions, and `tests/smoke/migration.ts` rehearses the whole procedure on two Kernels.

1. Prepare the destination: a Kernel with its own `LITTLEBOX_PUBLIC_ORIGIN`, an owner, an email transport, and the route signing secrets the app needs set with `secret_set` on a project with the same ID.
2. On the origin, `migration_begin { deploymentId, waitSeconds? }`. The app pauses at once, then the Kernel waits up to `waitSeconds` (30 by default, at most 120) for work in progress and captures the package. Download every entry of `downloads` with `transfer_read`.
3. On the destination, upload each component with `transfer_begin` and its component name, add `initialPackage` to the `release` download, and release it with `release_plan` and `release_apply`. The imported app is paused. Check it with read tools.
4. On the origin, `migration_confirm { operationId, newUrl? }`. Keep the `confirmation` from the receipt.
5. On the destination, `migration_activate { deploymentId, confirmation, resendInvitations? }`. The app serves; people sign in with their passwords.

### Maintenance

While an app is paused its host answers a request for a page, or any `GET` outside `/_littlebox/`, with a 503 maintenance page that names the operation, and every other request with HTTP 503, `Retry-After: 300` and:

```json
{ "error": "This app is paused for migration 5d74cf67-…. It accepts no writes, signups, recovery, administrative changes or email until the migration is confirmed or cancelled.", "operationId": "5d74cf67-…", "maintenance": true }
```

That covers actions, uploads and file reads, signup, sign-in, recovery, invitation acceptance, HTTP routes and the runtime MCP endpoint. Nothing is redirected. Owner tools that would change the app are refused with the same message: publishing, the signup policy, suspending and resuming accounts, invitations, secrets, API keys and owner SQL that writes. Tools ending in `_list` or `_read`, `package_export`, `database_export`, `project_export`, `release_plan`, `database_execute` with `inspect`, `browse` or `list-backups`, and the migration tools keep working. The mail worker holds the app's queued email; a held job keeps its attempt count.

The pause is the file `deployments/<id>/migration.json`. A restart keeps it, a scheduled copy carries it, and a restore of that copy stays paused. A file that cannot be parsed pauses the app with a message that names it.

### Tools

`migration_begin` needs `manage_lifecycle` and `export_data`. It writes the pause, takes the project, data and identity locks once each so every write that began before the pause finishes, then waits while an email of the app is with the provider. When the wait ends before that, it returns `phase: "pausing"` with `settle.waitingFor` set to `operations` or `mail`; the app stays paused and the next call continues. Then it ends the app's other queued email (`settle.mailEnded`), captures the package and returns `phase: "captured"` with `operationId`, `packageId`, `manifest` and `downloads`, one per component plus `release`. Calling it again while captured returns the same operation and package with new downloads. A copy does not carry the kept package, so an origin restored while captured captures again under the same operation and confirmation, with a new `packageId`. An error after the pause ends with `The app stays paused: call migration_begin again to retry, or migration_cancel to resume it.`

The manifest is version 2. Its `migration` stamp names the operation, the origin Deployment, project and URL, the size and SHA-256 of the `release` download, and `confirmationHash`, the SHA-256 of a confirmation the origin keeps until `migration_confirm`. A Kernel that reads only version 1 refuses the package. The `release` download is a release payload with the live artifact, `actions.json`, the project manifest and the visibility; it has no `schema.sql`, because the database arrives whole.

`migration_status { deploymentId }` needs `read` and returns `role` (`origin` or `destination`), `phase`, `paused`, the operation, the package and, once confirmed, `confirmedAt`, `retainUntil` and `newUrl`. It never returns the confirmation.

`migration_confirm { operationId, newUrl? }` needs `manage_lifecycle` and a captured operation. The origin stays paused for good, keeps its state and the kept package until `retainUntil`, seven days later, and shows `newUrl` on its maintenance page until then. `newUrl` must be an absolute `https` URL, or `http` on a loopback host, with no credentials, query or fragment, on another host than the app's generated one. Without `newUrl`, an app with a verified custom domain shows the alias, which moves with the package once its DNS points at the destination; the page served on the alias itself shows no link to itself. The receipt holds the `confirmation`. Repeating returns the same receipt.

`migration_cancel { operationId }` needs `manage_lifecycle`. It resumes the origin, deletes the kept package and discards the confirmation, so a package already imported elsewhere can never be activated. Email ended by the pause is not sent again: resend invitations, and people ask again for verification or recovery links. Repeating returns the same receipt.

`migration_activate { deploymentId, confirmation, resendInvitations? }` runs on the destination and needs `manage_lifecycle`, and `manage_users` with `resendInvitations`. It checks the confirmation against the package's hash, ends any session, email link or API key in the imported accounts, and starts serving. The receipt lists pending invitations; with `resendInvitations` each gets a new seven-day link on the destination's host, and `invitations.resent` holds each result. Repeating returns the same receipt.

The audit log records each migration tool with the Deployment as target and the operation as detail.

### What travels

| Carried | Not carried |
| --- | --- |
| Account IDs, password hashes, verified state and suspensions | Sessions, verification, recovery and invitation links |
| Rows, files and their owners, Usage and Signals | Live API keys and route signing secrets |
| API keys, revoked at import, with their names and scopes | |
| Visibility and signup policy | Queued email; pending invitations keep their recipient and are resent |
| The live artifact and `actions.json` | The origin's hostname; the destination builds one from its own origin |
| The custom domain's hostname, pending until `domain_set` finds the destination's token | The custom domain's token; the destination issues its own |

A custom domain moves in this order, so the alias never answers 404: activate the destination, read its new token with `domain_read`, add it to the TXT record next to the origin's value, call `domain_set` on the destination, then point the name's DNS at the destination. The paused import refuses `domain_set` until it is activated.

| Refusal | Message |
| --- | --- |
| `migration_begin` without `export_data` | `Permission denied for export_data.` |
| Any migration tool but `migration_status` without `manage_lifecycle` | `Unknown tool or insufficient permission. migration_begin needs a credential with manage_lifecycle. Run littlebox login with --manage-lifecycle, or use a token from token_create that includes it.`, naming the tool called |
| A write, signup, sign-in, recovery or route call on a paused app | HTTP 503 with the operation, as above |
| An owner change to a paused app | `This app is paused for migration <id>. …` |
| `migration_cancel` after confirmation | `Migration <id> was confirmed, so its destination may already accept writes. Cancelling would leave two installations writing. …` |
| `migration_begin` on a confirmed origin | `Migration <id> was confirmed: this app now takes its writes at the destination. Migrate it from there.` |
| `migration_begin` on an import that is not active | `This app was imported by migration <id> and is not active yet. …` |
| `migration_confirm` after cancel, or before the capture | `Migration <id> was cancelled. Begin a new migration.` / `… has not captured the app yet. …` |
| `migration_activate` with another confirmation, or after the origin cancelled | `This confirmation does not belong to migration <id>. …` |
| `migration_activate` on an app that was not imported | `This app was not imported by a migration, so it has nothing to activate.` |
| `newUrl` over plain `http`, with credentials, a query or fragment, or on the app's own host | `newUrl must use https.` and the like |
| A second package into an app that has one | `Portable state can only be imported on the first deployment.` |
| A package whose custom domain another Deployment on the destination serves | `Package import failed: <name> already serves another Deployment on this installation. …` |

### Restore and the revocation record

A restore brings back an installation from its last copy, up to an hour old. So that it cannot bring back an account suspended or an API key revoked since then, `app_user_suspend`, `api_keys_revoke` and `littlebox-kernel creator-account-suspend` write each change to every copy target, under `revocations/`, before they report success. If a target cannot be written the tool reports `The change holds on this server, but it could not be recorded at the copy target, so a restore could undo it …`; repeating the same request records it. `littlebox-kernel restore` applies the record before it installs the copy. When the record cannot be read, the restored installation starts in maintenance: every app serves its maintenance page and every owner change is refused with `This installation was restored from a copy and stays in maintenance …` until the operator runs `littlebox-kernel reconcile`. The README's section on copies gives the commands.

## Transfers and limits

The client starts a transfer by declaring the size and SHA-256. Chunks are at most 64 KiB and use base64. The server accepts identical retries at the same offset and rejects different content. Completing a transfer verifies size and hash and makes its bytes immutable.

Owner transfers expire after one hour and each owner can hold eight, whichever of their credentials began them. App user transfers expire after 10 minutes and each app user, or each signed-out visitor's client address, can hold two. Owners share a pool of 128 per installation, signed-in app users share a pool of 32 per Deployment, and the signed-out visitors of a public Deployment share a separate pool of 16, so visitors cannot hold the slots signed-in users upload with. An app's files are at most 2 MiB each and the Deployment has a 20 MiB quota, the sum of the files it holds, unless the operator sets `LITTLEBOX_FILE_MAX_BYTES` and `LITTLEBOX_FILES_QUOTA_BYTES`, and at most 10,000 files; a deleted file stops counting. An upload past the per-file limit answers `Files must be between 1 byte and 2 MiB.` and one past the quota `This deployment has reached its 20 MiB file quota.`, each with the configured number. `platform_describe` reports both as `files.maxFileBytes` and `files.quotaBytes`. An upload past the file count answers `This deployment has reached its limit of 10,000 files. Delete files before uploading more.` A file name keeps its first 180 characters after control characters are removed. An administrative transfer can be up to 32 MiB, or up to a package component's limit; a full publishing payload must fit in 26 MB and its artifact in 16 MiB. A project allows up to 100 actions. Queries limit rows, response size and execution time. Each app database is capped at 256 MiB. An action's statement stops after 2 seconds, checked between SQLite steps; a single function call over large values, such as `instr` or `replace` on two 1 MiB strings, is one step and can run past it. The Kernel runs action SQL on a blocking thread under the Deployment's lock, so such a call delays only that Deployment. A LIKE or GLOB pattern is at most 500 bytes, and a longer one answers `LIKE or GLOB pattern too complex`. Applying `schema.sql` caps each value at 2 MiB and the whole schema at 30 seconds, because an expression index or a CHECK runs once per stored row. Studio, migrations and actions cannot create a trigger or a view, and an app database that arrives holding one, through `initialDatabase` or a package, is refused with `The database holds CREATE TRIGGER <name>, and Littlebox runs no triggers or views. Drop it before importing.`

App login and signup accept 10 attempts per minute per client address, and 5 per minute for one email address from one client address, counted separately for each Deployment. Wrong passwords also count against the email address across all clients: after 100 failures in an hour, every login for that email waits, including a correct one. A successful login never uses up that count, and one client address adds at most 20 failures per hour to it, so pausing an account takes failures from at least five addresses. `/_littlebox/owner/login` and `/_littlebox/owner/signup` have the same limits. Recovery requests count against the email they name, and verify and reset requests share one bucket per client address, on each Deployment and on the creator scope; the runtime MCP account tools count in the same buckets. Further attempts get HTTP 429 with `Retry-After`. Each identity database is capped at 64 MiB.

A release's `schema.sql` may only create tables and indexes, and must be safe to run twice. CHECK constraints, expression and partial indexes and generated columns may call SQL functions, except `load_extension`, `readfile`, `writefile`, `edit` and `fts3_tokenizer`, which app actions cannot call either.

`/oauth/register` accepts 10 registrations per client address per hour; IPv6 clients share one bucket per /48. It holds at most 1000 clients that have not reached consent, and at that cap a new registration replaces the oldest of them. Unconsented clients expire after one hour. A client that reached consent expires after 30 days without a new token once it holds no live credential.

The audit log in `mcp.sqlite` records each owner tool call with its project or Deployment. `release_apply` records the project and the Deployment it produced, and `signup_policy_set` records the old and new mode. `secret_set` records the project, never the value. The user and invitation tools record the Deployment; no audit row holds an email or a token. A package download or a package component upload writes one row at its first block and one for each refused block, not one per 64 KiB, so moving a 256 MiB database does not push the rest out of the owner's 10,000 rows of the log.

`transfer_status` lets a client resume an upload from the last offset. Cancel transfers you no longer need. The SDK checks the checksum of downloads and releases its temporary transfers.

## Status and compatibility

The official SDK checks connection, listing and MCP calls, as well as the OAuth flow with registration and PKCE. The browser tests cover publishing, republishing, migrations, restore and export.

The self-hosted export includes the Kernel, its MCP and OAuth modules, and a deploy script that uses authenticated MCP. The platform image includes Bun and the Drizzle tooling. The minimal self-hosted kit supports administrative SQL and migrations supplied by its owner; it does not include the tool that derives Drizzle models.

`project_export`, which needs `export_data`, includes `initialSignupPolicy` with `{ "mode": "open" | "invitation" | "closed" | "allowlist", "allowlist": [...] }` alongside the portable database, accounts and files. It captures `visibility` under the same project lock, so exporters use the snapshot's visibility even if an earlier project inspection or editor copy differs. A release may import that policy only on its first Deployment. The Kernel validates and installs the policy before serving that Deployment, so a public app exported with Closed signup never opens registration during import. Republishing cannot override the policy through initial state. Older exports without these fields use the inspected Deployment's visibility or the editor's current project and the normal signup defaults.

Calls between apps (ADR-0019) change three answers a client may have recorded: `secret_set` and each entry of `secrets_list` carry `"use": "route"` or `"call"`; a `GET` route that answers anyone carries `Access-Control-Allow-Origin: *`; and an action of an unknown kind is refused with `Use "query", "mutation", "signal" or "call".` instead of the list without `call`. `docs/mcp-walkthrough.md` records the first.

Each entry of `secrets_list` also carries `svixKey`, true when the value decodes as a svix `whsec_` key. The value itself is never returned.

The editor preview still stores its test data in the browser, including uploads, which it keeps as data URLs in local storage. It uses MCP over a MessageChannel with an ephemeral preview credential. The registry of published projects and the credentials live in the Kernel.

Remote development environments, teams with Workspace membership and immutable artifact history are outside this release. Publishing keeps the project's URL and database, matching the Kernel's current behavior. Each Deployment is one folder on the Kernel volume with its record, artifact, database, accounts, files and backups. Scheduled copies send those folders to a second disk or an S3 prefix, and `littlebox-kernel restore` installs a copy on a fresh volume; the README's section on the data directory and copies describes both.

The editor's portable `project_export` also returns `domain`, a hostname string or null, under the export lock. It includes no domain verification token. The generated kit stores the hostname as `littlebox/project.json.domain` and sends `initialDomain` with its first release. `initialDomain` needs the release's `publish` permission, is refused on a republish, and cannot accompany `initialPackage`; a package uses `initialPackage.manifest.domain` instead. Both import paths create a pending claim with a new token issued by the destination. After import the generated deploy script calls `domain_read` and prints the TXT record and `domain_set` arguments for verification. Imported artifacts never establish domain ownership. The export still needs `export_data`, and a domain import does not change the snapshot's visibility or signup policy.
