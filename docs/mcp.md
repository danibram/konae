# Develop with authenticated MCP

Littlebox exposes its operations through MCP over Streamable HTTP. The editor, the app SDK and external agents use that protocol. The supported revision is `2025-11-25`.

## Startup and owner account

```sh
bun install
bun run dev:local
```

Open the editor and go to **Platform access**. The direct link is `http://localhost:4173/_littlebox/owner/login`. To create the first owner, enter an email, a password and the installation key. In the default local environment that key is `local-development-key`; `LITTLEBOX_ADMIN_KEY` sets it. The first account claims the Deployments from the earlier prototype. Every later creator enters an invitation code instead, described under [creator admission](#creator-admission).

Each owner administers their own projects. The page creates read-only or read, edit and publish tokens, optionally limited to specific projects, and lists and revokes active credentials. A token's secret appears only once. Tokens from this page expire after 24 hours; `token_create` accepts any subset of `read`, `write` and `publish` and a lifetime of up to 30 days.

The installation key does not authenticate MCP tools and is not included in the editor bundle. The server listens on `127.0.0.1` by default. The Kernel refuses to start on a non-loopback address, or with a `LITTLEBOX_PUBLIC_ORIGIN` whose host is not `localhost`, a `*.localhost` name or a loopback IP, while the installation key is unset or still `local-development-key`, unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`. For a remote installation, configure the installation key, the HTTPS origin and the proxy before listening on another interface.

## Connect an agent

Configure the endpoint `http://localhost:4173/mcp` in your MCP client. Clients with OAuth can discover the server and open the consent screen in the browser. The flow uses PKCE S256 and single-use codes. OAuth tokens last one hour; when one expires, the client must authorize again. The server issues no refresh tokens.

Clients that support headers can also use a delegated token.

```http
Authorization: Bearer YOUR_TOKEN
```

Call `platform_describe` and `projects_list` after connecting. An agent needs filesystem and local execution tools to create files and build an app. The MCP server does not read paths on the user's machine.

If MCP is published on an origin other than the hosting origin, set `LITTLEBOX_MCP_ORIGIN` to its public origin, without a path. The proxy must preserve `Host`. Owner signup answers on that host too, so the Kernel refuses to start with the default setup key when this host is not local. Browser calls to the editor and to the runtime use their own origin; there is no open CORS.

## Tools

| Group | Tools |
| --- | --- |
| Discovery | `platform_describe`, `platform_guide`, `session_inspect`, `projects_list`, `project_inspect` |
| Projects | `project_create` |
| Owner schema and data | `schema_plan`, `database_execute`, `database_export`, `project_export`, `storage_test` |
| Publishing | `release_plan`, `release_apply` |
| Signup policy | `signup_policy_read`, `signup_policy_set` |
| Transfers | `transfer_begin`, `transfer_status`, `transfer_write`, `transfer_complete`, `transfer_read`, `transfer_cancel` |
| Credentials from the owner session | `tokens_list`, `token_create`, `token_revoke` |
| App runtime | `auth_session`, `auth_logout`, `actions_invoke`, `files_list`, `files_upload`, `files_read` and transfers |
| App accounts, also before sign-in | `auth_signup`, `auth_verify`, `auth_recover`, `auth_reset` |

The catalog depends on the credentials. The Kernel checks the permission again on execution. A token limited to projects cannot create other projects. For `schema_plan`, that token must pass a `projectId` it has access to. Reading the signup policy needs the `read` scope; changing it needs the `publish` scope.

`project_create` returns a template and an External identity by default. Later sources stay in the user's folder; `project_inspect` returns metadata and the Deployment, without claiming to synchronize that code with the editor.

## Signup policy

Each Deployment has a signup policy that decides who may create an app account:

- Open: anyone can create an account.
- Allowlist: only exact emails or `@domain` entries can create an account, up to 100 entries. A domain entry matches that exact domain, not its subdomains.
- Closed: no new accounts.

Public Deployments start Open. Private Deployments start Closed, so the owner decides who may join before anyone can sign in. A Deployment record written before signup policies existed gets the same default from its visibility. Republishing keeps the current policy, except that republishing a public app as private turns Open into Closed. The owner changes it in the editor's Auth panel or with the admin MCP tools above. A tighter policy blocks new accounts only; existing accounts can still sign in. A refused signup gets the same answer whether signup is closed or the address is not listed.

In local mode Littlebox does not verify email addresses, so whoever signs up first with a listed address or an address in a listed domain gets that account. Use Closed for apps that hold private data, or require verification.

## Verified accounts

`LITTLEBOX_ACCOUNT_VERIFICATION=required` makes every new account, creator or app user, prove control of its email before it is active. The default `local` keeps unverified signup for development; the README explains both modes, the switch for existing accounts and the operator's suspension command.

An account is `pending`, `active` or `suspended`. Only `active` accounts get a session, and every runtime and administrative check reads the state: the app session, the owner session, a delegated MCP credential and the OAuth code exchange. Suspending an account deletes its sessions and open links and keeps its ID, password and rows.

A challenge has a purpose, `verification`, `recovery` or `invitation`, and a scope, `creator` or one Deployment. The Kernel stores only its SHA-256 hash. A token works once, expires (verification after 24 hours, recovery after 30 minutes), and does nothing for another purpose, on another Deployment or on the creator scope. A new link for the same purpose replaces the earlier one. Invitations reuse this model in a later delivery; no route accepts an invitation token yet.

In required mode the signup request carries only an email. The password is chosen on the page the link opens, so a password typed by whoever registered the address first never survives verification. Signing up again with a pending email keeps the account ID and sends a fresh link. Links put the token in the URL fragment, `#verification=` or `#recovery=`, so it reaches neither server logs nor a Referer. Opening a link with GET returns the form and changes nothing; the form posts the token with the new password.

| Route | Host | Body | Result |
| --- | --- | --- | --- |
| `POST /_littlebox/auth/signup` | app | `{ "email" }` | 202 `{ "result": { "status": "link-sent" } }`, the same for new, pending, active and suspended emails. A signup the policy refuses gets 403 as before. |
| `POST /_littlebox/auth/verify` | app | `{ "token", "password" }` | 200 with the app session cookie. Rechecks the Deployment's current signup policy: under Closed, or for an email no longer listed, it answers 403 and keeps the link usable. |
| `POST /_littlebox/auth/recover` | app | `{ "email" }` | 202, the same answer whether or not the account exists. Active accounts get a recovery link, pending ones a new verification link, suspended and unknown ones nothing. |
| `POST /_littlebox/auth/reset` | app | `{ "token", "password" }` | 200 with a new session; every earlier session of the account ends. |
| `GET /_littlebox/account` | app | | The page links open. Without a token it offers recovery. |
| `POST /_littlebox/owner/signup` | editor | `{ "email", "code", "returnTo"? }`, or `"setupKey"` for the first owner | 202 in required mode, after the code or key is consumed. See [creator admission](#creator-admission). |
| `POST /_littlebox/owner/verify`, `/_littlebox/owner/reset`, `/_littlebox/owner/recover` | editor | as above | The creator scope; links open `/_littlebox/owner/login`. |

A used, expired, foreign or wrong-purpose token answers 410. A pending account that signs in with a correct legacy password, or a suspended one, answers 403. A weak password answers 400 before the token is read. Without a mail transport, signup and recovery answer 503. The creator routes are unavailable when the optional hosted identity adapter is configured.

The runtime MCP tools `auth_signup`, `auth_verify`, `auth_recover` and `auth_reset` run the same flows for the app on the request's host. A signed-out visitor may call them on public and private apps; on a private app they are the only tools a signed-out caller reaches, and `tools/list` and every other call still answer 401. In local mode `auth_signup` also takes `password` and signs in at once. A tool that signs someone in sets the session cookie on the response and never returns a token. A full copy drops sessions and challenges; an import deletes any it finds.

## Creator admission

A creator account needs an invitation code, except the installation's first owner, which the installation key creates. The operator issues codes on the server with `littlebox-kernel creator-code-create [--expires-in 14d] [--note TEXT]`, lists them with `creator-code-list` and stops an open one with `creator-code-revoke <id>`; the README's operator section has the details. A code is single use, expires after 14 days by default, and is stored as a SHA-256 hash in `owners.sqlite`.

`POST /_littlebox/owner/signup` takes `{ "email", "code" }`, plus `"password"` in local mode. The Kernel consumes the code in the transaction that creates the account: a pending account with a verification link in required mode, an active account with a session in local mode. A code for an email whose account is pending is consumed and sends a fresh link to that account. A code for an active or suspended email is consumed and creates nothing, and the answer is the same 202 a new email gets. When both `code` and `setupKey` are sent, the code decides.

The installation key creates an account only while `owners.sqlite` holds no account, or only the first owner's pending account asking for a new link, and no code has ever been issued. The default `local-development-key` keeps creating owners until the first code is issued, so local development and the browser tests still work; the Kernel accepts that key only on loopback unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`.

`GET /_littlebox/owner/config` returns `"admission": "setup-key"` while the key can still create an account and `"code"` otherwise. The owner page and **Platform access** ask for the field it names and never mention the key in `code` mode.

| Request | Status | Message |
| --- | --- | --- |
| Used, expired, revoked, unknown or mistyped code | 403 | `This invitation code is not valid. It may be mistyped, expired, revoked or already used.` |
| Installation key once any code exists or any other account exists, or no admission at all | 403 | `An invitation code is required to create a creator account. Ask the operator of this installation for one.` |
| Wrong installation key while the key can still create the first owner | 403 | `A valid installation setup key is required to create the first owner.` |

A refused signup creates no account and sends no mail. Codes count in the same per-address and per-email sign-in limits as the key did.

An MCP client that starts OAuth while the creator is signed out sends them to `/_littlebox/owner/login?returnTo=/oauth/authorize?...`. The page passes `returnTo` with the signup. The Kernel keeps it only when it starts with `/oauth/authorize?`, has no fragment or control character and fits in 4096 bytes, and puts it in the verification link's query: `/_littlebox/owner/login?returnTo=...#verification=TOKEN`. After the password is set the page opens the consent screen, and approving returns the code to the client's registered redirect. In local mode the signup answers with a session and the page returns to the consent screen at once.

Suspending a creator with `creator-account-suspend` deletes their sessions, open verification and recovery links, delegated MCP credentials and OAuth codes not yet exchanged, and every later use of an OAuth token or credential reads the account state. A creator admitted with a code but suspended before verifying cannot use the link. The code stays used.

## Local development cycle

From this repository, set `LITTLEBOX_MCP_TOKEN` to a credential that can create and publish projects.

```sh
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

Signed-out visitors may call `actions_invoke` 30 times per 10 seconds per client address and Deployment, and each signed-in app user 120 times per 10 seconds. Further calls return a tool error with the text `rate-limited` and a `Retry-After` header. The Kernel uses the socket peer address and reads `X-Forwarded-For` only from peers listed in `LITTLEBOX_TRUSTED_PROXIES`. Such a proxy must append the address it received the request from, never forward the client's header unchanged. Owner calls are audited. For app users the Kernel records only a signed-in `files_upload` that stores a new file. Failed uploads, receipt replays, action calls, reads and session checks write no audit row, so app users cannot grow the shared audit table at will.

A file is private to the app user who uploaded it unless that upload set `visibility` to `public`. Only that user can list or read a private file. On a public app, a signed-out visitor can list and read public files. On a private app, any signed-in user of that app can read public files, and private files stay with the uploader. Files stored before this rule have no owner and stay unreadable to app users. A portable export still includes every file for the owner. The URLs the SDK returns are Blob URLs created after an MCP download. The app never receives an HTTP download path. Each visitor's downloads use a transfer bound to that client address, so anonymous callers do not share one transfer bucket.

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

## Build and publish

```sh
bun run littlebox -- build --dir /tmp/my-app
bun run littlebox -- publish --dir /tmp/my-app
```

A published app runs under the Kernel's Content Security Policy, described in [the architecture](./architecture.md#isolation-of-hosted-code). The inline scripts in an artifact's HTML run because the Kernel allows each by its hash. Inline event handler attributes, scripts from other origins and requests to other origins do not. Attach listeners in JavaScript and serve every script and request from the app's own origin.

The build lives in `.littlebox/artifact.json`. Publishing transfers it over MCP, validates a plan against the project revision and applies it. The tool saves the plan and the receipt in `.littlebox`. The Kernel rejects a stale plan. Repeating a plan that was already applied returns its receipt. `publish` deploys as private unless `littlebox.json` sets `project.manifest.visibility` to `"public"`. A republish keeps the URL and database and stops serving files that the new artifact no longer contains.

There is also `bun run littlebox -- login --dir /tmp/my-app --url http://localhost:4173`. It reads `LITTLEBOX_EMAIL` and `LITTLEBOX_PASSWORD` from the environment, obtains a 24-hour credential and stores it with `0600` permissions in `.littlebox/credentials.json`. `LITTLEBOX_INVITATION_CODE` lets it create the account if it does not exist yet, and `LITTLEBOX_SETUP_KEY` does so for an installation's first owner. On an installation that requires verification it stops and asks you to open the emailed link first. The command adds `.littlebox/` to `.gitignore`; do not copy credentials into the frontend.

## Transfers and limits

The client starts a transfer by declaring the size and SHA-256. Chunks are at most 64 KiB and use base64. The server accepts identical retries at the same offset and rejects different content. Completing a transfer verifies size and hash and makes its bytes immutable.

Owner transfers expire after one hour and each owner credential can hold eight. App user transfers expire after 10 minutes and each app user can hold two. Owners share a pool of 128 per installation, and app users share a pool of 32 per Deployment. An app's files are at most 2 MiB each and the Deployment has a 20 MiB quota. An administrative transfer can be up to 32 MiB; a full publishing payload must fit in 26 MB and its artifact in 16 MiB. A project allows up to 100 actions. Queries limit rows, response size and execution time. Each app database is capped at 256 MiB.

App login and signup accept 10 attempts per minute per client address, and 5 per minute for one email address from one client address, counted separately for each Deployment. Wrong passwords also count against the email address across all clients: after 100 failures in an hour, every login for that email waits, including a correct one. A successful login never uses up that count, and one client address adds at most 20 failures per hour to it, so pausing an account takes failures from at least five addresses. `/_littlebox/owner/login` and `/_littlebox/owner/signup` have the same limits. Recovery requests count against the email they name, and verify and reset requests share one bucket per client address, on each Deployment and on the creator scope; the runtime MCP account tools count in the same buckets. Further attempts get HTTP 429 with `Retry-After`. Each identity database is capped at 64 MiB.

A release's `schema.sql` may only create tables and indexes, and must be safe to run twice. CHECK constraints, expression and partial indexes and generated columns may call SQL functions, except `load_extension`, `readfile`, `writefile`, `edit` and `fts3_tokenizer`, which app actions cannot call either.

`/oauth/register` accepts 10 registrations per client address per hour; IPv6 clients share one bucket per /48. It holds at most 1000 clients that have not reached consent, and at that cap a new registration replaces the oldest of them. Unconsented clients expire after one hour. A client that reached consent expires after 30 days without a new token once it holds no live credential.

The audit log in `mcp.sqlite` records each owner tool call with its project or Deployment. `release_apply` records the project and the Deployment it produced, and `signup_policy_set` records the old and new mode.

`transfer_status` lets a client resume an upload from the last offset. Cancel transfers you no longer need. The SDK checks the checksum of downloads and releases its temporary transfers.

## Status and compatibility

The official SDK checks connection, listing and MCP calls, as well as the OAuth flow with registration and PKCE. The browser tests cover publishing, republishing, migrations, restore and export.

The self-hosted export includes the Kernel, its MCP and OAuth modules, and a deploy script that uses authenticated MCP. The platform image includes Bun and the Drizzle tooling. The minimal self-hosted kit supports administrative SQL and migrations supplied by its owner; it does not include the tool that derives Drizzle models.

`project_export` includes `initialSignupPolicy` with `{ "mode": "open" | "allowlist" | "closed", "allowlist": [...] }` alongside the portable database, accounts and files. It captures `visibility` under the same project lock, so exporters use the snapshot's visibility even if an earlier project inspection or editor copy differs. A release may import that policy only on its first Deployment. The Kernel validates and installs the policy before serving that Deployment, so a public app exported with Closed signup never opens registration during import. Republishing cannot override the policy through initial state. Older exports without these fields use the inspected Deployment's visibility or the editor's current project and the normal signup defaults.

The editor preview still stores its test data in the browser, including uploads, which it keeps as data URLs in local storage. It uses MCP over a MessageChannel with an ephemeral preview credential. The registry of published projects and the credentials live in the Kernel.

Remote development environments, teams with Workspace membership and immutable artifact history are outside this release. Publishing keeps the project's URL and database, matching the Kernel's current behavior. Each Deployment is one folder on the Kernel volume with its record, artifact, database, accounts, files and backups. Scheduled copies send those folders to a second disk or an S3 prefix, and `littlebox-kernel restore` installs a copy on a fresh volume; the README's section on the data directory and copies describes both.
