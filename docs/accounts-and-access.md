# Accounts and access

Three questions stay separate. Who edits the app, who may open it, and who may read a row. Each has its own mechanism, and the host decides none of the last two.

## Who is who

| Role | What it is |
| --- | --- |
| Project Owner | An authenticated workspace member who edits a Project and administers its data, schema, files, Usage and deployments. |
| Runtime Actor | Whoever calls a Deployment at runtime. It is an anonymous visitor, a signed-in app user, or a service holding an API key. |
| App account | The account a Runtime Actor of kind app user signs in with on one Deployment. |

A Runtime Actor's permissions are evaluated by server-side actions and never imply Project ownership. Do not build a users table of your own that bypasses the Runtime Actor. Use the Signup Policy and invitations.

## Who may read a row

The app decides, in the statement and its bindings. A Server Action binds the caller with `{ "source": "actor", "field": "id" }` or `"role"`.

- `id` is the app user's ID or the API key's ID. It is nothing for a visitor and for a schedule's run.
- `role` is `owner` for the Project Owner's own app account and `member` for every other app user. A visitor, a key, a signed request and a schedule's run have no role, and the binding yields null.

```json
"items.list": {
  "kind": "query",
  "access": "authenticated",
  "statement": "SELECT id,title FROM items WHERE user_id=?1 LIMIT 50",
  "bindings": [{ "source": "actor", "field": "id" }]
}
```

Public visibility of a Deployment grants no database access. A signed-out visitor may call only Server Actions declared `anonymous`, and may list and read files marked public. Guard owner-only actions with a check on the `role` binding, for example `?n = 'owner'`, instead of a key-only route.

## Public and private Deployments

- A Public Deployment opens its shell without signing in.
- A Private Deployment requires a signed-in Runtime Actor before anything loads or runs.

When a request leaves access open for an app with accounts, the guide's advice to an agent is to propose a private Deployment with the `invitation` signup mode.

## Signup Policy

The Signup Policy decides who may create an app account on one Deployment.

| Mode | Who may create an account |
| --- | --- |
| `open` | Anyone. |
| `invitation` | Nobody signs up. An invited person creates the account from the invitation link. |
| `closed` | Nobody signs up. A person the owner invites can still accept the invitation. |
| `allowlist` | Only exact emails or `@domain` entries, up to 100. A domain entry matches that exact domain, not its subdomains. Entering an address sends no invitation. |

Public Deployments start Open. Private Deployments start Closed. A first release may name another mode. After that, a republish never changes it, except that republishing a public app as private turns Open into Closed.

The Project Owner changes it in the editor's Auth > Users view, or with `signup_policy_set`, which needs the `manage_users` permission. A project published with the CLI names its first release's policy in `konae.json` as `project.manifest.signupPolicy`.

In local verification mode, Konae does not verify email addresses. Whoever signs up first with a listed address gets that account. Use Closed for apps that hold private data, or require verification.

## Sign in with Google

Every app account can sign in with email and password. The owner can add Google for one app, with that app's own Google OAuth client. The installation holds no Google client.

1. Make a Web application client in your Google Cloud console.
2. Set its two halves as project secrets, `google_client_id` and `google_client_secret`, with `secret_set` and `use: "google"`, or in the editor's Auth > Users view.
3. Register the redirect URIs that `sign_in_methods_read` lists in that client.
4. Turn Google on with `sign_in_methods_set` and `["password", "google"]`, which needs `manage_users`.

A Google sign-in reaches the app account with the same email. For an email with no account, the Signup Policy decides as it does for a password. Password stays on, so every account keeps a way back in. A package carries the setting and the accounts but never the client, so on another installation the owner sets the two secrets again. [The MCP guide](./mcp.md#sign-in-methods) states every rule and refusal.

## Invitations

An invitation offers one email an account on one Deployment. The link lasts seven days. Accepting it sets the password and activates the account, and the Kernel rechecks the Signup Policy and the account's state in the same transaction.

- `app_invitations_create` sends the email. Call it only when the owner asked for those invitations.
- Check `mail_status` first. Inviting needs `sending: true`.
- `app_invitations_list` shows each status. `app_invitation_resend` sends a new link. `app_invitation_revoke` ends one.

An invitation never changes an active account's password or resumes a suspended one. A package, a portable export and a restore keep the invitation as history without its link, and the owner resends it.

## Suspending and resuming

An account is pending until it proves control of its email, active, or suspended. Only an active account gets a session. `app_user_suspend` ends sessions and keeps the account's ID and rows. `app_user_resume` returns it to `active` if it ever verified, otherwise `pending`. `app_users_list` shows each account's state and role.

## The owner's account in an app

The Project Owner's own app account has the role `owner`. Only the owner's browser session, through `app_owner_enter`, or an invitation the owner sends to their own email creates it. `app_owner_enter` returns a link that signs the account in once, within two minutes.

## Credentials for agents

An Agent Credential grants no Runtime Actor identity. Its permissions are `read`, `write`, `publish`, `manage_users`, `export_data` and `manage_lifecycle`. Only `manage_users` lists, suspends and invites app accounts or changes a Signup Policy. See [MCP tools and guide topics](./mcp-tools.md).

## Read next

- [Invite people](./guides/invite-people.md)
- [Server Actions](./server-actions.md)
- [MCP tools and guide topics](./mcp-tools.md)
- [Concepts](./concepts.md)
