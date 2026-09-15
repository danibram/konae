# Littlebox

An open-source workshop for making tiny useful apps: edit code like CodePen, ask an agent for changes, and deploy static apps with SQLite, auth and files already attached.

> **Status:** self-hosted vertical slice. "Littlebox" is a working name.

## What works today

- Workspace home with search, project creation, visibility state and direct access to either the editor or latest deployed app.
- Automatic migration of the original single-project browser state into a multi-project workspace index.
- A project tree with nested files and empty folders. `app.js` or `src/main.ts` is the entrypoint, and relative JavaScript, TypeScript, JSX, JSON and CSS imports compile in the browser.
- Sandboxed iframe preview with a small `miniapp` SDK.
- Real SQLite in the browser through `sql.js`, persisted locally and exportable as a `.sqlite` file.
- Per-app signup, login, logout and public or login-required app pages, with a per-app signup policy.
- File uploads. The editor preview keeps them in browser storage; published apps send them to the Kernel in chunked, checksummed MCP transfers.
- Code and data persist through page reloads.
- Provider-independent Kernel policy for public/private shells, declared actions and owner-only administration.
- Opaque Kernel sessions with hashed tokens, expiry and revocation.
- A real in-browser agent on `pi-agent-core`: a streaming tool loop over a disposable copy of the project tree, bring-your-own provider keys kept in the browser, and a Validation Gate every proposal must pass before it reaches the Project. The agent can create nested source files but has no shell.
- Browser-side builds with `esbuild-wasm`. The Kernel receives a static Build Artifact and never runs project code.
- A shared Rust Kernel, Nginx hostname routing, one durable SQLite database per app, local backups and optional S3 storage for static artifacts.
- Named, parameterized server actions: published browsers receive neither raw SQL nor database credentials.
- A separate owner database export endpoint. Deployed browsers cannot reach it.
- A Playwright test that creates a multi-file app, builds it in Chrome, publishes its artifact to MinIO and verifies task data after a reload.

## Prerequisites

- [Bun](https://bun.sh) 1.3 or newer. The Docker images pin Bun 1.3.11.
- A Rust toolchain with Cargo, version 1.91 or newer. The Kernel uses the 2024 edition and its locked AWS SDK crates require 1.91.
- Google Chrome for the Playwright suites, which run with `channel: "chrome"`.
- Docker with Compose, only for the full publishing stack and the MinIO end-to-end test.

## Run locally

```bash
bun install
bun run dev:local
```

This starts the Rust Kernel on port `8790` and the Vite editor on port `4173`. Open [http://localhost:4173](http://localhost:4173). Local app data stays under `data/kernel-local` and persists across restarts; set `LITTLEBOX_DATA_DIR` to move it. The runner does not start Docker, and it removes any S3 settings from the Kernel's environment, so artifacts stay on local disk.

Set `LITTLEBOX_KERNEL_PORT` to change the local Kernel port and `LITTLEBOX_EDITOR_PORT` to change the editor port. Set `LITTLEBOX_KERNEL_URL` to point the editor proxy at a different Kernel.

To run only the Kernel, use `bun run kernel`. It runs `cargo run` on the Kernel's default port `8787` and defaults `LITTLEBOX_DATA_DIR` to `data/kernel-local` and `LITTLEBOX_BUN_PATH` to the Bun on your `PATH`.

To run the complete publishing stack:

```bash
docker compose up --build
```

Open [http://editor.localhost:8080](http://editor.localhost:8080). Published apps receive URLs such as `http://tiny-tasks-a1b2c3.localhost:8080`; `*.localhost` resolves locally without editing `/etc/hosts` on modern browsers.

Compose publishes the gateway, MinIO API and MinIO console on `127.0.0.1` only. The Kernel container binds `0.0.0.0` inside the Compose network with the development setup key, so the Compose file sets `LITTLEBOX_ALLOW_DEFAULT_KEY=1`. Do not copy that setting to a reachable installation.

The local stack creates two MinIO buckets. `littlebox-apps` receives a second copy of each static artifact, and `littlebox-data` receives scheduled copies of the data directory when `LITTLEBOX_COPY_S3_PREFIX` is set. The Kernel volume stays the authority: every app's record, artifact, database, accounts, uploads and backups live there, and a Kernel started on an empty volume serves nothing until `littlebox-kernel restore` installs a copy. See [Data directory and copies](#data-directory-and-copies). Mutations execute against the live database. MinIO's console runs at [http://localhost:9001](http://localhost:9001) with the development credentials `littlebox` and `littlebox-secret`.

If you ran the stack before the Kernel image became non-root, fix the volume once: `docker compose run --rm --user root --entrypoint chown kernel -R 10001:10001 /data`.

The S3 buckets must already exist. The same S3 settings work with MinIO, Cloudflare R2, Scaleway Object Storage and Hetzner Object Storage through their S3 endpoints.

```bash
bun test
bun run build
cargo test --manifest-path kernel-rs/Cargo.toml
bun run test:e2e:local
# Requires the Docker stack
bun run test:e2e:minio
```

## Environment variables

The Kernel reads these at startup. Defaults are the values in `kernel-rs/src` unless a column says otherwise.

| Variable | Default | Meaning |
| --- | --- | --- |
| `LITTLEBOX_KERNEL_PORT` | `8787` (`8790` under `bun run dev:local`) | Port the Kernel listens on. `dev:local` also uses it to build the editor's proxy target. |
| `LITTLEBOX_BIND` | `127.0.0.1` (`0.0.0.0` in the Kernel image) | Address the Kernel listens on. |
| `LITTLEBOX_ADMIN_KEY` | `local-development-key` | Installation setup key that permits owner registration. An empty value counts as unset. |
| `LITTLEBOX_ALLOW_DEFAULT_KEY` | unset | Set to `1` to let the Kernel start on a non-loopback address with the default setup key. Compose sets it for the development stack. |
| `LITTLEBOX_ACCOUNT_VERIFICATION` | `local` | `required` makes new creator and app accounts prove their email before they are active, and moves accounts that never verified to pending with their sessions revoked. `local` keeps unverified signup for development. See [Verified accounts](#verified-accounts). |
| `LITTLEBOX_MAIL_TRANSPORT` | unset | How the Kernel sends account links. `development` appends each message to `mailbox.jsonl` in the data directory. Unset means no email: verification-required signup and recovery answer 503, and `app_invitations_create` refuses. |
| `LITTLEBOX_TRUSTED_PROXIES` | unset | Comma-separated proxy IP addresses whose `X-Forwarded-For` header the Kernel trusts for rate limiting. CIDR ranges are rejected. |
| `LITTLEBOX_DATA_DIR` | `/data` (`data/kernel-local` under `bun run dev:local` and `bun run kernel`) | Data directory: one folder per Deployment under `deployments/<id>/`, plus `owners.sqlite` and `mcp.sqlite`. |
| `LITTLEBOX_PUBLIC_ORIGIN` | `http://localhost:8080` (`http://localhost:<kernel port>` under `dev:local`) | Origin used to build app URLs. Its scheme and port are copied onto each app hostname, and an `https://` origin makes OAuth metadata use HTTPS. |
| `LITTLEBOX_HOST_SUFFIX` | `localhost` | Domain appended to new app hostnames, as in `<slug>-<id>.localhost`. |
| `LITTLEBOX_MCP_ORIGIN` | unset | Public origin of the MCP endpoint, without a path, when it differs from the hosting origin. OAuth discovery advertises it. |
| `LITTLEBOX_S3_ENDPOINT` | unset | S3 endpoint URL. The Kernel always serves artifacts from its data directory; when this is set it also uploads each artifact to `LITTLEBOX_ARTIFACT_BUCKET` and can send copies to `LITTLEBOX_DATA_BUCKET`. When unset, the Kernel ignores the other S3 variables. |
| `LITTLEBOX_S3_REGION` | `us-east-1` | S3 region. |
| `LITTLEBOX_S3_ACCESS_KEY` | `littlebox` | S3 access key. |
| `LITTLEBOX_S3_SECRET_KEY` | `littlebox-secret` | S3 secret key. |
| `LITTLEBOX_ARTIFACT_BUCKET` | `littlebox-apps` | Bucket for static Build Artifacts. |
| `LITTLEBOX_DATA_BUCKET` | `littlebox-data` | Bucket that receives copies when `LITTLEBOX_COPY_S3_PREFIX` is set. |
| `LITTLEBOX_COPY_DIR` | unset | Directory, on another disk, that receives scheduled copies of the data directory. It must be outside `LITTLEBOX_DATA_DIR`. |
| `LITTLEBOX_COPY_S3_PREFIX` | unset | Prefix in `LITTLEBOX_DATA_BUCKET`, such as `copies/`, that receives scheduled copies. Needs `LITTLEBOX_S3_ENDPOINT` and must not start with `deployments/`. |
| `LITTLEBOX_COPY_INTERVAL_SECONDS` | `3600` | Seconds between copies when a copy target is set. The first copy runs at startup. |
| `LITTLEBOX_BUN_PATH` | `bun` | Bun executable the Kernel spawns to derive schema migrations. The child starts with an empty environment, so a bare `bun` is found only in the system default path (`/usr/bin`, `/bin`). Set an absolute path when Bun lives elsewhere. `dev:local`, `bun run kernel` and `littlebox dev` pass their own Bun path. |
| `LITTLEBOX_SOURCE_ROOT` | Kernel working directory | Directory that holds `database-lifecycle/` and the `node_modules` it needs. The Kernel image sets `/opt/littlebox`. |

The Kernel refuses to start when `LITTLEBOX_ADMIN_KEY` is unset or equal to `local-development-key` and either `LITTLEBOX_BIND` is not a loopback address (`localhost`, `127.0.0.1`, `::1` or another loopback IP) or the host in `LITTLEBOX_PUBLIC_ORIGIN` or in `LITTLEBOX_MCP_ORIGIN`, when set, is not `localhost`, a `*.localhost` name or a loopback IP, unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`. Set a long random setup key before exposing a Kernel.

The editor, the `littlebox` CLI and the test configurations read these:

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `LITTLEBOX_EDITOR_PORT` | `dev:local` | `4173` | Port of the Vite editor. |
| `LITTLEBOX_KERNEL_URL` | `dev:local`, `vite.config.ts` | `http://127.0.0.1:<kernel port>` under `dev:local`, `http://127.0.0.1:8787` for plain `bun run dev` | Kernel the editor proxies `/mcp`, `/_littlebox`, `/oauth` and `/.well-known` to. |
| `LITTLEBOX_MCP_TOKEN` | `littlebox` CLI | unset | Bearer token for `init`, `publish` and `export --with-data`. When unset, the CLI reads `.littlebox/credentials.json`. |
| `LITTLEBOX_URL` | `littlebox login`, `littlebox init` | `http://localhost:4173` | Littlebox installation to talk to. `--url` overrides it. |
| `LITTLEBOX_EMAIL`, `LITTLEBOX_PASSWORD` | `littlebox login` | unset | Owner credentials exchanged for a 24-hour MCP token. |
| `LITTLEBOX_SETUP_KEY` | `littlebox login` | unset | Installation setup key, needed only when the owner account does not exist yet. |
| `LITTLEBOX_EDITOR_URL` | `playwright.local.config.ts` | `http://127.0.0.1:4176` | Editor the local end-to-end suite opens. When set outside CI, the suite reuses a server that is already running. |
| `LITTLEBOX_TEST_KERNEL_PORT` | `playwright.local.config.ts` | `8794` | Kernel port for the local end-to-end suite. |
| `LITTLEBOX_PORT` | exported self-host kit | `8787` | Port the exported Kernel publishes, read from the kit's `.env`. |

`littlebox dev` ignores every `LITTLEBOX_` and `VITE_` variable from your shell. It starts its own Kernel on `127.0.0.1` with a generated setup key and data under `.littlebox/dev-data`, and opens signup on the local Deployment so you can create test accounts.

## Data directory and copies

Each app is one folder in the Kernel's data directory. A copy of that folder, with the owner and MCP stores beside it, is the app.

```text
data/
├── owners.sqlite           # owner accounts and sessions
├── mcp.sqlite              # projects, sources, agent credentials, audit
└── deployments/<id>/
    ├── deployment.json     # record, visibility, signup policy
    ├── artifact/           # the static files the Deployment serves
    ├── database.sqlite     # the app's rows
    ├── identity.sqlite     # its accounts and sessions
    ├── files/              # uploads
    └── backups/            # snapshots made from the Data panel
```

A Kernel that finds the older layout, with `artifacts/<id>/` and `backups/<id>/` at the top of the data directory, moves both into the Deployment's folder when it starts and logs each move. It refuses to start when the old and new locations both hold files, and names the two paths.

Set `LITTLEBOX_COPY_DIR`, `LITTLEBOX_COPY_S3_PREFIX` or both to copy the data directory off the volume, by default every hour. Each copy takes every Deployment's data and identity locks in turn, snapshots its SQLite files with `VACUUM INTO`, copies its folder, then writes `manifest.json` with the size and SHA-256 of every file. A directory copy appears under its final name only when complete; an S3 copy is complete once its manifest is uploaded. The Kernel keeps the 24 newest copies and the newest copy of each day for 30 days on each target. The copy is staged under `.copy-staging/` in the data directory, so the volume needs room for one more copy.

A failed copy never stops the Kernel from serving. It is logged, and `/_littlebox/health` reports it under `copies`: each target's `lastAttemptAt`, `lastSuccessAt`, `lastCopyId` and `lastError`, and an `alert` message while any target's last attempt failed. `copies.targets` is empty when no target is set.

To move to a new volume or machine, restore into an empty data directory, then start the Kernel:

```sh
LITTLEBOX_DATA_DIR=/data littlebox-kernel restore --from /mnt/copies
LITTLEBOX_DATA_DIR=/data littlebox-kernel restore --from s3://littlebox-data/copies/ --copy 2026-11-06T14-00-00.000Z
```

Restoring from `s3://` reads `LITTLEBOX_S3_ENDPOINT`, `LITTLEBOX_S3_REGION`, `LITTLEBOX_S3_ACCESS_KEY` and `LITTLEBOX_S3_SECRET_KEY` from the same shell. Without `--copy`, restore takes the newest complete copy. It refuses a data directory that holds anything besides `lost+found`, a copy without a manifest, and any file whose size or checksum differs from the manifest, and then installs nothing. It also runs SQLite's integrity check on every database. After installing, it signs everyone out: owner and app sessions, Clerk creator sessions, agent credentials, OAuth tokens and codes, and unfinished transfers and release plans. Owners create new agent credentials after a restore.

## Authenticated MCP

Open **Platform access** in the editor to create an owner account or sign in. Owner registration requires the installation setup key, `local-development-key` in the local development configuration. The editor uses an HttpOnly owner session; it never receives the setup key at build time.

Agents connect to `/mcp` using OAuth with PKCE or a scoped, expiring bearer token. Apps use the SDK over `/_littlebox/mcp` with their own app sessions. On a public app, a signed-out visitor may call actions declared with `"access": "anonymous"`; every other runtime operation requires an app session, and administration always requires an owner credential. The old operation routes return HTTP 410.

With `LITTLEBOX_MCP_TOKEN` configured, create and develop a project from this repository:

```sh
bun run littlebox -- init --dir /tmp/my-app --name "My app" --url http://localhost:4173
bun run littlebox -- dev --dir /tmp/my-app
bun run littlebox -- publish --dir /tmp/my-app
```

The code stays in that folder. Development starts a separate local Rust Kernel with its own identities, files and database. `publish` deploys as private unless `littlebox.json` sets `project.manifest.visibility` to `"public"`. See [the MCP guide](./docs/mcp.md) for credentials, tools, limits and deployment configuration.

## Mini-app SDK

Apps use the same capability surface in local preview and when published:

```js
const session = await miniapp.auth.session();
await miniapp.auth.signIn();
await miniapp.auth.signOut();

const rows = await miniapp.actions.call("tasks.list");
await miniapp.actions.call("tasks.create", { title: "Ship it" });

const files = await miniapp.files.list();
```

SQL implementations and typed input bindings live in `actions.json`, which stays server-side after publication. The browser only knows action names and returned fields. SQL Studio and `.sqlite` export use the Kernel's separate owner Admin Interface.

Each action declares an access level. On a public Deployment, `anonymous` actions run for signed-out visitors and `authenticated` actions require an app session. Files and uploads always require a session. A private Deployment requires visitors to sign in before the app shell loads, and nothing runs without that login. The Rust Kernel rejects `granted` actions until it supports explicit deployment grants.

Signed-out visitors may call anonymous actions 30 times per 10 seconds per client address and Deployment; further calls return `rate-limited` until the bucket refills. Each signed-in app user may call actions 120 times per 10 seconds. IPv6 clients share one bucket per /64. App login and signup allow 10 attempts per minute per client address and 5 per minute for one email address from one client address, counted separately for each Deployment. After 100 wrong passwords for one email in an hour, logins for that email wait, correct ones included. Each client address adds at most 20 failures per hour to that count, so it takes at least five addresses to pause an account. Owner login and signup have the same limits. Further attempts get HTTP 429 with `Retry-After`. The Kernel counts the socket peer address. It reads `X-Forwarded-For` only when the peer is listed in `LITTLEBOX_TRUSTED_PROXIES` (comma-separated IP addresses, unset by default), and then takes the rightmost entry that is not itself a listed proxy. A trusted proxy must append the address it received the request from to `X-Forwarded-For`, never pass the client's header through unchanged, or clients choose their own bucket. The bundled Nginx configuration appends it with `$proxy_add_x_forwarded_for`; list the Nginx address in `LITTLEBOX_TRUSTED_PROXIES` to count visitors separately behind it. Each app database is capped at 256 MiB and each identity database at 64 MiB; writes past that fail with "database or disk is full".

## Signup policy

Each Deployment has a signup policy that decides who may create an app account:

- Open: anyone can create an account.
- Invitation only: nobody can sign up; people the owner invites by email create their account from the invitation link.
- Closed: no new accounts, and open invitations cannot be accepted.
- Allowlist, the advanced option kept from earlier releases: only listed exact emails or `@domain` entries can create an account. A domain entry matches that exact domain, not its subdomains. Listing an address sends nothing.

Public Deployments start Open. Private Deployments start Closed, so the owner chooses who may join. Deployments published before signup policies existed get the same default from their visibility: Open if public, Closed if private. A first release may name its policy, Invitation only included, in `initialSignupPolicy`, and the Kernel installs it before serving the first request. Republishing keeps the current policy, except that republishing a public app as private turns Open into Closed. The owner changes the policy in the editor's Auth panel or with `signup_policy_set`, which needs the `manage_users` permission. Existing accounts keep working when the policy tightens. A refused signup gets the same answer whether signup is closed or the address is not listed.

In local mode the Kernel does not verify email addresses, so whoever signs up first with a listed address or an address in a listed domain gets that account. Use Invitation only for apps that hold private data, or require verification.

### Invitations

The owner or an agent with `manage_users` invites people with `app_invitations_create`. Each recipient gets an email with a link to `/_littlebox/invitation` on the app's host, valid for seven days. The page shows the invited email, which the person cannot change, and asks for a password. Accepting activates the account and signs the person in. One transaction consumes the link, rechecks the Deployment's current policy and the account's state, sets the password and marks the invitation accepted, so two people answering one link produce one account.

An invitation works under Open and Invitation only, and under an Allowlist only for a listed address. Under Closed the page says the app is not accepting accounts and the link stays usable for when the owner reopens. An email that already has an active account is told to sign in; the invitation never changes its password. A suspended account is not invited and an invitation never resumes it. Resending replaces the link, so the earlier one stops working. Revoking ends a pending invitation and removes the pending account it created, if nobody used it.

Inviting needs a mail transport; without one, `app_invitations_create` refuses and creates nothing. A full copy carries the invitation history and the pending recipients but no link: the export drops every challenge and clears the invitations' token hashes. At the destination a pending invitation shows `needs-resend`, and resending sends a link on the destination's host.

## Verified accounts

Creator accounts and app accounts share one account core in the Kernel. An account is pending, active or suspended, and only an active account gets a session. Every session check reads the state, so suspension ends the sessions an account already holds.

`LITTLEBOX_ACCOUNT_VERIFICATION` chooses the mode for the whole installation:

- `local`, the default: signup creates an active, unverified account and signs it in, as before. This keeps local development working without email.
- `required`: signup creates a pending account with no password and sends a verification link. The person opens the link, chooses the password and becomes active. Recovery sends a 30-minute link that sets a new password and ends every earlier session.

Switching an installation to `required` moves every account that never verified, in both scopes, to pending and revokes its sessions. IDs, rows and file owners stay. Those people enter again through **Forgot your password?**, which sends them a verification link. The switch happens the first time the Kernel opens each identity database in the new mode, so an installation never reaches production with active accounts whose email nobody proved. Switching back to `local` and later to `required` again moves accounts created unverified in between.

`required` needs a mail transport. Without one, signup and recovery answer 503 with an explanation, and active accounts still sign in. Until a production transport exists, `LITTLEBOX_MAIL_TRANSPORT=development` writes each link to `mailbox.jsonl` in the data directory with `0600` permissions; that file holds live links, so use it only on a machine you control.

The operator suspends or resumes a local creator account from the server:

```sh
littlebox-kernel creator-account-suspend owner@example.com
littlebox-kernel creator-account-resume owner@example.com
```

Suspension ends the creator's sessions, open links, MCP credentials and OAuth codes. Projects and app accounts stay. A resumed account is active if it ever verified, and pending otherwise. [docs/mcp.md](docs/mcp.md#verified-accounts) lists the routes and tools.

## Security status

- Runtime requests are bound to the hostname-resolved Deployment; the browser cannot select another database ID.
- Private Deployments require a per-app login before serving the shell. The signup policy decides who can create that login.
- File operations require an authenticated app session. Files are shared among authenticated users within the same Deployment; per-user private files need a separate policy.
- Public HTML is readable by anyone. Data is reachable only through declared actions, and a signed-out visitor can reach only the actions declared `anonymous`.
- Sessions are opaque; only token hashes are persisted. Cookies are `HttpOnly` and host-only. Under HTTPS the owner and app cookies carry the `__Host-` prefix, so a cookie an app sets with `Domain=<suffix>` never authenticates the editor or another app.
- Every app response carries a Content Security Policy. Scripts run only from the app's origin or as the exact inline scripts of its `index.html`, matched by SHA-256 hash, so inline event handler attributes and foreign scripts do not run. Requests reach only the app's own origin. No host can be framed by another origin, and HSTS is sent on every host when the public origin is HTTPS. Administrative MCP and owner sign-in refuse a request whose `Origin` is an app host before they read any credential. [The architecture](./docs/architecture.md#isolation-of-hosted-code) lists the policy.
- Pending and suspended accounts get no session. Verification and recovery links are single use, expire, answer only their own purpose and scope, and are stored as hashes. Opening a link changes nothing until the person posts the form.
- Every Deployment gets a separate SQLite working file. The Kernel writes mutations transactionally and stores backups on explicit requests and before migrations.
- Action inputs are typed, parameterized and quota-limited; structural SQL is rejected from runtime actions.
- Owners use first-party sessions and delegated credentials. `LITTLEBOX_ADMIN_KEY` is an installation setup key for owner registration and never enters the editor bundle. The Kernel binds to `127.0.0.1` by default and refuses a non-loopback bind with the default setup key. Remote installations must configure their public origin, TLS and setup key.
- The repository's two Docker images run as unprivileged users. The Dockerfile in an exported self-host kit still runs the Kernel as root.

## Export and self-host

Create either shared tasks or private notes from the project dashboard. The notes starter has a public landing page and actions that bind each row to the signed-in user. The Auth panel lets you simulate Guest, Alice and Bob in the preview; published apps use their own accounts.

Use Data to choose the local or deployed database, edit the model, review a generated migration, create backups and restore them. Publishing the same project keeps its URL and database, and the republished app stops serving files that the new artifact no longer contains. Local and deployed model acceptance are tracked separately.

Export project downloads a tar archive with source files, a built frontend, the Rust Kernel and startup scripts. For a published app it also includes the database, users and uploads. Imported users keep their password hashes, while sessions are cleared. The combined state limit is 16 MiB and each file is limited to 2 MiB. The archive documents two ways to run it: Docker, or native Cargo with Rust 1.91 or newer. Its `deploy.mjs` publishes the app as private unless the exported project manifest sets `visibility` to `"public"`.

External projects can export through the CLI too. A source-only kit needs no connection or credential. Add `--with-data` to include the published app's database, users and uploads through authenticated MCP.

```sh
bun run littlebox -- export --dir /tmp/my-app --out /tmp/my-app-selfhost.tar
bun run littlebox -- export --dir /tmp/my-app --out /tmp/my-app-with-data.tar --with-data
```

The command refuses to overwrite an archive and writes it with owner-only permissions. Live-state exports retain the deployed visibility and signup policy. The target installs that policy with its first deployment. Exports with live state contain password hashes and attachments; keep them private. Extracted snapshots have owner-only permissions and the kit's Git rules exclude them.

Eight runnable examples and their MCP replay are in [examples/product-fit](./examples/product-fit/README.md). The [product-fit assessment](./docs/research/product-fit.md) records tested workflows, remaining limits and the proposed next primitives.

Provider-free testing covers the editor and offline demo. Connect a provider in Agent settings to generate arbitrary apps with a model.

## Next slice

1. Add Workspace membership and team administration above the existing owner accounts.
2. Add immutable release history, data-aware rollback and file uploads backed by the private bucket.

Architecture and security boundaries are described in [docs/architecture.md](./docs/architecture.md).
