# Accounts and sign-in

Each app decides who may create an account, how its people sign in and prove an email, and where its roles live.

## Signup policy

Each Deployment has a Signup Policy that decides who may create an app account. `signup_policy_set { deploymentId, mode, allowlist? }` needs `manage_users`, and `signup_policy_read` needs `read`. A `deploymentId` is the app's ID, the same value as its `projectId` and the `id` in `konae.json`, because an app keeps one ID for its folder, its project and its Deployment.

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

`KONAE_ACCOUNT_VERIFICATION=required` makes every new account, owner or app user, prove its email before it is active. The default `local` keeps unverified signup for development; [Operate an installation](../operate.md#verified-accounts) covers the switch.

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

`POST /_konae/owner/signup` takes `{ "email", "code" }`, plus `"password"` in local mode, or `"setupKey"` instead of the code for the first owner. One transaction consumes the code and creates the account, which is pending with a verification link in required mode and active with a session in local mode. A code for an email whose account is active or suspended is consumed, creates nothing and gets the same 202. When both `code` and `setupKey` arrive, the code decides. `GET /_konae/owner/config` answers `"admission": "setup-key"` while the key can still create an account and `"code"` otherwise. It also answers `"editor": true` when this server serves the editor from `KONAE_EDITOR_DIR`, so the owner page links to the project list only where one exists, and `"mailToFile": true` under `KONAE_MAIL_TRANSPORT=development`, so the page says a link was written to the mailbox file instead of sent. On a local installation that still runs with the development key, `"defaultSetupKey"` holds that key so the page can fill it in; everywhere else it is `null`.

| Request | Status | Message |
| --- | --- | --- |
| A used, expired, revoked, unknown or mistyped code | 403 | `This invitation code is not valid. It may be mistyped, expired, revoked or already used.` |
| The setup key once a code or another account exists | 403 | `An invitation code is required to create an owner account. Ask the operator of this installation for one.` |
| A wrong setup key while it can still create the first owner | 403 | `A valid installation setup key is required to create the first owner.` |

An MCP client that starts OAuth while the owner is signed out sends them to `/_konae/owner/login?returnTo=/oauth/authorize?...`. Konae keeps `returnTo` only when it starts with `/oauth/authorize?` and fits in 4096 bytes, and puts it in the verification link, so the person lands back on the consent screen after choosing a password.

## Account email

Account links and every [notice](./usage-and-signals.md#notices) leave through the queue in `mail.sqlite`. [Operate an installation](../operate.md#account-email) lists the transports, their variables and the limits. A refused request stores nothing. 503 means no transport, or a sender the preflight refused, and 429 means a limit.

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

## Read next

- [Invite people](../guides/invite-people.md)
- [Add a second owner](../guides/add-a-second-owner.md)
- [Accounts and access](../accounts-and-access.md)
- [Operate an installation](../operate.md#verified-accounts)
