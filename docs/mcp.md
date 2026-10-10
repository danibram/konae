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

Call `platform_describe` and `projects_list` after connecting. `platform_guide` takes the topics `credentials`, `development`, `actions`, `sdk`, `publishing`, `users` and `portability`, each a short form of a section of this page. [The walkthrough](mcp-walkthrough.md) runs every step from a credential to a migration against a Kernel, with the exact answers. An agent needs filesystem and local execution tools to create files and build an app. The MCP server does not read paths on the user's machine.

If MCP is published on an origin other than the hosting origin, set `LITTLEBOX_MCP_ORIGIN` to its public origin, without a path. The proxy must preserve `Host`. Owner signup answers on that host too, so the Kernel refuses to start with the default setup key when this host is not local. Browser calls to the editor and to the runtime use their own origin; there is no open CORS.

## Tools

| Group | Tools |
| --- | --- |
| Discovery | `platform_describe`, `platform_guide`, `session_inspect`, `mail_status`, `projects_list`, `project_inspect` |
| Projects | `project_create`, `project_sources_read`, `project_sources_save` |
| Eject, with `manage_lifecycle` | `eject_plan`, `eject_apply` |
| Migration, with `manage_lifecycle` | `migration_begin`, `migration_confirm`, `migration_cancel`, `migration_activate`; `migration_status` with `read` |
| Owner schema and data | `schema_plan`, `database_execute`, `storage_test` |
| App export, with `export_data` | `package_export`, `project_export`, `database_export` |
| Publishing | `release_plan`, `release_apply` |
| Signup policy | `signup_policy_read`, `signup_policy_set` |
| Signing secrets | `secrets_list`, `secret_set`, `secret_delete` |
| API keys | `api_keys_list`, `api_keys_create`, `api_keys_revoke` |
| Usage and Signals | `usage_read`, `signals_read` |
| App users and invitations | `app_users_list`, `app_user_suspend`, `app_user_resume`, `app_invitations_create`, `app_invitations_list`, `app_invitation_resend`, `app_invitation_revoke` |
| Transfers | `transfer_begin`, `transfer_status`, `transfer_write`, `transfer_complete`, `transfer_read`, `transfer_cancel` |
| Credentials from the owner session | `tokens_list`, `token_create`, `token_revoke` |
| App runtime | `auth_session`, `auth_logout`, `actions_invoke`, `files_list`, `files_upload`, `files_read`, `files_update`, `files_delete` and transfers |
| App accounts, also before sign-in | `auth_signup`, `auth_verify`, `auth_recover`, `auth_reset` |

The catalog depends on the credentials. The Kernel checks the permission again on execution. A call to a tool the credential does not list answers `Unknown tool or insufficient permission.` and, for a tool another credential would list, the permission it needs and how to get it, such as `app_invitations_create needs a credential with manage_users. Run littlebox login with --manage-users, or use a token from token_create that includes it.` A tool of the runtime endpoint called here says so. A token limited to projects cannot create other projects. For `schema_plan`, that token must pass a `projectId` it has access to. Reading the signup policy needs the `read` scope. Changing it, and every `app_users_*` and `app_invitation*` tool, needs `manage_users`. The owner's browser session holds it. A delegated token holds it only when it was created with it, so tokens issued before this permission existed do not gain it. `read` alone never lists accounts or invitations.

This changes the contract. `signup_policy_set` needed `publish` before `manage_users` existed, so a client that changed the policy with a `read`, `write` and `publish` token now gets "Unknown tool" and needs a new token that includes `manage_users`. `bun run littlebox -- login --manage-users` asks for one, and `bun run littlebox -- dev` asks for one on its Development Kernel.

Listing signing secrets and API keys needs the `read` scope; setting, creating and revoking them needs `publish`. `usage_read` and `signals_read` need `read`.

`project_create` returns a template and an External identity by default. Later sources stay in the user's folder; `project_inspect` returns metadata and the Deployment, without claiming to synchronize that code with the editor.

## Signup policy

Each Deployment has a signup policy that decides who may create an app account. `signup_policy_set` takes its `mode`:

- `open`: anyone can create an account.
- `invitation`: nobody can sign up; an invited person creates the account from the invitation link.
- `closed`: nobody can sign up; a person the owner invites can still accept the invitation. Pending invitations stay acceptable when the owner closes signup, so revoke the ones that should end.
- `allowlist`: only exact emails or `@domain` entries can create an account, up to 100 entries. A domain entry matches that exact domain, not its subdomains. Entering an address sends no invitation.

Public Deployments start Open. Private Deployments start Closed, so the owner decides who may join before anyone can sign in. A Deployment record written before signup policies existed gets the same default from its visibility. A first release may set any mode in `initialSignupPolicy`, with the `publish` permission, and the Kernel installs it before serving the first request; an agent publishing an app whose access the request leaves open proposes private and `invitation`. Republishing keeps the current policy, except that republishing a public app as private turns Open into Closed; `initialSignupPolicy` on a republish answers "Portable state can only be imported on the first deployment." Changing an existing app's policy needs `manage_users`, through `signup_policy_set`, so no publish credential changes who may join. The owner changes it in the editor's Auth > Users view or with the admin MCP tools above. The editor's publish dialog sends `initialSignupPolicy` on a first publish, proposing Invitation only for a private app, and on a republish shows the current policy read-only.

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

Every account link leaves through the queue in `mail.sqlite` (`kernel-rs/src/mail.rs`). The README's [Account email](../README.md#account-email) section lists the transports and their variables. What a client sees:

- Signup and recovery store the challenge and its email job in one transaction and answer at once. The worker sends the job on its own task.
- A job is `pending`, `accepted` (the provider took it), `delivered` (the provider reported delivery to the recipient's server) or `failed`. No job state activates an account; only the person answering the link does.
- A refusal stores nothing. 503 means no transport, or a sender the preflight refused. 429 means a limit: per address 3 a minute across the installation and 10 a day in each scope, so requests on one app cannot use up an address's allowance on another app or in the editor; per client address 20 an hour, where one IPv6 /64 is one client; per creator 100 a day, per app 100 a day, per installation 1,000 a day. The address and client limits count requests, so an address with no account reaches them after the same number of requests as one with an account.

`mail_status` is an admin tool for any owner credential. It returns the transport, the sender, `sending`, the preflight with its state (`checking`, `ready` or `refused`), reason and time, the queue depth and the count per state, the last failure as `{ kind, at, purpose }`, the limits, and whether provider events are configured. `refresh: true` runs the preflight again. It names no recipient. Without a transport it returns `sending: false` and how to configure one.

`/_littlebox/health` carries a `mail` field when a transport is set: `transport`, `sending`, `preflight`, `depth` and `lastFailure` with its kind and time only.

A failure kind is one of `provider-unavailable` (retrying, or out of retries), `rejected` (the provider refused that message), `sender-refused` (the provider refused the sender; sending stops until a preflight passes), `superseded` (a newer link replaced it, or its challenge ended), `link-expired`, `secret-unavailable` (the sealing secret changed), `bounced`, `provider-failed`, `suppressed` and `restored` (a restore ended it).

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

The uploader, and nobody else, changes a file afterwards. `files_update` with `{ fileId, visibility }` sets `private` or `public` and returns the file without a `url`; the ID and its path stay the same, so a published draft is the same file and counts once against the quota. `files_delete` with `{ fileId }` removes the file's entry and bytes and returns `{ id, deleted: true }`; its size stops counting against the 20 MiB quota, and from then on `files_read`, `files_list` and the file path answer as for a file that never existed. Both need a session. A user who may not read the file gets `File not found.`, the same as for a missing one, and a user who reads it but did not upload it gets `Only the person who uploaded this file may change or delete it.` A file stored before files had owners has no uploader, so the app cannot change or delete it. A row that still names a deleted file ID keeps it; the app removes that row with its own action. Neither tool counts in Usage. Each visitor's downloads use a transfer bound to that client address, so anonymous callers do not share one transfer bucket.

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
| A kind other than `query`, `mutation` or `signal` | `Action <name> has an invalid kind. Use "query", "mutation" or "signal".` |
| `"nullable": true` on a binding that is not an input | `Action <name> has an invalid nullable input binding.` |
| A binding of another source, an input with no key or another type, an actor field other than `id` or `role`, a kernel field other than `now`, a literal that is not a string, a number or null | `Invalid binding <index> in <name>.` |
| A string input's `maxLength` outside 1 to 100,000 | `Invalid maxLength in <name>.` |
| A statement longer than 20,000 bytes, or more than 100 bindings | `Action <name> exceeds statement or binding limits.` |

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

The route answers to the callers it takes, described below, and on a private app an unsigned route takes an API key or a session, never a caller with neither. The action keeps its bindings, its query bound and the runtime endpoint's rate limits. A call that succeeds counts in [Usage](#usage-and-signals) under the route's method and path, and records the action's Signal if it declares one. A `GET` maps the query string onto the input bindings by their declared type, so `?min=10` binds the number 10; a `POST` reads a JSON object of at most 64 KiB. Inputs the action does not declare are refused, as with `miniapp.actions.call`, so a sender whose payload carries other fields needs an action that declares them or an adapter.

| Answer | Status | When |
| --- | --- | --- |
| `{ "result": [rows] }` or `{ "result": { "changes", "lastInsertId" } }` | 200 | The action ran. Every answer carries `Cache-Control: no-store` |
| `{ "error": "<binding message>" }` | 400 | An input is missing, unexpected or of the wrong type, the body is not a JSON object, or a query parameter repeats |
| `{ "error": "invalid-signature" }` | 401 | A route that needs the signature from this caller got none, a malformed one, or one whose MAC does not match |
| `{ "error": "stale-timestamp" }` | 401 | The signature's `t` is more than 300 seconds from the Kernel's clock. Checked before the MAC, so it says nothing about the secret |
| `{ "error": "authentication-required" }` | 401 | A route that takes an API key or a session, and no signature, got neither |
| `{ "error": "invalid-api-key" }` | 401 | The bearer token starts with `lbk_` and names no live key of this Deployment |
| `{ "error": "This route does not accept API keys." }` | 403 | The route's `callers` leave out `key` and the request carries a live key |
| `{ "error": "This API key may not call <action>." }` | 403 | The key does not name the action |
| `{ "error": "No route accepts POST on this path." }` | 405 | No action binds the path for `POST`. `Allow` is `GET` on a path a query binds, `GET, HEAD` elsewhere |
| `{ "error": "A route does not answer HEAD. Send GET." }` | 405 | A `HEAD` reaches a path a query binds. It is refused before the caller is resolved and before any SQL, with `Allow: GET`. `HEAD` on every other path serves the Build Artifact's headers as before |
| `{ "error": "replayed-signature" }` | 409 | The same signature arrived before, on this path, in the last 600 seconds, before or after a Kernel restart. A sender signs a retry again, with a new nonce |
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

The owner sets the secret for the project before publishing. A release whose signature names an unset secret is refused when planned and again when applied, with `Action <name> signs with the secret <secret>, which is not set for this project. Set it with secret_set before publishing.` When several are unset, one refusal names every action and every secret: `Actions events.feed and events.receive sign with the secrets events_feed and events_webhook, which are not set for this project. Set each with secret_set before publishing.` The editor cannot see secrets, so this is the one refusal the Validation Gate does not make.

```text
secret_set   { "projectId", "name": "orders_webhook", "value": "<16 to 512 printable characters, no spaces>" }
secrets_list { "projectId" }                      names and update times, never values
secret_delete { "projectId", "name" }             refused while the published Deployment signs with it
```

A signature names no app or host, so a request signed for one project would be accepted by another that holds the same value. `secret_set` therefore refuses a value that another project of the same owner already holds, with `Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.` A copy of an app for staging gets its own secret, and its sender signs with that one.

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

Services with their own fixed signing scheme do not speak this one and need an adapter. `littlebox-kernel restore` brings back the table as the copy held it, so a signature accepted after that copy is accepted once more if it is replayed while its `t` is still within 300 seconds of the Kernel's clock.

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

## Usage and Signals

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
| `scheduled` | the action | the Kernel's timer ran a [scheduled](#scheduled-work) mutation, whether it succeeded or failed |

Nothing else the Kernel refuses is counted: a call without the session it needs, an input the bindings reject, a statement SQLite refuses, a bad signature or a wrong password. A guard's refusal is the app's own rule saying no, which the owner wants to see next to the calls that ran. The Kernel writes Usage off the request path. A response never waits for it, and when more than 16,384 entries are waiting the newest are dropped and logged. The two read tools wait up to five seconds for the queue to drain, so they include every call that finished before them.

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
| A signal action with `http` | `Action <name> must bind GET for a query and POST for a mutation.` |
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

Some work has to happen when nobody asks: a hold that lapsed is released so the hour shows free, a day is closed so tomorrow's page starts from a total. A mutation that takes no input may declare when the Kernel's own timer runs it ([ADR-0015](./adr/0015-the-kernel-runs-a-declared-schedule.md)):

```json
"holds.release": {
  "kind": "mutation",
  "access": "authenticated",
  "schedule": { "every": "15m" },
  "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL AND until < ?1",
  "bindings": [{ "source": "kernel", "field": "now" }]
}
```

`"every"` takes a whole number of minutes or hours, `"5m"` to `"24h"`, and periods count from 00:00 UTC, so `"15m"` runs at :00, :15, :30 and :45. `"daily": "HH:MM"` runs once a day at that UTC time. The Kernel looks once a minute and runs an action at the first look after its period opens, in the Kernel's one process. Nobody calls it, so `kernel.now` is the time of the run, literals bind as written and an `actor` binding, of the `id` or of the `role`, is null. A person's call always binds an ID, so a `WHERE` that tests the actor binding with `IS NULL` holds only for the timer. The action still answers callers as its `access` says.

| Rule | What the Kernel does |
| --- | --- |
| Once a period | The period it last ran for is kept in the Deployment's `usage.sqlite`. A restart, a copy, a package and a migration carry it, so a daily job does not run twice in a day. A Kernel that was down for several periods runs the action once when it is back |
| Publishing | The first period an action is seen in counts as done, so publishing never runs it. A republish that changes the schedule starts again from the next period |
| One run at a time | A run takes the Deployment's data lock and has the 2-second statement budget. A Deployment's scheduled actions run one after another in name order, and its next look waits until they finish |
| Maintenance | A Deployment in maintenance runs nothing, and runs each due action once when it leaves maintenance |
| Failure | A run SQLite refuses is logged, recorded with `outcome: "failed"` and its error, and tried again at the next look. Runs never pile up |
| Guard and Signal | A run that changes no row had nothing to do: it is recorded with `changes: 0` and the guard does not apply. A run that changed a row records the action's Signal, with `actor` null |
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

The content type follows the extension: HTML, CSS, JavaScript, JSON and source maps, `webmanifest`, text, Markdown, CSV, XML (`application/xml`), RSS, Atom, SVG, PNG, JPEG, GIF, WebP, AVIF, ICO, PDF, MP4, WebM, MP3, WASM, and WOFF2, WOFF, TTF and OTF fonts. Any other file is `application/octet-stream`, and every file is sent with `X-Content-Type-Options: nosniff`. A `HEAD` on an artifact file answers its headers with no body. Every HTML page, the second page and `404.html` included, gets the Content Security Policy with the hashes of its own inline scripts, and `/runtime.js` loads under `script-src 'self'`.

The 16 MiB artifact limit counts the bytes the Kernel stores, after base64 is decoded. The Kernel refuses an encoding other than base64, an encoding for a file the artifact does not hold, and a file marked base64 that does not decode, each with the path in the message.

There is also `bun run littlebox -- login --dir /tmp/my-app --url http://localhost:4173`. It reads `LITTLEBOX_EMAIL` and `LITTLEBOX_PASSWORD` from the environment, obtains a 24-hour credential with `read`, `write` and `publish` and stores it with `0600` permissions in `.littlebox/credentials.json`, which `init`, `publish`, `export` and `eject` read when `LITTLEBOX_MCP_TOKEN` is unset. `--manage-users`, `--export-data` and `--manage-lifecycle` add those permissions; a delegated token cannot add one to itself later, so ask for every permission the work needs. `LITTLEBOX_INVITATION_CODE` lets it create the account if it does not exist yet, and `LITTLEBOX_SETUP_KEY` does so for an installation's first owner. On an installation that requires verification, creating the account sends a link, and `login` stops with the two steps: open the link, choose `LITTLEBOX_PASSWORD` as the password, then run `login` again without the code or key, which are used by then. On a development Kernel on the same machine, `--verify-from <data folder>/mailbox.jsonl` does both in one run: after the signup it waits up to 15 seconds for the newest creator verification line for that email in the development mailbox, answers it with `LITTLEBOX_PASSWORD` and goes on to save the credential. The flag refuses any Kernel URL that is not `localhost`, a `.localhost` name or a loopback address, because the mailbox is a file on the Kernel's machine and only the development transport writes it. The command adds `.littlebox/` to `.gitignore`; do not copy credentials into the frontend.

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
| `files` | A ustar archive of `index.json` and one `<id>.blob` per file, in index order | 2 MiB per file, 20 MiB of file data |
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

`migration_confirm { operationId, newUrl? }` needs `manage_lifecycle` and a captured operation. The origin stays paused for good, keeps its state and the kept package until `retainUntil`, seven days later, and shows `newUrl` on its maintenance page until then. `newUrl` must be an absolute `https` URL, or `http` on a loopback host, with no credentials, query or fragment, on another host than the app's. The receipt holds the `confirmation`. Repeating returns the same receipt.

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

### Restore and the revocation record

A restore brings back an installation from its last copy, up to an hour old. So that it cannot bring back an account suspended or an API key revoked since then, `app_user_suspend`, `api_keys_revoke` and `littlebox-kernel creator-account-suspend` write each change to every copy target, under `revocations/`, before they report success. If a target cannot be written the tool reports `The change holds on this server, but it could not be recorded at the copy target, so a restore could undo it …`; repeating the same request records it. `littlebox-kernel restore` applies the record before it installs the copy. When the record cannot be read, the restored installation starts in maintenance: every app serves its maintenance page and every owner change is refused with `This installation was restored from a copy and stays in maintenance …` until the operator runs `littlebox-kernel reconcile`. The README's section on copies gives the commands.

## Transfers and limits

The client starts a transfer by declaring the size and SHA-256. Chunks are at most 64 KiB and use base64. The server accepts identical retries at the same offset and rejects different content. Completing a transfer verifies size and hash and makes its bytes immutable.

Owner transfers expire after one hour and each owner can hold eight, whichever of their credentials began them. App user transfers expire after 10 minutes and each app user, or each signed-out visitor's client address, can hold two. Owners share a pool of 128 per installation, signed-in app users share a pool of 32 per Deployment, and the signed-out visitors of a public Deployment share a separate pool of 16, so visitors cannot hold the slots signed-in users upload with. An app's files are at most 2 MiB each and the Deployment has a 20 MiB quota, the sum of the files it holds, and at most 10,000 files; a deleted file stops counting. An upload past the file count answers `This deployment has reached its limit of 10,000 files. Delete files before uploading more.` A file name keeps its first 180 characters after control characters are removed. An administrative transfer can be up to 32 MiB, or up to a package component's limit; a full publishing payload must fit in 26 MB and its artifact in 16 MiB. A project allows up to 100 actions. Queries limit rows, response size and execution time. Each app database is capped at 256 MiB. An action's statement stops after 2 seconds, checked between SQLite steps; a single function call over large values, such as `instr` or `replace` on two 1 MiB strings, is one step and can run past it. The Kernel runs action SQL on a blocking thread under the Deployment's lock, so such a call delays only that Deployment. A LIKE or GLOB pattern is at most 500 bytes, and a longer one answers `LIKE or GLOB pattern too complex`. Applying `schema.sql` caps each value at 2 MiB and the whole schema at 30 seconds, because an expression index or a CHECK runs once per stored row. Studio, migrations and actions cannot create a trigger or a view, and an app database that arrives holding one, through `initialDatabase` or a package, is refused with `The database holds CREATE TRIGGER <name>, and Littlebox runs no triggers or views. Drop it before importing.`

App login and signup accept 10 attempts per minute per client address, and 5 per minute for one email address from one client address, counted separately for each Deployment. Wrong passwords also count against the email address across all clients: after 100 failures in an hour, every login for that email waits, including a correct one. A successful login never uses up that count, and one client address adds at most 20 failures per hour to it, so pausing an account takes failures from at least five addresses. `/_littlebox/owner/login` and `/_littlebox/owner/signup` have the same limits. Recovery requests count against the email they name, and verify and reset requests share one bucket per client address, on each Deployment and on the creator scope; the runtime MCP account tools count in the same buckets. Further attempts get HTTP 429 with `Retry-After`. Each identity database is capped at 64 MiB.

A release's `schema.sql` may only create tables and indexes, and must be safe to run twice. CHECK constraints, expression and partial indexes and generated columns may call SQL functions, except `load_extension`, `readfile`, `writefile`, `edit` and `fts3_tokenizer`, which app actions cannot call either.

`/oauth/register` accepts 10 registrations per client address per hour; IPv6 clients share one bucket per /48. It holds at most 1000 clients that have not reached consent, and at that cap a new registration replaces the oldest of them. Unconsented clients expire after one hour. A client that reached consent expires after 30 days without a new token once it holds no live credential.

The audit log in `mcp.sqlite` records each owner tool call with its project or Deployment. `release_apply` records the project and the Deployment it produced, and `signup_policy_set` records the old and new mode. `secret_set` records the project, never the value. The user and invitation tools record the Deployment; no audit row holds an email or a token. A package download or a package component upload writes one row at its first block and one for each refused block, not one per 64 KiB, so moving a 256 MiB database does not push the rest out of the owner's 10,000 rows of the log.

`transfer_status` lets a client resume an upload from the last offset. Cancel transfers you no longer need. The SDK checks the checksum of downloads and releases its temporary transfers.

## Status and compatibility

The official SDK checks connection, listing and MCP calls, as well as the OAuth flow with registration and PKCE. The browser tests cover publishing, republishing, migrations, restore and export.

The self-hosted export includes the Kernel, its MCP and OAuth modules, and a deploy script that uses authenticated MCP. The platform image includes Bun and the Drizzle tooling. The minimal self-hosted kit supports administrative SQL and migrations supplied by its owner; it does not include the tool that derives Drizzle models.

`project_export`, which needs `export_data`, includes `initialSignupPolicy` with `{ "mode": "open" | "invitation" | "closed" | "allowlist", "allowlist": [...] }` alongside the portable database, accounts and files. It captures `visibility` under the same project lock, so exporters use the snapshot's visibility even if an earlier project inspection or editor copy differs. A release may import that policy only on its first Deployment. The Kernel validates and installs the policy before serving that Deployment, so a public app exported with Closed signup never opens registration during import. Republishing cannot override the policy through initial state. Older exports without these fields use the inspected Deployment's visibility or the editor's current project and the normal signup defaults.

The editor preview still stores its test data in the browser, including uploads, which it keeps as data URLs in local storage. It uses MCP over a MessageChannel with an ephemeral preview credential. The registry of published projects and the credentials live in the Kernel.

Remote development environments, teams with Workspace membership and immutable artifact history are outside this release. Publishing keeps the project's URL and database, matching the Kernel's current behavior. Each Deployment is one folder on the Kernel volume with its record, artifact, database, accounts, files and backups. Scheduled copies send those folders to a second disk or an S3 prefix, and `littlebox-kernel restore` installs a copy on a fresh volume; the README's section on the data directory and copies describes both.
