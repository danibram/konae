# Littlebox

An open-source workshop for making tiny useful apps: edit code like CodePen, ask an agent for changes, and deploy static apps with SQLite, auth and files already attached.

> **Status:** self-hosted vertical slice. "Littlebox" is a working name.

## What works today

- Workspace home with search, project creation, visibility state and direct access to either the editor or latest deployed app.
- Automatic migration of the original single-project browser state into a multi-project workspace index.
- A project tree with nested files and empty folders. `app.js` or `src/main.ts` is the entrypoint, and relative JavaScript, TypeScript, JSX, JSON and CSS imports compile in the browser.
- Sandboxed iframe preview with a small `miniapp` SDK.
- Real SQLite in the browser through `sql.js`, persisted locally and exportable as a `.sqlite` file.
- Per-app signup, login, logout and public or login-required app pages, with a per-app signup policy: Open, Invitation only, Closed or Allowlist.
- Verified accounts for creators and app users, with recovery and suspension, and account email through a persistent queue with Resend, SMTP or a development mailbox.
- Creator admission with operator-issued invitation codes, and app invitations by email from the editor's Users view or over MCP.
- File uploads, private to the uploader unless made public, which the uploader can change or delete later. The editor preview keeps them in browser storage; published apps send them to the Kernel in chunked, checksummed MCP transfers.
- Code and data persist through page reloads.
- Provider-independent Kernel policy for public/private shells, declared actions and owner-only administration.
- Opaque Kernel sessions with hashed tokens, expiry and revocation.
- A real in-browser agent on `pi-agent-core`: a streaming tool loop over a disposable copy of the project tree, bring-your-own provider keys kept in the browser, and a Validation Gate every proposal must pass before it reaches the Project. The agent can create nested source files but has no shell.
- Browser-side builds with `esbuild-wasm`. The Kernel receives a static Build Artifact and never runs project code.
- A shared Rust Kernel, Nginx hostname routing, one durable SQLite database per app, local backups and optional S3 storage for static artifacts.
- Named, parameterized server actions: published browsers receive neither raw SQL nor database credentials. A mutation may declare a guard that answers when the app's own rule changes no row.
- HTTP routes on anonymous actions, for signed webhooks, programs with an API key and signed-in people, and a file read path, `GET /_littlebox/files/<id>`.
- Usage and Signals per Deployment, read by the owner.
- An Action Contract for every project and `littlebox conform`, which checks a running backend against it.
- Multi-page static publishing with `404.html`, extensionless paths and an opt-in single-page fallback.
- Owner-only export of an app's database, accounts and files as a chunked package behind the `export_data` permission, and its import on another Kernel. Deployed browsers cannot reach it.
- Scheduled copies of the data directory to another disk or S3, `littlebox-kernel restore`, and a revocation record that keeps suspensions and revoked keys through a restore.
- Migration of one app to another Kernel with one writer at a time, and Eject of a Managed project to its owner.
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
| `LITTLEBOX_ADMIN_KEY` | `local-development-key` | Installation setup key that creates the first owner. Later creators need an invitation code; see [Creator admission](#creator-admission). An empty value counts as unset. |
| `LITTLEBOX_ALLOW_DEFAULT_KEY` | unset | Set to `1` to let the Kernel start on a non-loopback address with the default setup key. Compose sets it for the development stack. |
| `LITTLEBOX_ACCOUNT_VERIFICATION` | `local` | `required` makes new creator and app accounts prove their email before they are active, and moves accounts that never verified to pending with their sessions revoked. `local` keeps unverified signup for development. See [Verified accounts](#verified-accounts). |
| `LITTLEBOX_MAIL_TRANSPORT` | unset | How the Kernel sends account links: `resend`, `smtp` or `development`. Every transport sends through the queue in `mail.sqlite`. `development` appends each message to `mailbox.jsonl` in the data directory. Unset means no email: verification-required signup and recovery answer 503. See [Account email](#account-email). |
| `LITTLEBOX_MAIL_FROM` | `Littlebox <littlebox@localhost>` for `development`; required otherwise | Sender address, such as `Littlebox <no-reply@mail.example.com>`. Its domain is the one the preflight checks. |
| `LITTLEBOX_MAIL_SECRET` | derived from `LITTLEBOX_ADMIN_KEY` | Secret, at least 32 characters, from which the Kernel derives the key that encrypts queued links. When unset the Kernel derives the key from `LITTLEBOX_ADMIN_KEY`, and refuses to start `resend` or `smtp` if that is still `local-development-key`. |
| `LITTLEBOX_RESEND_API_KEY` | unset | Resend API key for `resend`. A sending-only key is enough to send. |
| `LITTLEBOX_RESEND_DOMAINS_KEY` | `LITTLEBOX_RESEND_API_KEY` | Resend key that can list domains, for the sender preflight. Set it when the sending key has `sending_access` only. |
| `LITTLEBOX_RESEND_WEBHOOK_SECRET` | unset | The `whsec_` signing secret of the Resend webhook that posts to `/_littlebox/mail/resend-events`. Unset means the route answers 404 and jobs stop at accepted. |
| `LITTLEBOX_RESEND_API_URL` | `https://api.resend.com` | Resend API base URL. Tests point it at a local server. |
| `LITTLEBOX_SMTP_HOST` | unset | SMTP server for `smtp`. |
| `LITTLEBOX_SMTP_PORT` | `587`, or `465` with implicit TLS | SMTP port. |
| `LITTLEBOX_SMTP_TLS` | `starttls` | `starttls` requires STARTTLS on a plain connection; `implicit` uses TLS from the first byte. `none` is accepted only for a loopback host. |
| `LITTLEBOX_SMTP_USERNAME`, `LITTLEBOX_SMTP_PASSWORD` | unset | SMTP credentials. Set both or neither. |
| `LITTLEBOX_MAIL_LIMIT_RECIPIENT_PER_MINUTE` | `3` | Account email requests one address may receive per minute. |
| `LITTLEBOX_MAIL_LIMIT_RECIPIENT_PER_DAY` | `10` | Account email requests one address may receive per 24 hours. |
| `LITTLEBOX_MAIL_LIMIT_CREATOR_PER_DAY` | `100` | Account emails the apps of one creator may queue per 24 hours. |
| `LITTLEBOX_MAIL_LIMIT_APP_PER_DAY` | `100` | Account emails one Deployment may queue per 24 hours. |
| `LITTLEBOX_MAIL_LIMIT_INSTALLATION_PER_DAY` | `1000` | Emails of every kind, account email and notices, the installation may queue per 24 hours. Keep it at or below the provider's contracted limit. |
| `LITTLEBOX_MAIL_LIMIT_CLIENT_PER_HOUR` | `20` | Account email requests one client address may make per hour. |
| `LITTLEBOX_MAIL_LIMIT_NOTIFY_APP_PER_DAY` | `200` | Notices (`notify` on a mutation) one Deployment may queue per 24 hours. |
| `LITTLEBOX_MAIL_LIMIT_NOTIFY_CREATOR_PER_DAY` | `500` | Notices the apps of one creator may queue per 24 hours. |
| `LITTLEBOX_MAIL_LIMIT_NOTIFY_RECIPIENT_PER_DAY` | `50` | Notices one address may receive from one Deployment per 24 hours. |
| `LITTLEBOX_MAIL_WORKER_INTERVAL_MS` | `5000` | How often the worker looks for due jobs. A new job also wakes it. |
| `LITTLEBOX_CALLS_PER_MINUTE` | `20` | Calls one app's call actions may queue in any 60 seconds, from 1 to 10000. The Kernel sends them from `calls.sqlite`; see [docs/mcp.md](./docs/mcp.md#calls-between-apps). |
| `LITTLEBOX_FILES_QUOTA_BYTES` | `20971520` (20 MiB) | Bytes the files of one Deployment may hold together. An upload past it is refused with the configured number, and so is a package whose files exceed it. |
| `LITTLEBOX_FILE_MAX_BYTES` | `2097152` (2 MiB) | Bytes one file may hold. It must not exceed `LITTLEBOX_FILES_QUOTA_BYTES`; the Kernel refuses to start otherwise. Uploads and reads still move in 64 KiB MCP blocks, so a larger file takes more calls. |
| `LITTLEBOX_FILES_S3_BUCKET` | unset | Bucket that holds the bytes of app files, as `<prefix><deployment id>/<file id>`. Unset keeps them in each Deployment's `files/` folder. The index stays in the folder either way. See [File bytes in a bucket](#file-bytes-in-a-bucket). |
| `LITTLEBOX_FILES_S3_ENDPOINT`, `LITTLEBOX_FILES_S3_ACCESS_KEY`, `LITTLEBOX_FILES_S3_SECRET_KEY` | unset | Required with `LITTLEBOX_FILES_S3_BUCKET`. They are separate from `LITTLEBOX_S3_*`, so the private file bytes can use another bucket and key than the artifacts. |
| `LITTLEBOX_FILES_S3_REGION` | `us-east-1` | Region of the file bucket. R2 uses `auto`. |
| `LITTLEBOX_FILES_S3_PREFIX` | empty | Prefix inside the file bucket, such as `files/`. |
| `LITTLEBOX_FILES_S3_PATH_STYLE` | `1` | `1` addresses the bucket as `<endpoint>/<bucket>`, `0` as `<bucket>.<endpoint host>`. |
| `LITTLEBOX_TRUSTED_PROXIES` | unset | Comma-separated proxy IP addresses whose `X-Forwarded-For` header the Kernel trusts for rate limiting. CIDR ranges are rejected. |
| `LITTLEBOX_DATA_DIR` | `/data` (`data/kernel-local` under `bun run dev:local` and `bun run kernel`) | Data directory: one folder per Deployment under `deployments/<id>/`, plus `owners.sqlite`, `mcp.sqlite`, `mail.sqlite` and `calls.sqlite`. |
| `LITTLEBOX_PUBLIC_ORIGIN` | `http://localhost:8080` (`http://localhost:<kernel port>` under `dev:local`) | Origin used to build app URLs. Its scheme and port are copied onto each app hostname, and an `https://` origin makes OAuth metadata use HTTPS. |
| `LITTLEBOX_HOST_SUFFIX` | `localhost` | Domain appended to new app hostnames, as in `<slug>-<id>.localhost`. |
| `LITTLEBOX_MCP_ORIGIN` | unset | Public origin of the MCP endpoint, without a path, when it differs from the hosting origin. OAuth discovery advertises it. |
| `LITTLEBOX_DNS_RESOLVER` | the first `nameserver` in `/etc/resolv.conf` | The resolver, as an IP address with an optional port, that the Kernel asks for a custom domain's TXT record `_littlebox.<hostname>` when `domain_set` runs (ADR-0015). |
| `LITTLEBOX_S3_ENDPOINT` | unset | S3 endpoint URL. The Kernel always serves artifacts from its data directory; when this is set it also uploads each artifact to `LITTLEBOX_ARTIFACT_BUCKET` and can send copies to `LITTLEBOX_DATA_BUCKET`. When unset, the Kernel ignores the other S3 variables. |
| `LITTLEBOX_S3_REGION` | `us-east-1` | S3 region. |
| `LITTLEBOX_S3_ACCESS_KEY` | `littlebox` | S3 access key. |
| `LITTLEBOX_S3_SECRET_KEY` | `littlebox-secret` | S3 secret key. |
| `LITTLEBOX_ARTIFACT_BUCKET` | `littlebox-apps` | Bucket for static Build Artifacts. |
| `LITTLEBOX_DATA_BUCKET` | `littlebox-data` | Bucket that receives copies when `LITTLEBOX_COPY_S3_PREFIX` is set. |
| `LITTLEBOX_COPY_DIR` | unset | Directory, on another disk, that receives scheduled copies of the data directory. It must be outside `LITTLEBOX_DATA_DIR`. |
| `LITTLEBOX_COPY_S3_PREFIX` | unset | Prefix in `LITTLEBOX_DATA_BUCKET`, such as `copies/`, that receives scheduled copies. Needs `LITTLEBOX_S3_ENDPOINT` and must not start with `deployments/`. |
| `LITTLEBOX_COPY_INTERVAL_SECONDS` | `3600` | Seconds between copies when a copy target is set. The first copy runs at startup. |
| `LITTLEBOX_SCHEDULE_MIN_SECONDS` | unset | For tests only. Runs every scheduled action once every this many seconds, 1 to 300, instead of at its declared times, so a test can watch a run. The Kernel refuses to start with it unless `LITTLEBOX_BIND` is a loopback address. |
| `LITTLEBOX_COPY_SECRET` | unset | Secret, at least 32 characters, that encrypts every file of a copy sent to S3 with AES-256-GCM. `restore` needs the same secret to open such a copy. Directory copies stay unencrypted. |
| `LITTLEBOX_BUN_PATH` | `bun` | Bun executable the Kernel spawns to derive schema migrations. The child starts with an empty environment, so a bare `bun` is found only in the system default path (`/usr/bin`, `/bin`). Set an absolute path when Bun lives elsewhere. `dev:local`, `bun run kernel` and `littlebox dev` pass their own Bun path. |
| `LITTLEBOX_SOURCE_ROOT` | Kernel working directory | Directory that holds `database-lifecycle/` and the `node_modules` it needs. The Kernel image sets `/opt/littlebox`. |
| `LITTLEBOX_DERIVE_HELPER` | unset (`/usr/local/bin/littlebox-derive` in the Kernel image) | Setuid helper that derives schema migrations as a user with no access to the data directory. A Kernel that is not loopback-only refuses schema derivation without it. |
| `LITTLEBOX_CLERK_ISSUER`, `LITTLEBOX_CLERK_JWKS_FILE`, `LITTLEBOX_CLERK_AUTHORIZED_ORIGINS`, `LITTLEBOX_CLERK_BETA_SUBJECTS`, `LITTLEBOX_CLERK_PUBLISHABLE_KEY` | unset | The optional hosted creator sign-in adapter. Unset keeps it off. [Hosted creator access](./docs/clerk-creator-access.md) describes each. |
| `LITTLEBOX_GOOGLE_TEST_ISSUER` | unset | Points Google sign-in at a local fake issuer (`<base>/authorize`, `/token`, `/jwks`) for development; `bun run scripts/fake-google.ts` is one. Read only by a development build with a local HTTP public origin; a release build refuses to start with it set. Google sign-in has no other installation setting: each app's owner sets the app's own Google client in Auth > Users. |

The Kernel refuses to start when `LITTLEBOX_ADMIN_KEY` is unset or equal to `local-development-key` and either `LITTLEBOX_BIND` is not a loopback address (`localhost`, `127.0.0.1`, `::1` or another loopback IP) or the host in `LITTLEBOX_PUBLIC_ORIGIN` or in `LITTLEBOX_MCP_ORIGIN`, when set, is not `localhost`, a `*.localhost` name or a loopback IP, unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`. Set a long random setup key before exposing a Kernel.

The editor, the `littlebox` CLI and the test configurations read these:

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `LITTLEBOX_EDITOR_PORT` | `dev:local` | `4173` | Port of the Vite editor. |
| `LITTLEBOX_KERNEL_URL` | `dev:local`, `vite.config.ts` | `http://127.0.0.1:<kernel port>` under `dev:local`, `http://127.0.0.1:8787` for plain `bun run dev` | Kernel the editor proxies `/mcp`, `/_littlebox`, `/oauth` and `/.well-known` to. |
| `LITTLEBOX_MCP_TOKEN` | `littlebox` CLI | unset | Bearer token for `init`, `publish`, `export --with-data` and `eject`. When unset, the CLI reads `.littlebox/credentials.json`. |
| `LITTLEBOX_URL` | `littlebox login`, `littlebox init`, `littlebox eject` | `http://localhost:4173` | Littlebox installation to talk to. `--url` overrides it. |
| `LITTLEBOX_EMAIL`, `LITTLEBOX_PASSWORD` | `littlebox login` | unset | Owner credentials exchanged for a 24-hour MCP token. |
| `LITTLEBOX_SETUP_KEY` | `littlebox login` | unset | Installation setup key, needed only to create an installation's first owner. |
| `LITTLEBOX_INVITATION_CODE` | `littlebox login` | unset | Invitation code that creates the account when it does not exist yet. Under required verification, add `--verify-from <data folder>/mailbox.jsonl` on a development Kernel to answer the emailed link in the same run. |
| `LITTLEBOX_EDITOR_URL` | `playwright.local.config.ts` | `http://127.0.0.1:4176` | Editor the local end-to-end suite opens. When set outside CI, the suite reuses a server that is already running. |
| `LITTLEBOX_TEST_KERNEL_PORT` | `playwright.local.config.ts` | `8794` | Kernel port for the local end-to-end suite. |
| `LITTLEBOX_PORT` | exported self-host kit | `8787` | Port the exported Kernel publishes, read from the kit's `.env`. |

`littlebox dev` ignores every `LITTLEBOX_` and `VITE_` variable from your shell. It starts its own Kernel on `127.0.0.1` with a generated setup key and data under `.littlebox/dev-data`, and opens signup on the local Deployment so you can create test accounts.

## Data directory and copies

Each app is one folder in the Kernel's data directory. A copy of that folder, with the owner and MCP stores beside it, is the app.

```text
data/
├── owners.sqlite           # creator accounts, sessions and invitation codes
├── mcp.sqlite              # projects, sources, agent credentials, signing secrets, audit
├── mail.sqlite             # the account email queue
├── calls.sqlite            # the call actions' queue and outbox
├── transfers/              # unfinished MCP uploads and downloads
└── deployments/<id>/
    ├── deployment.json     # record, visibility, signup policy
    ├── artifact/           # the static files the Deployment serves
    ├── database.sqlite     # the app's rows
    ├── identity.sqlite     # its accounts, sessions, invitations and API keys
    ├── usage.sqlite        # Usage and Signals
    ├── files/              # the file index, and the uploads unless a bucket holds them
    └── backups/            # snapshots made from the Data panel
```

[The architecture](./docs/architecture.md#current-self-hosted-topology) lists the files that appear only during a migration, after a restore or with the development mailbox.

A Kernel that finds the older layout, with `artifacts/<id>/` and `backups/<id>/` at the top of the data directory, moves both into the Deployment's folder when it starts and logs each move. It refuses to start when the old and new locations both hold files, and names the two paths.

Set `LITTLEBOX_COPY_DIR`, `LITTLEBOX_COPY_S3_PREFIX` or both to copy the data directory off the volume, by default every hour. Each copy takes every Deployment's data and identity locks in turn, snapshots its SQLite files with `VACUUM INTO`, copies its folder, then writes `manifest.json` with the size and SHA-256 of every file. A directory copy appears under its final name only when complete; an S3 copy is complete once its manifest is uploaded. The Kernel keeps the 24 newest copies and the newest copy of each day for 30 days on each target. The copy is staged under `.copy-staging/` in the data directory, so the volume needs room for one more copy.

A failed copy never stops the Kernel from serving. It is logged, and `/_littlebox/health` reports it under `copies`: each target's `lastAttemptAt`, `lastSuccessAt`, `lastCopyId` and `lastError`, and an `alert` message while any target's last attempt failed. `copies.targets` is empty when no target is set.

### File bytes in a bucket

With `LITTLEBOX_FILES_S3_BUCKET` set, the bytes of app files live in that bucket and each Deployment's `files/index.json` stays in its folder, which still decides which files exist and who may read them ([ADR-0024](./docs/adr/0024-a-deployments-file-bytes-may-live-outside-its-folder.md)). The Kernel sends only `PutObject`, `GetObject` with a range, `DeleteObject` and `ListObjectsV2`, and every byte still passes through it; the browser never gets a bucket URL. It has been tested against SeaweedFS, not yet against R2, Scaleway or Hetzner.

`files-store.json` in the data directory records which store holds the bytes, and a Kernel whose settings name another store refuses to start. To move the bytes, stop the Kernel and run, with the `LITTLEBOX_FILES_S3_` settings set in both directions:

```sh
LITTLEBOX_DATA_DIR=/data littlebox-kernel files-migrate --to s3
LITTLEBOX_DATA_DIR=/data littlebox-kernel files-migrate --to local
```

It copies every indexed file, reads each back and compares its SHA-256, records the new store, and only then removes the old copies. A missing file or a mismatch stops it before anything switches. An upload writes its bytes before its index entry and a delete removes the entry first, so a crash can leave bytes no index names. `littlebox-kernel files-reconcile` removes those once they are an hour old, and `--dry-run` only counts them.

A copy carries the file bytes, read from the bucket after the Deployment's locks are released, so a restore needs no bucket and lands the bytes in each folder. Run `files-migrate --to s3` after a restore to put them back in the bucket.

To move to a new volume or machine, restore into an empty data directory, then start the Kernel:

```sh
LITTLEBOX_DATA_DIR=/data littlebox-kernel restore --from /mnt/copies
LITTLEBOX_DATA_DIR=/data littlebox-kernel restore --from s3://littlebox-data/copies/ --copy 2026-11-06T14-00-00.000Z
```

Restoring from `s3://` reads `LITTLEBOX_S3_ENDPOINT`, `LITTLEBOX_S3_REGION`, `LITTLEBOX_S3_ACCESS_KEY` and `LITTLEBOX_S3_SECRET_KEY` from the same shell, and `LITTLEBOX_COPY_SECRET` when the copies are encrypted. Without `--copy`, restore takes the newest complete copy. It refuses a data directory that holds anything besides `lost+found`, a copy without a manifest, any file whose size or checksum differs from the manifest, and an encrypted copy without its secret or with another one, and then installs nothing. It also runs SQLite's integrity check on every database. After installing, it signs everyone out: owner and app sessions, Clerk creator sessions, agent credentials, OAuth tokens and codes, open verification, recovery and invitation links, and unfinished transfers and release plans. Owners create new agent credentials after a restore.

A copy can be up to an hour old, so each copy target also keeps a record of what was revoked since: every app account suspension, creator suspension and API key revocation is written to `revocations/` on each target before the tool reports it done. Restore applies that record before installing, so a restored installation never brings back a suspended account or a revoked key. If the record cannot be read, because the target is unreachable or the source has no `revocations/started.json`, the restore still installs but the installation starts in maintenance: every app shows its maintenance page and owner changes are refused. Then either apply the record once it is reachable, or reopen without it after checking suspensions and keys by hand:

```sh
LITTLEBOX_DATA_DIR=/data littlebox-kernel reconcile --from /mnt/copies
LITTLEBOX_DATA_DIR=/data littlebox-kernel reconcile --without-record
```

Run `reconcile` with the Kernel stopped, or start it again afterwards: a running Kernel keeps the maintenance it read at startup.

A Deployment paused or retired by a migration stays so after a restore, because its migration state travels in the copy.

## Authenticated MCP

Open **Platform access** in the editor to create an owner account or sign in. The installation setup key, `local-development-key` in the local development configuration, creates the first owner; every later creator signs up with an invitation code, as [Creator admission](#creator-admission) explains. The editor uses an HttpOnly owner session; it never receives the setup key at build time.

Agents connect to `/mcp` using OAuth with PKCE or a scoped, expiring bearer token. Apps use the SDK over `/_littlebox/mcp` with their own app sessions. On a public app, a signed-out visitor may call actions declared with `"access": "anonymous"`; every other runtime operation requires an app session, and administration always requires an owner credential. The old operation routes return HTTP 410, except the file read path `GET /_littlebox/files/<id>`, which answers its allowed callers under the file's visibility.

With `LITTLEBOX_MCP_TOKEN` configured, create and develop a project from this repository:

```sh
bun run littlebox -- init --dir /tmp/my-app --name "My app" --url http://localhost:4173
bun run littlebox -- dev --dir /tmp/my-app
bun run littlebox -- publish --dir /tmp/my-app
```

The code stays in that folder. Development starts a separate local Rust Kernel with its own identities, files and database. `publish` deploys as private unless `littlebox.json` sets `project.manifest.visibility` to `"public"`. `project.manifest.signupPolicy` names who may create an app account on the first release; `init` writes private and `{ "mode": "invitation" }`, and a later publish that names another policy warns and changes nothing. See [the MCP guide](./docs/mcp.md) for credentials, tools, limits and deployment configuration. [The MCP walkthrough](./docs/mcp-walkthrough.md) takes one app from a credential through publishing, invitations, export, import, Eject and migration, with each command and the Kernel's exact answer; `login --manage-users` gets the permission that inviting needs.

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

Each action declares an access level. On a public Deployment, `anonymous` actions run for signed-out visitors and `authenticated` actions require an app session. Signed-out visitors may list and read public files; uploads and private files require a session. A private Deployment requires visitors to sign in before the app shell loads, and its runtime endpoint runs nothing without that login. The HTTP route of an `anonymous` action answers its own callers, a signature, an API key or a session, on either kind of Deployment; [the MCP guide](./docs/mcp.md#who-may-call-a-route) lists the rules. The Rust Kernel rejects `granted` actions until it supports explicit deployment grants.

Signed-out visitors may call anonymous actions 30 times per 10 seconds per client address and Deployment; further calls return `rate-limited` until the bucket refills. Each signed-in app user may call actions 120 times per 10 seconds. IPv6 clients share one bucket per /64. App login and signup allow 10 attempts per minute per client address and 5 per minute for one email address from one client address, counted separately for each Deployment. After 100 wrong passwords for one email in an hour, logins for that email wait, correct ones included. Each client address adds at most 20 failures per hour to that count, so it takes at least five addresses to pause an account. Owner login and signup have the same limits. Further attempts get HTTP 429 with `Retry-After`. The Kernel counts the socket peer address. It reads `X-Forwarded-For` only when the peer is listed in `LITTLEBOX_TRUSTED_PROXIES` (comma-separated IP addresses, unset by default), and then takes the rightmost entry that is not itself a listed proxy. A trusted proxy must append the address it received the request from to `X-Forwarded-For`, never pass the client's header through unchanged, or clients choose their own bucket. The bundled Nginx configuration appends it with `$proxy_add_x_forwarded_for`; list the Nginx address in `LITTLEBOX_TRUSTED_PROXIES` to count visitors separately behind it. Each app database is capped at 256 MiB and each identity database at 64 MiB; writes past that fail with "database or disk is full".

## Signup policy

Each Deployment has a signup policy that decides who may create an app account:

- Open: anyone can create an account.
- Invitation only: nobody can sign up; people the owner invites by email create their account from the invitation link.
- Closed: nobody can sign up, and a person the owner invites can still accept the invitation.
- Allowlist, the advanced option kept from earlier releases: only listed exact emails or `@domain` entries can create an account. A domain entry matches that exact domain, not its subdomains. Listing an address sends nothing.

Public Deployments start Open. Private Deployments start Closed, so the owner chooses who may join. Deployments published before signup policies existed get the same default from their visibility: Open if public, Closed if private. A first release may name its policy, Invitation only included, in `initialSignupPolicy`, and the Kernel installs it before serving the first request. Republishing keeps the current policy, except that republishing a public app as private turns Open into Closed. The owner changes the policy in the editor's Auth panel or with `signup_policy_set`, which needs the `manage_users` permission. Existing accounts keep working when the policy tightens. A refused signup gets the same answer whether signup is closed or the address is not listed.

In local mode the Kernel does not verify email addresses, so whoever signs up first with a listed address or an address in a listed domain gets that account. Use Invitation only for apps that hold private data, or require verification.

### Invitations

The owner or an agent with `manage_users` invites people with `app_invitations_create`. Each recipient gets an email with a link to `/_littlebox/invitation` on the app's host, valid for seven days. The page shows the invited email, which the person cannot change, and asks for a password. Accepting activates the account and signs the person in. One transaction consumes the link, rechecks the Deployment's current policy and the account's state, sets the password and marks the invitation accepted, so two people answering one link produce one account.

An invitation works under Open, Invitation only and Closed, and under an Allowlist only for a listed address, where the page otherwise says the app is not accepting accounts and the link stays usable until the owner lists it. Closing signup does not end pending invitations; revoke the ones that should end. An email that already has an active account is told to sign in; the invitation never changes its password. A suspended account is not invited and an invitation never resumes it. Resending replaces the link, so the earlier one stops working. Revoking ends a pending invitation and removes the pending account it created, if nobody used it.

Inviting needs a mail transport; without one, `app_invitations_create` refuses and creates nothing. An export or a package carries the invitation history and the pending recipients but no link, because the export drops every challenge and clears the invitations' token hashes. At the destination a pending invitation shows `needs-resend`, and resending sends a link on the destination's host.

## Verified accounts

Creator accounts and app accounts share one account core in the Kernel. An account is pending, active or suspended, and only an active account gets a session. Every session check reads the state, so suspension ends the sessions an account already holds.

`LITTLEBOX_ACCOUNT_VERIFICATION` chooses the mode for the whole installation:

- `local`, the default: signup creates an active, unverified account and signs it in, as before. This keeps local development working without email.
- `required`: signup creates a pending account with no password and sends a verification link. The person opens the link, chooses the password and becomes active. Recovery sends a 30-minute link that sets a new password and ends every earlier session.

Switching an installation to `required` moves every account that never verified, in both scopes, to pending and revokes its sessions. IDs, rows and file owners stay. Those people enter again through **Forgot your password?**, which sends them a verification link. The switch happens the first time the Kernel opens each identity database in the new mode, so an installation never reaches production with active accounts whose email nobody proved. Switching back to `local` and later to `required` again moves accounts created unverified in between.

`required` needs a mail transport. Without one, signup and recovery answer 503 with an explanation, and active accounts still sign in. `LITTLEBOX_MAIL_TRANSPORT=development` writes each link to `mailbox.jsonl` in the data directory with `0600` permissions; that file holds live links, so use it only on a machine you control.

## Account email

Signup, recovery and invitation links leave through a queue in `mail.sqlite` in the data directory. The Kernel stores the job in the same transaction as the link's challenge, so a request either stores both or neither. A worker inside the Kernel sends due jobs every few seconds and whenever a job arrives; a request never waits for the provider.

Choose a transport with `LITTLEBOX_MAIL_TRANSPORT`:

- `resend` sends through Resend's HTTP API. Set `LITTLEBOX_RESEND_API_KEY` and `LITTLEBOX_MAIL_FROM` with an address on a domain verified in Resend. Each request carries the job ID as its `Idempotency-Key`, so a retry after a restart is the same email to Resend.
- `smtp` sends through any SMTP server with STARTTLS (the default) or implicit TLS. Set `LITTLEBOX_SMTP_HOST`, the credentials and `LITTLEBOX_MAIL_FROM`. SMTP has no idempotency key: a Kernel stopped between the server's answer and the queue's record sends that one email twice.
- `development` writes to `mailbox.jsonl`, as above.

Before it sends anything, the Kernel checks the sender. For Resend the sender's domain must be `verified` in the account; a key with `sending_access` cannot read domains, so set `LITTLEBOX_RESEND_DOMAINS_KEY` to a full-access key or the check refuses. For SMTP the check connects, negotiates TLS and authenticates; whether the server lets that account send as the sender address is the provider's rule, and a refusal there shows as failed jobs. While the check refuses, signup and recovery answer 503 and nothing is queued. A refused check runs again every 10 minutes, and the admin MCP tool `mail_status` with `refresh: true` runs it at once. `/_littlebox/health` reports the transport, whether it is sending, the queue depth and the kind of the last failure.

A job is `pending` until the provider takes it, then `accepted`, then `delivered` when the provider reports delivery to the recipient's server, or `failed`. A provider outage keeps jobs pending and retries up to nine attempts over about two hours, waiting 30 seconds at first and doubling up to an hour. A job whose link was replaced, used or expired before sending fails without being sent. Delivery says the recipient's server took the email, not that a person read it, and it never activates an account.

Each queued link is encrypted with AES-256-GCM under a key derived from `LITTLEBOX_MAIL_SECRET`, or from `LITTLEBOX_ADMIN_KEY` when that is unset, and is deleted once the provider accepts the email. Changing either secret leaves pending links unreadable; those jobs fail and people ask for a new link.

To record delivery, add a webhook in Resend that posts `email.sent`, `email.delivered`, `email.bounced`, `email.failed` and `email.suppressed` to `<LITTLEBOX_PUBLIC_ORIGIN>/_littlebox/mail/resend-events`, and set `LITTLEBOX_RESEND_WEBHOOK_SECRET` to its signing secret. The Kernel rejects an event without a valid signature, records each event ID once, and only moves a job forward.

The limits refuse a request with 429 and queue nothing. Each address takes at most 3 account emails a minute and 10 a day, one client address may ask for 20 an hour, the apps of one creator 100 a day, one app 100 a day and the installation 1,000 a day. Notices from a mutation's `notify` count apart, 200 a day per app, 500 per creator and 50 per address from one app, so a busy app never uses up the account email that signup and recovery need; every email counts toward the installation's 1,000. The address and client limits count requests, including those for addresses with no account, so a refusal never tells who has one. The `LITTLEBOX_MAIL_LIMIT_*` variables change them; keep the installation limit at or below what the provider allows.

`mail.sqlite` travels in scheduled copies, with its links still encrypted. A restore ends every pending job, because the restore invalidates the challenges their links carry. An app export does not include the queue.

The operator suspends or resumes a local creator account from the server:

```sh
littlebox-kernel creator-account-suspend owner@example.com
littlebox-kernel creator-account-resume owner@example.com
```

Suspension ends the creator's sessions, open links, MCP credentials and OAuth codes. Projects and app accounts stay. A resumed account is active if it ever verified, and pending otherwise. [docs/mcp.md](docs/mcp.md#verified-accounts) lists the routes and tools.

## Creator admission

A creator account is created with an invitation code the operator issues on the server. The installation setup key creates only the first owner.

Run these with the same `LITTLEBOX_DATA_DIR` as the Kernel; they work while it runs.

```sh
littlebox-kernel creator-code-create --expires-in 14d --note "For Ada"
littlebox-kernel creator-code-list
littlebox-kernel creator-code-revoke code_1a2b3c4d5e6f
```

`creator-code-create` prints the code once, with its ID and expiry. The Kernel stores only a SHA-256 hash of it in `owners.sqlite`. A code has 25 characters from Crockford's base32 alphabet, about 125 random bits; case, dashes and spaces do not matter. It expires after 14 days by default, and `--expires-in` takes hours or days from `1h` to `90d`. `creator-code-list` shows every code with its state (open, used, revoked or expired), its note and the creator it produced with that account's state. `creator-code-revoke` stops an open code; a used code cannot be revoked, so suspend its creator instead.

The person enters the code on the owner page or in **Platform access**. One transaction consumes the code and creates the account, so two people racing with one code produce one account. Under `LITTLEBOX_ACCOUNT_VERIFICATION=required` that account is pending until its email is verified, as in [Verified accounts](#verified-accounts); in `local` mode it is active at once. A code presented for an email whose account is active or suspended is consumed and creates nothing, with the same answer as a new email, so a code holder cannot test which creators exist. A signup that started from an MCP client's OAuth consent carries the consent path in the verification link and returns there after the password is set.

The setup key creates an account only while `owners.sqlite` holds no account and no code has ever been issued. A first owner whose verification link was lost can sign up again with the key for a new link, or use **Forgot your password?**. Once a second account exists or the first code is issued, the key creates no account, and revoking every code does not bring it back. A leaked key therefore does not open creator signup. One allowance keeps local development working: with the default `local-development-key`, which the Kernel accepts only on loopback unless `LITTLEBOX_ALLOW_DEFAULT_KEY=1`, the key keeps creating owners until the first code is issued. The owner page and **Platform access** ask for the key only while it can still create an account, and ask for a code otherwise.

## Security status

- Runtime requests are bound to the hostname-resolved Deployment; the browser cannot select another database ID.
- Private Deployments require a per-app login before serving the shell. The signup policy decides who can create that login.
- A file is private to its uploader unless it is made public, and only the uploader changes its visibility or deletes it. Uploads need an app session; a signed-out visitor of a public Deployment may read public files, and an API key reads only the visibilities it names.
- Public HTML is readable by anyone. Data is reachable only through declared actions, and a signed-out visitor can reach only the actions declared `anonymous`. An HTTP route answers only the callers it takes, and an action whose route does not answer anyone is not callable through the runtime endpoint.
- API keys are stored as hashes, name the actions and file visibilities they may use, and are refused by the runtime endpoint. Route signatures cover the method, path, query string, body, time and a nonce, and a replay is refused across restarts.
- Sessions are opaque; only token hashes are persisted. Cookies are `HttpOnly` and host-only. Under HTTPS the owner and app cookies carry the `__Host-` prefix, so a cookie an app sets with `Domain=<suffix>` never authenticates the editor or another app.
- Every app response carries a Content Security Policy. Scripts run only from the app's origin or as the exact inline scripts of each HTML page, matched by SHA-256 hash, so inline event handler attributes and foreign scripts do not run. Requests reach only the app's own origin. No host can be framed by another origin, and HSTS is sent on every host when the public origin is HTTPS. Administrative MCP and owner sign-in refuse a request whose `Origin` is an app host before they read any credential. [The architecture](./docs/architecture.md#isolation-of-hosted-code) lists the policy.
- Pending and suspended accounts get no session. Verification and recovery links are single use, expire, answer only their own purpose and scope, and are stored as hashes. Opening a link changes nothing until the person posts the form.
- Every Deployment gets a separate SQLite working file. The Kernel writes mutations transactionally and stores backups on explicit requests and before migrations.
- Action inputs are typed, parameterized and quota-limited; structural SQL is rejected from runtime actions.
- Owners use first-party sessions and delegated credentials. `LITTLEBOX_ADMIN_KEY` is an installation setup key that creates the first owner and never enters the editor bundle; later creators need an operator-issued invitation code. The Kernel binds to `127.0.0.1` by default and refuses a non-loopback bind with the default setup key. Remote installations must configure their public origin, TLS and setup key.
- The repository's two Docker images run as unprivileged users. The Dockerfile in an exported self-host kit still runs the Kernel as root.

## Export and self-host

Create either shared tasks or private notes from the project dashboard. The notes starter has a public landing page and actions that bind each row to the signed-in user. The Auth panel lets you simulate Guest, Alice and Bob in the preview; published apps use their own accounts.

Use Data to choose the local or deployed database, edit the model, review a generated migration, create backups and restore them. Publishing the same project keeps its URL and database, and the republished app stops serving files that the new artifact no longer contains. Local and deployed model acceptance are tracked separately.

Export project downloads a tar archive with source files, a built frontend, the Rust Kernel and startup scripts. For a published app it also includes the database, users and uploads. Imported users keep their password hashes, while sessions are cleared. The editor still builds this archive in the browser in the small format, so its combined state limit is 16 MiB and each file is limited to 2 MiB; the CLI below has no such limit. The archive documents two ways to run it: Docker, or native Cargo with Rust 1.91 or newer. Its `deploy.mjs` publishes the app as private unless the exported project manifest sets `visibility` to `"public"`.

External projects can export through the CLI too. A source-only kit needs no connection or credential. Add `--with-data` to include the published app's database, users and uploads through authenticated MCP. That needs a credential with the `export_data` permission. `login --export-data` creates one; the read, write and publish permissions do not include it.

```sh
bun run littlebox -- export --dir /tmp/my-app --out /tmp/my-app-selfhost.tar
LITTLEBOX_EMAIL=... LITTLEBOX_PASSWORD=... bun run littlebox -- login --dir /tmp/my-app --url http://localhost:4173 --export-data
bun run littlebox -- export --dir /tmp/my-app --out /tmp/my-app-with-data.tar --with-data
```

`--with-data` downloads the app as a package. It holds a manifest and one file each for the database, the accounts and the files, streamed in 64 KiB blocks, so an app at its 256 MiB database and its file quota exports whole. The kit's `deploy.mjs` uploads the package the same way and the target Kernel checks it before it creates the app. [The MCP guide](./docs/mcp.md#export-and-import-an-app) describes the format. The command refuses to overwrite an archive and writes it with owner-only permissions. Live-state exports retain the deployed visibility and signup policy. The target installs that policy with its first deployment. Exports with live state contain password hashes and attachments; keep them private. Extracted snapshots have owner-only permissions and the kit's Git rules exclude them.

## Migrate to another server

A migration moves a published app to another Littlebox Kernel with its accounts, passwords, rows, files, suspensions, policy, Usage and Signals, and leaves one Kernel taking its writes. It needs a credential with `manage_lifecycle` and `export_data` on the origin and `manage_lifecycle` on the destination.

1. Start the destination Kernel with its own `LITTLEBOX_PUBLIC_ORIGIN`, an owner and an email transport. Create a project with the same ID and set any route signing secrets the app needs.
2. Run `migration_begin` on the origin. The app shows a maintenance page and takes no writes, signups, sign-ins or email from then on, also after a restart. Download the package and the release.
3. Import them on the destination with `release_plan` and `release_apply`. The imported app stays in maintenance; check it with read tools.
4. Run `migration_confirm` on the origin, with the new address if it changed. The origin stays in maintenance for good and shows a link to the new address for seven days. Keep the confirmation it returns.
5. Run `migration_activate` on the destination with that confirmation, and `resendInvitations` to send pending invitations again from the new host. People sign in again with their passwords.

Until step 4, `migration_cancel` resumes the origin, and a package imported elsewhere can never be activated. After it, cancel is refused: to come back, migrate the destination's state here. [The MCP guide](./docs/mcp.md#migrate-an-app) lists the tools and refusals, and `bun run tests/smoke/migration.ts --port 19460` rehearses the whole procedure on two local Kernels.

## Eject

Eject hands a Managed project's code to its owner to edit outside Littlebox. The project becomes External; the hosted app keeps its URL, users, sessions, data, invitations and email service, and Eject exports no data. From then on the Kernel refuses edits to its saved sources from every tab and agent, and the owner publishes Build Artifacts with the CLI. Littlebox does not take a project back: to edit in the editor again, create a new project. [ADR-0005](./docs/adr/0005-build-artifacts-separate-managed-and-external-projects.md) records the decision.

In the editor, the project list's "Take it with you" group has an Eject action per project. It downloads the code as a tar, checks it, and only then ejects. The project then opens read-only with the commands to publish from outside.

From the CLI, eject writes the code into an empty folder and publishes from there. Eject needs the `manage_lifecycle` permission: `login --manage-lifecycle` asks for it, and the read, write, publish, manage_users and export_data permissions do not include it.

```sh
LITTLEBOX_EMAIL=... LITTLEBOX_PASSWORD=... bun run littlebox -- login --dir /tmp/ejected --url http://localhost:4173 --manage-lifecycle
bun run littlebox -- eject --dir /tmp/ejected --project <project id> --url http://localhost:4173
bun run littlebox -- publish --dir /tmp/ejected
```

`eject` plans at the revision the server last saved, or at `--revision`, downloads the package, checks every file's SHA-256, writes the folder and then applies, saving the receipt in `.littlebox/eject-receipt.json`. A stale revision is refused before anything downloads. After an interruption, run the same command again in the same folder. [The MCP guide](./docs/mcp.md#eject) lists the tools and refusals.

Eight runnable examples and their MCP replay are in [examples/product-fit](./examples/product-fit/README.md). The [product-fit assessment](./docs/research/product-fit.md) records tested workflows, remaining limits and the proposed next primitives.

Provider-free testing covers the editor and offline demo. Connect a provider in Agent settings to generate arbitrary apps with a model.

## Next slice

1. Run Littlebox on one real host: the production recipe with HTTPS, a real setup key, trusted proxies and S3 copies, and a README statement of what such an installation is for. These are deliveries 3 and 4 of the [self-use plan](./docs/plans/self-use-and-sdk-modules.md), and they wait on the operator.
2. Operate the beta: quotas shown to creators, alerts, account closing, production email and an end-to-end rehearsal. This is delivery K of the [launch plan](./docs/plans/platform-launch.md), which also waits on the operator.
3. Close the open findings of the [showcase apps](./docs/research/showcase-findings.md): a bridge from a role inside an app to the owner's user tools, per-action rate limits, allowed values for inputs, and a link from an invitation to its email job.

The launch plan defers Workspace membership and teams. Immutable release history and data-aware rollback have no plan yet.

Architecture and security boundaries are described in [docs/architecture.md](./docs/architecture.md).
