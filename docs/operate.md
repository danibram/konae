# Operate an installation

An installation is one Kernel with its data directory, and usually the editor in front of it. This page is for whoever operates one. It covers the settings the Kernel reads, where the data lives, how copies and restores work, how accounts are verified and how creators are admitted.

## The local stack

`bun run dev:local` starts a Kernel and the editor without Docker. The repository's [README](../README.md#run-it-locally) has the commands. The complete publishing stack, with the gateway and S3 storage, runs under Compose:

```bash
docker compose up --build
```

Open [http://app.localhost:8080](http://app.localhost:8080). Published apps receive URLs such as `http://tiny-tasks-a1b2c3.localhost:8080`; `*.localhost` resolves locally without editing `/etc/hosts` on modern browsers.

Compose publishes the gateway and the S3 API on `127.0.0.1` only. The Kernel container binds `0.0.0.0` inside the Compose network with the development setup key, so the Compose file sets `KONAE_ALLOW_DEFAULT_KEY=1`. Do not copy that setting to a reachable installation.

The local stack's S3 server is RustFS:

- Its API answers at [http://localhost:9000](http://localhost:9000) with the development credentials `konae` and `konae-secret`.
- `konae-apps` receives a second copy of each static artifact.
- `konae-data` receives scheduled copies of the data directory when `KONAE_COPY_S3_PREFIX` is set.

The Kernel volume stays the authority. Every app's record, artifact, database, accounts, uploads and backups live there, and mutations run against the live database. A Kernel started on an empty volume serves nothing until `konae restore` installs a copy. See [Data directory and copies](#data-directory-and-copies).

If you ran the stack before the Kernel image became non-root, fix the volume once: `docker compose run --rm --user root --entrypoint chown kernel -R 10001:10001 /data`.

The S3 buckets must already exist. The same S3 settings work with RustFS, MinIO, Cloudflare R2, Scaleway Object Storage and Hetzner Object Storage through their S3 endpoints.

### Two binaries

`kernel-rs` builds two binaries. `konae` is the installation: the Kernel with owner accounts, the owner's MCP, OAuth and the project list. `Dockerfile.konae`, `bun run kernel` and `bun run dev:local` run it, and every command on this page is a `konae` command. `kernel` is the Kernel alone: it serves the app folders under `apps/` of its data directory, answers `restore`, `reconcile`, `files-migrate` and `files-reconcile`, and reads no owner, credential or project list.

At startup `kernel` opens each folder under `apps/` as a release. It runs the checks a publish runs on `konae.json`, `app/` and `dist/`, brings `data/database.sqlite` to `app/schema.sql`, writes its record in the folder's `.konae/`, and prints `Serving <name> at <url>`. A folder that fails a check is not served, and the Kernel prints why and serves the others. A schema change that loses data needs `KONAE_CONFIRM_SCHEMA` set to the confirmation it prints. A folder without a record answers at `<slug>.<KONAE_HOST_SUFFIX>`, or at a random address when another app holds that one. While it serves, `kernel` reads an app's `konae.json` again when it changes.

`kernel` also answers the commands an app needs without Konae: `users`, `invite`, `suspend`, `resume`, `signup-policy`, `api-key`, `secret`, `sql`, `events`, `maintenance`, `enter` and `import`. Each calls the function Konae's owner tool calls and prints its answer as JSON. They run beside a serving Kernel, except `import`, which replaces the app's data with the `data/` of an exported folder after a backup and runs while no Kernel serves the app. `kernel help` lists them with their options, and so does an exported folder's `AGENTS.md`.

`Dockerfile.kernel` builds an image that holds only `kernel`, and an exported folder's `compose.yaml` runs it on the folder, named by `KONAE_KERNEL_IMAGE` in the folder's `.env`. The image runs the Kernel and each command as the owner of the app folder it finds under `/kernel/apps`, so the files it writes there stay that person's. Konae publishes no image; build it with `docker build -f Dockerfile.kernel -t <registry>/<name>:<version> .` and push it where the server can pull it.

## Before a Kernel is reachable

The defaults suit one machine. The Kernel refuses to start on a non-loopback address with the default setup key, and the rest of this list is the operator's to set.

- Set `KONAE_ADMIN_KEY` to a long random value. It creates the first owner and nothing else.
- Set `KONAE_DOMAIN` to the installation's domain, such as `example.com`. The editor and MCP then answer at `app.example.com`, each app at `<slug>-<id>.example.com`, and path addresses at `example.com/p/<name>`.
- Put a proxy in front that terminates TLS for the domain, for `*.<domain>` and for every custom domain, and forwards `Host` unchanged. Konae issues no certificate. `ops/nginx.local.conf` is the local stack's proxy, without TLS.
- List the proxy's address in `KONAE_TRUSTED_PROXIES`, or every visitor shares one rate-limit bucket.
- Set `KONAE_ACCOUNT_VERIFICATION=required` and a mail transport, so nobody holds an account for an email they did not prove. Set `KONAE_MAIL_SECRET` as well.
- Set `KONAE_COPY_DIR` or `KONAE_COPY_S3_PREFIX`, with `KONAE_COPY_SECRET` for S3. The data directory is the only place the apps live.
- To serve an app at the domain's root, turn on its path address with the name `konae-home`. Without an app on that name, the root answers not found. [`examples/home`](../examples/home/README.md) is a home ready to publish.

## Environment variables

The Kernel reads these at startup. Defaults are the values in `kernel-rs/kernel/src` and `kernel-rs/konae/src` unless a column says otherwise.

| Variable | Default | Meaning |
| --- | --- | --- |
| `KONAE_KERNEL_PORT` | `8787` (`8790` under `bun run dev:local`) | Port the Kernel listens on. `dev:local` also uses it to build the editor's proxy target. |
| `KONAE_BIND` | `127.0.0.1` (`0.0.0.0` in the Kernel image) | Address the Kernel listens on. |
| `KONAE_ADMIN_KEY` | `local-development-key` | Installation setup key that creates the first owner. Later creators need an invitation code; see [Creator admission](#creator-admission). An empty value counts as unset. |
| `KONAE_ALLOW_DEFAULT_KEY` | unset | Set to `1` to let the Kernel start on a non-loopback address with the default setup key. Compose sets it for the development stack. |
| `KONAE_ACCOUNT_VERIFICATION` | `local` | `required` makes new creator and app accounts prove their email before they are active, and moves accounts that never verified to pending with their sessions revoked. `local` keeps unverified signup for development. See [Verified accounts](#verified-accounts). |
| `KONAE_MAIL_TRANSPORT` | unset | How the Kernel sends account links: `resend`, `smtp` or `development`. Every transport sends through the queue in `mail.sqlite`. `development` appends each message to `mailbox.jsonl` in the data directory. Unset means no email: verification-required signup and recovery answer 503. See [Account email](#account-email). |
| `KONAE_MAIL_FROM` | `Konae <konae@localhost>` for `development`; required otherwise | Sender address, such as `Konae <no-reply@mail.example.com>`. Its domain is the one the preflight checks. |
| `KONAE_MAIL_SECRET` | derived from `KONAE_ADMIN_KEY` | Secret, at least 32 characters, from which the Kernel derives the key that encrypts queued links. When unset, Konae derives the key from `KONAE_ADMIN_KEY` and refuses to start `resend` or `smtp` if that is still `local-development-key`. The `kernel` binary alone has no setup key: its first start writes a random key to `mail-key` in the data directory, readable by its owner alone, and every later start and command reads it. |
| `KONAE_RESEND_API_KEY` | unset | Resend API key for `resend`. A sending-only key is enough to send. |
| `KONAE_RESEND_DOMAINS_KEY` | `KONAE_RESEND_API_KEY` | Resend key that can list domains, for the sender preflight. Set it when the sending key has `sending_access` only. |
| `KONAE_RESEND_WEBHOOK_SECRET` | unset | The `whsec_` signing secret of the Resend webhook that posts to `/_konae/mail/resend-events`. Unset means the route answers 404 and jobs stop at accepted. |
| `KONAE_RESEND_API_URL` | `https://api.resend.com` | Resend API base URL. Tests point it at a local server. |
| `KONAE_SMTP_HOST` | unset | SMTP server for `smtp`. |
| `KONAE_SMTP_PORT` | `587`, or `465` with implicit TLS | SMTP port. |
| `KONAE_SMTP_TLS` | `starttls` | `starttls` requires STARTTLS on a plain connection; `implicit` uses TLS from the first byte. `none` is accepted only for a loopback host. |
| `KONAE_SMTP_USERNAME`, `KONAE_SMTP_PASSWORD` | unset | SMTP credentials. Set both or neither. |
| `KONAE_MAIL_LIMIT_RECIPIENT_PER_MINUTE` | `3` | Account email requests one address may receive per minute. |
| `KONAE_MAIL_LIMIT_RECIPIENT_PER_DAY` | `10` | Account email requests one address may receive per 24 hours. |
| `KONAE_MAIL_LIMIT_CREATOR_PER_DAY` | `100` | Account emails the apps of one creator may queue per 24 hours. |
| `KONAE_MAIL_LIMIT_APP_PER_DAY` | `100` | Account emails one Deployment may queue per 24 hours. |
| `KONAE_MAIL_LIMIT_INSTALLATION_PER_DAY` | `1000` | Emails of every kind, account email and notices, the installation may queue per 24 hours. Keep it at or below the provider's contracted limit. |
| `KONAE_MAIL_LIMIT_CLIENT_PER_HOUR` | `20` | Account email requests one client address may make per hour. |
| `KONAE_MAIL_LIMIT_NOTIFY_APP_PER_DAY` | `200` | Notices (`notify` on a mutation) one Deployment may queue per 24 hours. |
| `KONAE_MAIL_LIMIT_NOTIFY_CREATOR_PER_DAY` | `500` | Notices the apps of one creator may queue per 24 hours. |
| `KONAE_MAIL_LIMIT_NOTIFY_RECIPIENT_PER_DAY` | `50` | Notices one address may receive from one Deployment per 24 hours. |
| `KONAE_MAIL_WORKER_INTERVAL_MS` | `5000` | How often the worker looks for due jobs. A new job also wakes it. |
| `KONAE_CALLS_PER_MINUTE` | `20` | Calls one app's call actions may queue in any 60 seconds, from 1 to 10000. The Kernel sends them from `calls.sqlite`; see [docs/mcp.md](./mcp.md#calls-between-apps). |
| `KONAE_FILES_QUOTA_BYTES` | `20971520` (20 MiB) | Bytes the files of one Deployment may hold together. An upload past it is refused with the configured number, and so is a package whose files exceed it. |
| `KONAE_FILE_MAX_BYTES` | `2097152` (2 MiB) | Bytes one file may hold. It must not exceed `KONAE_FILES_QUOTA_BYTES`; the Kernel refuses to start otherwise. Uploads and reads still move in 64 KiB MCP blocks, so a larger file takes more calls. |
| `KONAE_FILES_S3_BUCKET` | unset | Bucket that holds the bytes of app files, as `<prefix><deployment id>/<file id>`. Unset keeps them in each Deployment's `files/` folder. The index stays in the folder either way. See [File bytes in a bucket](#file-bytes-in-a-bucket). |
| `KONAE_FILES_S3_ENDPOINT`, `KONAE_FILES_S3_ACCESS_KEY`, `KONAE_FILES_S3_SECRET_KEY` | unset | Required with `KONAE_FILES_S3_BUCKET`. They are separate from `KONAE_S3_*`, so the private file bytes can use another bucket and key than the artifacts. |
| `KONAE_FILES_S3_REGION` | `us-east-1` | Region of the file bucket. R2 uses `auto`. |
| `KONAE_FILES_S3_PREFIX` | empty | Prefix inside the file bucket, such as `files/`. |
| `KONAE_FILES_S3_PATH_STYLE` | `1` | `1` addresses the bucket as `<endpoint>/<bucket>`, `0` as `<bucket>.<endpoint host>`. |
| `KONAE_TRUSTED_PROXIES` | unset | Comma-separated proxy IP addresses whose `X-Forwarded-For` header the Kernel trusts for rate limiting. CIDR ranges are rejected. |
| `KONAE_DATA_DIR` | `/data` (`data/kernel-local` under `bun run dev:local` and `bun run kernel`) | Data directory: one app folder per project under `apps/<id>/`, plus `owners.sqlite`, `mcp.sqlite`, `mail.sqlite`, `calls.sqlite` and `transfers.sqlite`. |
| `KONAE_DOMAIN` | unset | Derives the installation's hosts from one domain (ADR-0014). The public and MCP origin become `<scheme>://app.<domain>` and the app suffix becomes `<domain>`. Each of `KONAE_PUBLIC_ORIGIN`, `KONAE_MCP_ORIGIN` and `KONAE_HOST_SUFFIX` still wins when it is set. The Kernel refuses to start when `KONAE_PUBLIC_ORIGIN` or `KONAE_MCP_ORIGIN` has the bare domain as its host, because the home and path addresses answer there. The local stack sets `localhost` with port `8080`. |
| `KONAE_SCHEME` | `http` for `localhost`, `https` otherwise | Scheme of the derived origins, `http` or `https`. Read only with `KONAE_DOMAIN`. Any other value stops the Kernel. |
| `KONAE_PUBLIC_PORT` | unset | Port appended to the derived origins, such as `8080`. Read only with `KONAE_DOMAIN`. A value that is not a port number stops the Kernel. |
| `KONAE_PUBLIC_ORIGIN` | `http://localhost:8080` (`http://localhost:<kernel port>` under `dev:local`), or `<scheme>://app.<domain>` with `KONAE_DOMAIN` | Origin used to build app URLs. Its scheme and port are copied onto each app hostname, and an `https://` origin makes OAuth metadata use HTTPS. |
| `KONAE_HOST_SUFFIX` | `localhost`, or `<domain>` with `KONAE_DOMAIN` | Domain appended to new app hostnames, as in `<slug>-<id>.localhost`. |
| `KONAE_MCP_ORIGIN` | unset, or the public origin with `KONAE_DOMAIN` | Public origin of the MCP endpoint, without a path, when it differs from the hosting origin. OAuth discovery advertises it. |
| `KONAE_DNS_RESOLVER` | the first `nameserver` in `/etc/resolv.conf` | The resolver, as an IP address with an optional port, that the Kernel asks for a custom domain's TXT record `_konae.<hostname>` when `domain_set` runs (ADR-0014). |
| `KONAE_S3_ENDPOINT` | unset | S3 endpoint URL. The Kernel always serves artifacts from its data directory; when this is set it also uploads each artifact to `KONAE_ARTIFACT_BUCKET` and can send copies to `KONAE_DATA_BUCKET`. When unset, the Kernel ignores the other S3 variables. |
| `KONAE_S3_REGION` | `us-east-1` | S3 region. |
| `KONAE_S3_ACCESS_KEY` | `konae` | S3 access key. |
| `KONAE_S3_SECRET_KEY` | `konae-secret` | S3 secret key. |
| `KONAE_ARTIFACT_BUCKET` | `konae-apps` | Bucket for static Build Artifacts. |
| `KONAE_DATA_BUCKET` | `konae-data` | Bucket that receives copies when `KONAE_COPY_S3_PREFIX` is set. |
| `KONAE_COPY_DIR` | unset | Directory, on another disk, that receives scheduled copies of the data directory. It must be outside `KONAE_DATA_DIR`. |
| `KONAE_COPY_S3_PREFIX` | unset | Prefix in `KONAE_DATA_BUCKET`, such as `copies/`, that receives scheduled copies. Needs `KONAE_S3_ENDPOINT` and must not start with `deployments/`. |
| `KONAE_COPY_INTERVAL_SECONDS` | `3600` | Seconds between copies when a copy target is set. The first copy runs at startup. |
| `KONAE_SCHEDULE_MIN_SECONDS` | unset | For tests only. Runs every scheduled action once every this many seconds, 1 to 300, instead of at its declared times, so a test can watch a run. The Kernel refuses to start with it unless `KONAE_BIND` is a loopback address. |
| `KONAE_COPY_SECRET` | unset | Secret, at least 32 characters, that encrypts every file of a copy sent to S3 with AES-256-GCM. `restore` needs the same secret to open such a copy. Directory copies stay unencrypted. |
| `KONAE_CLERK_ISSUER`, `KONAE_CLERK_JWKS_FILE`, `KONAE_CLERK_AUTHORIZED_ORIGINS`, `KONAE_CLERK_BETA_SUBJECTS`, `KONAE_CLERK_PUBLISHABLE_KEY` | unset | The optional hosted creator sign-in adapter. Unset keeps it off. [Hosted creator access](./clerk-creator-access.md) describes each. |
| `KONAE_GOOGLE_CLIENT_ID`, `KONAE_GOOGLE_CLIENT_SECRET` | unset | No longer read. A Kernel that finds either set prints a warning and starts. Each app's owner sets the app's own Google client in Auth > Users or with `secret_set` and `use: "google"`. |
| `KONAE_GOOGLE_TEST_ISSUER` | unset | Points Google sign-in at a local fake issuer (`<base>/authorize`, `/token`, `/jwks`) for development; `bun run scripts/fake-google.ts` is one. Read only by a development build with a local HTTP public origin; a release build refuses to start with it set. Google sign-in has no other installation setting: each app's owner sets the app's own Google client in Auth > Users. |

The Kernel refuses to start when `KONAE_ADMIN_KEY` is unset or equal to `local-development-key` and either `KONAE_BIND` is not a loopback address (`localhost`, `127.0.0.1`, `::1` or another loopback IP) or the host in `KONAE_PUBLIC_ORIGIN` or in `KONAE_MCP_ORIGIN`, when set, is not `localhost`, a `*.localhost` name or a loopback IP, unless `KONAE_ALLOW_DEFAULT_KEY=1`. Set a long random setup key before exposing a Kernel.

The editor, the `konae` CLI and the test configurations read these:

| Variable | Read by | Default | Meaning |
| --- | --- | --- | --- |
| `KONAE_EDITOR_PORT` | `dev:local` | `4173` | Port of the Vite editor. |
| `KONAE_KERNEL_URL` | `dev:local`, `vite.config.ts` | `http://127.0.0.1:<kernel port>` under `dev:local`, `http://127.0.0.1:8787` for plain `bun run dev` | Kernel the editor proxies `/mcp`, `/_konae`, `/oauth` and `/.well-known` to. |
| `KONAE_MCP_TOKEN` | `konae` CLI | unset | Bearer token for `init`, `publish`, `domain` and `export --with-data`. When unset, the CLI reads `.konae/credentials.json`. |
| `KONAE_KERNEL_BIN` | `konae dev`, `sql`, `events` | unset | A `kernel` binary built from this checkout. `dev` starts it instead of compiling one with `cargo`. |
| `KONAE_URL` | `konae login`, `konae init`, `konae domain` | `http://localhost:4173`; unset for `init` | Konae installation to talk to. `--url` overrides it. `init` writes it into `konae.json` only when it is set. |
| `KONAE_EMAIL`, `KONAE_PASSWORD` | `konae login` | unset | Owner credentials exchanged for a 24-hour MCP token. |
| `KONAE_SETUP_KEY` | `konae login` | unset | Installation setup key, needed only to create an installation's first owner. |
| `KONAE_INVITATION_CODE` | `konae login` | unset | Invitation code that creates the account when it does not exist yet. Under required verification, add `--verify-from <data folder>/mailbox.jsonl` on a development Kernel to answer the emailed link in the same run. |
| `KONAE_EDITOR_URL` | `playwright.local.config.ts` | `http://127.0.0.1:4176` | Editor the local end-to-end suite opens. When set outside CI, the suite reuses a server that is already running. |
| `KONAE_TEST_KERNEL_PORT` | `playwright.local.config.ts` | `8794` | Kernel port for the local end-to-end suite. |
| `KONAE_PORT` | an exported app folder's `compose.yaml` | `8787` | Port the folder's Kernel publishes, read from its `.env`. |
| `KONAE_KERNEL_IMAGE` | an exported app folder's `compose.yaml` | unset | The image `Dockerfile.kernel` builds, as published to a registry. Compose refuses to start without it. |

`konae dev` ignores every `KONAE_` and `VITE_` variable from your shell. It runs the `kernel` binary on `127.0.0.1` with a generated key, a data directory at `.konae/kernel`, and the folder's own `data/`. It sets a local value for each signing secret `actions.json` names and prints it, and it serves a page of the email, calls, scheduled runs and Signals the app produced on the port after the app's, on this machine only.

## Data directory and copies

Each app is one folder in the Kernel's data directory. A copy of that folder, with the owner and MCP stores beside it, is the app.

```text
data/
├── owners.sqlite           # creator accounts, sessions and invitation codes
├── mcp.sqlite              # projects and their save counts, agent credentials, audit
├── mail.sqlite             # the account email queue
├── calls.sqlite            # the call actions' queue and outbox
├── transfers.sqlite        # unfinished MCP transfers and the receipts of file uploads
├── transfers/              # the bytes of unfinished MCP uploads and downloads
└── apps/<id>/              # one app folder (ADR-0001)
    ├── konae.json          # name, visibility, Signup Policy, sign-in methods, contact email
    ├── app/                # the saved sources
    ├── dist/               # the static files the Deployment serves
    ├── data/               # database.sqlite, identity.sqlite, usage.sqlite, files/, secrets.sqlite
    └── .konae/             # the Deployment record, address markers, maintenance and Data panel backups
```

[The architecture](./architecture.md#current-self-hosted-topology) lists the files that appear only in maintenance, after a restore or with the development mailbox.

Set `KONAE_COPY_DIR`, `KONAE_COPY_S3_PREFIX` or both to copy the data directory off the volume, by default every hour. Each copy takes every Deployment's data and identity locks in turn, snapshots its SQLite files with `VACUUM INTO`, copies its folder, then writes `manifest.json` with the size and SHA-256 of every file. A directory copy appears under its final name only when complete; an S3 copy is complete once its manifest is uploaded. The Kernel keeps the 24 newest copies and the newest copy of each day for 30 days on each target. The copy is staged under `.copy-staging/` in the data directory, so the volume needs room for one more copy.

A failed copy never stops the Kernel from serving. It is logged, and `/_konae/health` reports it under `copies`: each target's `lastAttemptAt`, `lastSuccessAt`, `lastCopyId` and `lastError`, and an `alert` message while any target's last attempt failed. `copies.targets` is empty when no target is set.

### File bytes in a bucket

With `KONAE_FILES_S3_BUCKET` set, the bytes of app files live in that bucket and each Deployment's `files/index.json` stays in its folder, which still decides which files exist and who may read them ([ADR-0009](./adr/0009-file-bytes-go-through-one-store-and-the-index-stays-with-the-app.md)). The Kernel sends only `PutObject`, `GetObject` with a range, `DeleteObject` and `ListObjectsV2`, and every byte still passes through it; the browser never gets a bucket URL. It has been tested against SeaweedFS, not yet against R2, Scaleway or Hetzner.

`files-store.json` in the data directory records which store holds the bytes, and a Kernel whose settings name another store refuses to start. To move the bytes, stop the Kernel and run, with the `KONAE_FILES_S3_` settings set in both directions:

```sh
KONAE_DATA_DIR=/data konae files-migrate --to s3
KONAE_DATA_DIR=/data konae files-migrate --to local
```

It copies every indexed file, reads each back and compares its SHA-256, records the new store, and only then removes the old copies. A missing file or a mismatch stops it before anything switches. An upload writes its bytes before its index entry and a delete removes the entry first, so a crash can leave bytes no index names. `konae files-reconcile` removes those once they are an hour old, and `--dry-run` only counts them. A serving Kernel holds `kernel.lock` in the data directory, so both commands refuse while it runs, and a Kernel started during a migration refuses to start.

A copy carries the file bytes, read from the bucket after the Deployment's locks are released, so a restore needs no bucket and lands the bytes in each folder. Run `files-migrate --to s3` after a restore to put them back in the bucket.

To move to a new volume or machine, restore into an empty data directory, then start the Kernel:

```sh
KONAE_DATA_DIR=/data konae restore --from /mnt/copies
KONAE_DATA_DIR=/data konae restore --from s3://konae-data/copies/ --copy 2026-11-06T14-00-00.000Z
```

Restoring from `s3://` reads `KONAE_S3_ENDPOINT`, `KONAE_S3_REGION`, `KONAE_S3_ACCESS_KEY` and `KONAE_S3_SECRET_KEY` from the same shell, and `KONAE_COPY_SECRET` when the copies are encrypted. Without `--copy`, restore takes the newest complete copy. It refuses a data directory that holds anything besides `lost+found`, a copy without a manifest, any file whose size or checksum differs from the manifest, and an encrypted copy without its secret or with another one, and then installs nothing. It also runs SQLite's integrity check on every database. After installing, it signs everyone out: owner and app sessions, Clerk creator sessions, agent credentials, OAuth tokens and codes, open verification, recovery and invitation links, and unfinished transfers and release plans. Owners create new agent credentials after a restore.

A copy can be up to an hour old, so each copy target also keeps a record of what was revoked since: every app account suspension, creator suspension and API key revocation is written to `revocations/` on each target before the tool reports it done. Restore applies that record before installing, so a restored installation never brings back a suspended account or a revoked key. If the record cannot be read, because the target is unreachable or the source has no `revocations/started.json`, the restore still installs but the installation starts in maintenance: every app shows its maintenance page and owner changes are refused. Then either apply the record once it is reachable, or reopen without it after checking suspensions and keys by hand:

```sh
KONAE_DATA_DIR=/data konae reconcile --from /mnt/copies
KONAE_DATA_DIR=/data konae reconcile --without-record
```

Run `reconcile` with the Kernel stopped, or start it again afterwards: a running Kernel keeps the maintenance it read at startup.

A Deployment in maintenance stays so after a restore, because its maintenance state travels in the copy.

## Verified accounts

Creator accounts and app accounts share one account core in the Kernel. An account is pending, active or suspended, and only an active account gets a session. Every session check reads the state, so suspension ends the sessions an account already holds.

`KONAE_ACCOUNT_VERIFICATION` chooses the mode for the whole installation:

- `local`, the default: signup creates an active, unverified account and signs it in, as before. This keeps local development working without email.
- `required`: signup creates a pending account with no password and sends a verification link. The person opens the link, chooses the password and becomes active. Recovery sends a 30-minute link that sets a new password and ends every earlier session.

Switching an installation to `required` moves every account that never verified, in both scopes, to pending and revokes its sessions. IDs, rows and file owners stay. Those people enter again through **Forgot your password?**, which sends them a verification link. The switch happens the first time the Kernel opens each identity database in the new mode, so an installation never reaches production with active accounts whose email nobody proved. Switching back to `local` and later to `required` again moves accounts created unverified in between.

`required` needs a mail transport. Without one, signup and recovery answer 503 with an explanation, and active accounts still sign in. `KONAE_MAIL_TRANSPORT=development` writes each link to `mailbox.jsonl` in the data directory with `0600` permissions; that file holds live links, so use it only on a machine you control.

## Account email

Signup, recovery and invitation links leave through a queue in `mail.sqlite` in the data directory. The Kernel stores the job in the same transaction as the link's challenge, so a request either stores both or neither. A worker inside the Kernel sends due jobs every few seconds and whenever a job arrives; a request never waits for the provider.

Choose a transport with `KONAE_MAIL_TRANSPORT`:

- `resend` sends through Resend's HTTP API. Set `KONAE_RESEND_API_KEY` and `KONAE_MAIL_FROM` with an address on a domain verified in Resend. Each request carries the job ID as its `Idempotency-Key`, so a retry after a restart is the same email to Resend.
- `smtp` sends through any SMTP server with STARTTLS (the default) or implicit TLS. Set `KONAE_SMTP_HOST`, the credentials and `KONAE_MAIL_FROM`. SMTP has no idempotency key: a Kernel stopped between the server's answer and the queue's record sends that one email twice.
- `development` writes to `mailbox.jsonl`, as above.

Before it sends anything, the Kernel checks the sender. For Resend the sender's domain must be `verified` in the account; a key with `sending_access` cannot read domains, so set `KONAE_RESEND_DOMAINS_KEY` to a full-access key or the check refuses. For SMTP the check connects, negotiates TLS and authenticates; whether the server lets that account send as the sender address is the provider's rule, and a refusal there shows as failed jobs. While the check refuses, signup and recovery answer 503 and nothing is queued. A refused check runs again every 10 minutes, and the admin MCP tool `mail_status` with `refresh: true` runs it at once. `/_konae/health` reports the transport, whether it is sending, the queue depth and the kind of the last failure.

A job is `pending` until the provider takes it, then `accepted`, then `delivered` when the provider reports delivery to the recipient's server, or `failed`. A provider outage keeps jobs pending and retries up to nine attempts over about two hours, waiting 30 seconds at first and doubling up to an hour. A job whose link was replaced, used or expired before sending fails without being sent. Delivery says the recipient's server took the email, not that a person read it, and it never activates an account.

Each queued link is encrypted with AES-256-GCM under a key derived from `KONAE_MAIL_SECRET`, or when that is unset from `KONAE_ADMIN_KEY` on Konae and from `mail-key` on the `kernel` binary, and is deleted once the provider accepts the email. Changing either secret leaves pending links unreadable; those jobs fail and people ask for a new link.

To record delivery, add a webhook in Resend that posts `email.sent`, `email.delivered`, `email.bounced`, `email.failed` and `email.suppressed` to `<KONAE_PUBLIC_ORIGIN>/_konae/mail/resend-events`, and set `KONAE_RESEND_WEBHOOK_SECRET` to its signing secret. The Kernel rejects an event without a valid signature, records each event ID once, and only moves a job forward.

The limits refuse a request with 429 and queue nothing. Each address takes at most 3 account emails a minute and 10 a day, one client address may ask for 20 an hour, the apps of one creator 100 a day, one app 100 a day and the installation 1,000 a day. Notices from a mutation's `notify` count apart, 200 a day per app, 500 per creator and 50 per address from one app, so a busy app never uses up the account email that signup and recovery need; every email counts toward the installation's 1,000. The address and client limits count requests, including those for addresses with no account, so a refusal never tells who has one. The `KONAE_MAIL_LIMIT_*` variables change them; keep the installation limit at or below what the provider allows.

`mail.sqlite` travels in scheduled copies, with its links still encrypted. A restore ends every pending job, because the restore invalidates the challenges their links carry. An app export does not include the queue.

The operator suspends or resumes a local creator account from the server:

```sh
konae creator-account-suspend owner@example.com
konae creator-account-resume owner@example.com
```

Suspension ends the creator's sessions, open links, MCP credentials and OAuth codes. Projects and app accounts stay. A resumed account is active if it ever verified, and pending otherwise. [docs/mcp.md](./mcp.md#verified-accounts) lists the routes and tools.

## Creator admission

A creator account is created with an invitation code the operator issues on the server. The installation setup key creates only the first owner.

Run these with the same `KONAE_DATA_DIR` as the Kernel; they work while it runs.

```sh
konae creator-code-create --expires-in 14d --note "For Ada"
konae creator-code-list
konae creator-code-revoke code_1a2b3c4d5e6f
```

`creator-code-create` prints the code once, with its ID and expiry. The Kernel stores only a SHA-256 hash of it in `owners.sqlite`. A code has 25 characters from Crockford's base32 alphabet, about 125 random bits; case, dashes and spaces do not matter. It expires after 14 days by default, and `--expires-in` takes hours or days from `1h` to `90d`. `creator-code-list` shows every code with its state (open, used, revoked or expired), its note and the creator it produced with that account's state. `creator-code-revoke` stops an open code; a used code cannot be revoked, so suspend its creator instead.

The person enters the code on the owner page or in **Platform access**. One transaction consumes the code and creates the account, so two people racing with one code produce one account. Under `KONAE_ACCOUNT_VERIFICATION=required` that account is pending until its email is verified, as in [Verified accounts](#verified-accounts); in `local` mode it is active at once. A code presented for an email whose account is active or suspended is consumed and creates nothing, with the same answer as a new email, so a code holder cannot test which creators exist. A signup that started from an MCP client's OAuth consent carries the consent path in the verification link and returns there after the password is set.

The setup key creates an account only while `owners.sqlite` holds no account and no code has ever been issued. A first owner whose verification link was lost can sign up again with the key for a new link, or use **Forgot your password?**. Once a second account exists or the first code is issued, the key creates no account, and revoking every code does not bring it back. A leaked key therefore does not open creator signup. One allowance keeps local development working: with the default `local-development-key`, which the Kernel accepts only on loopback unless `KONAE_ALLOW_DEFAULT_KEY=1`, the key keeps creating owners until the first code is issued. The owner page and **Platform access** ask for the key only while it can still create an account, and ask for a code otherwise.

## Rate limits and trusted proxies

Signed-out visitors may call anonymous actions 30 times per 10 seconds per client address and Deployment; further calls return `rate-limited` until the bucket refills. Each signed-in app user may call actions 120 times per 10 seconds. IPv6 clients share one bucket per /64. App login and signup allow 10 attempts per minute per client address and 5 per minute for one email address from one client address, counted separately for each Deployment. After 100 wrong passwords for one email in an hour, logins for that email wait, correct ones included. Each client address adds at most 20 failures per hour to that count, so it takes at least five addresses to pause an account. Owner login and signup have the same limits. Further attempts get HTTP 429 with `Retry-After`. The Kernel counts the socket peer address. It reads `X-Forwarded-For` only when the peer is listed in `KONAE_TRUSTED_PROXIES` (comma-separated IP addresses, unset by default), and then takes the rightmost entry that is not itself a listed proxy. A trusted proxy must append the address it received the request from to `X-Forwarded-For`, never pass the client's header through unchanged, or clients choose their own bucket. The bundled Nginx configuration appends it with `$proxy_add_x_forwarded_for`; list the Nginx address in `KONAE_TRUSTED_PROXIES` to count visitors separately behind it.

Each app database is capped at 256 MiB and each identity database at 64 MiB. Writes past that fail with "database or disk is full".

## Security status

- Runtime requests are bound to the hostname-resolved Deployment; the browser cannot select another database ID.
- Private Deployments require a per-app login before serving the shell. The signup policy decides who can create that login.
- A file is private to its uploader unless it is made public, and only the uploader changes its visibility or deletes it. Uploads need an app session; a signed-out visitor of a public Deployment may read public files, and an API key reads only the visibilities it names.
- Public HTML is readable by anyone. Data is reachable only through declared actions, and a signed-out visitor can reach only the actions declared `anonymous`. An HTTP route answers only the callers it takes, and an action whose route does not answer anyone is not callable through the runtime endpoint.
- API keys are stored as hashes, name the actions and file visibilities they may use, and are refused by the runtime endpoint. Route signatures cover the method, path, query string, body, time and a nonce, and a replay is refused across restarts.
- Sessions are opaque; only token hashes are persisted. Cookies are `HttpOnly` and host-only. Under HTTPS the owner and app cookies carry the `__Host-` prefix, so a cookie an app sets with `Domain=<suffix>` never authenticates the editor or another app.
- Every app response carries a Content Security Policy. Scripts run only from the app's origin or as the exact inline scripts of each HTML page, matched by SHA-256 hash, so inline event handler attributes and foreign scripts do not run. Requests reach only the app's own origin. No host can be framed by another origin, and HSTS is sent on every host when the public origin is HTTPS. Administrative MCP and owner sign-in refuse a request whose `Origin` is an app host before they read any credential. [The architecture](./architecture.md#isolation-of-hosted-code) lists the policy.
- Pending and suspended accounts get no session. Verification and recovery links are single use, expire, answer only their own purpose and scope, and are stored as hashes. Opening a link changes nothing until the person posts the form.
- Every Deployment gets a separate SQLite working file. The Kernel writes mutations transactionally and stores backups on explicit requests and before migrations.
- Action inputs are typed, parameterized and quota-limited; structural SQL is rejected from runtime actions.
- Owners use first-party sessions and delegated credentials. `KONAE_ADMIN_KEY` is an installation setup key that creates the first owner and never enters the editor bundle; later creators need an operator-issued invitation code. The Kernel binds to `127.0.0.1` by default and refuses a non-loopback bind with the default setup key. Remote installations must configure their public origin, TLS and setup key.
- The repository's two Docker images run as unprivileged users.
