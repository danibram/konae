# Littlebox

An open-source workshop for making tiny useful apps: edit code like CodePen, ask an agent for changes, and deploy static apps with SQLite, auth and files already attached.

> **Status:** self-hosted vertical slice. “Littlebox” is a working name.

## What works today

- Workspace home with search, project creation, visibility state and direct access to either the editor or latest deployed app.
- Automatic migration of the original single-project browser state into a multi-project workspace index.
- A project tree with nested files and empty folders. `app.js` or `src/main.ts` is the entrypoint, and relative JavaScript, TypeScript, JSX, JSON and CSS imports compile in the browser.
- Sandboxed iframe preview with a small `miniapp` SDK.
- Real SQLite in the browser through `sql.js`, persisted locally and exportable as a `.sqlite` file.
- Per-app signup, login, logout and public or login-required app pages.
- Local file uploads, persisted as data URLs for the prototype.
- Code and data persist through page reloads.
- Provider-independent Kernel policy for public/private shells, declared actions and owner-only administration.
- Opaque Kernel sessions with hashed tokens, expiry and revocation.
- A real in-browser agent on `pi-agent-core`: a streaming tool loop over a disposable copy of the project tree, bring-your-own provider keys kept in the browser, and a Validation Gate every proposal must pass before it reaches the Project. The agent can create nested source files but has no shell.
- Browser-side builds with `esbuild-wasm`. The Kernel receives a static Build Artifact and never runs project code.
- A shared Rust Kernel, Nginx hostname routing, one durable SQLite database per app, local backups and optional S3 storage for static artifacts.
- Named, parameterized server actions: published browsers receive neither raw SQL nor database credentials.
- A separate owner database export endpoint. Deployed browsers cannot reach it.
- A Playwright test that creates a multi-file app, builds it in Chrome, publishes its artifact to MinIO and verifies task data after a reload.

## Run locally

```bash
bun install
bun run dev:local
```

This starts the Rust Kernel on port `8790` and the Vite editor on port `4173`. Open [http://localhost:4173](http://localhost:4173). Local app data stays under `data/kernel-local`; set `LITTLEBOX_DATA_DIR` to move it. The runner does not start Docker or require S3.

Set `LITTLEBOX_KERNEL_PORT` to change the local Kernel port. Set `LITTLEBOX_KERNEL_URL` to point the editor proxy at a different Kernel. The app data directory defaults to `data/kernel-local` and persists across restarts.

The local Kernel listens for signup without an invite so a personal prototype can add users. Treat this as development auth, not a public account system.

To run the complete publishing stack:

```bash
docker compose up --build
```

Open [http://editor.localhost:8080](http://editor.localhost:8080). Published apps receive URLs such as `http://tiny-tasks-a1b2c3.localhost:8080`; `*.localhost` resolves locally without editing `/etc/hosts` on modern browsers.

The local stack creates two MinIO buckets. `littlebox-apps` contains static artifacts and `littlebox-data` mirrors deployment metadata. Live SQLite databases, auth databases, uploads and backups remain on the Kernel volume. Mutations execute against the live database. MinIO's console runs at [http://localhost:9001](http://localhost:9001) with the development credentials `littlebox` and `littlebox-secret`.

The Rust Kernel reads this provider-neutral S3 configuration:

```text
LITTLEBOX_S3_ENDPOINT
LITTLEBOX_S3_REGION
LITTLEBOX_S3_ACCESS_KEY
LITTLEBOX_S3_SECRET_KEY
LITTLEBOX_ARTIFACT_BUCKET
LITTLEBOX_DATA_BUCKET
```

The buckets must already exist. This same configuration works with MinIO, Cloudflare R2, Scaleway Object Storage and Hetzner Object Storage through their S3 endpoints.

```bash
bun test
bun run build
cargo test --offline --manifest-path kernel-rs/Cargo.toml
bun run test:e2e:local
# Requires the Docker stack
bun run test:e2e:minio
```

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

Public deployments serve their app shell anonymously. Private deployments require a per-app login before serving the shell. Signup is open in this prototype, so it does not prove an owner invited an account. Server actions still check the current runtime actor, and the Rust Kernel rejects `granted` actions until it supports explicit deployment grants.

## Security status

- Runtime requests are bound to the hostname-resolved Deployment; the browser cannot select another database ID.
- Private deployments require a per-app login in this local prototype. Signup is open, so login does not prove that an owner invited the account.
- Files follow Deployment visibility in v1: files attached to a public app are public to its visitors; use a private Deployment for sensitive blobs.
- Public HTML is readable by anyone, but data is only reachable through declared actions.
- Sessions are opaque; only token hashes are persisted. Cookies are `HttpOnly` and host-only.
- Every Deployment gets a separate SQLite working file. The Kernel writes mutations transactionally and stores backups on explicit requests and before migrations.
- Action inputs are typed, parameterized and quota-limited; structural SQL is rejected from runtime actions.
- The local Docker editor uses `local-development-key` for its Admin Interface. This is intentionally a localhost bootstrap credential, **not production account authentication**. A network-accessible installation must replace it with Kernel owner sessions, CSRF protection, rate limiting and TLS.

## Export and self-host

Create either shared tasks or private notes from the project dashboard. The notes starter has a public landing page and actions that bind each row to the signed-in user. The Auth panel lets you simulate Guest, Alice and Bob in the preview; published apps use their own accounts.

Use Data to choose the local or deployed database, edit the model, review a generated migration, create backups and restore them. Publishing the same project keeps its URL and database. Local and deployed model acceptance are tracked separately.

Export project downloads a tar archive with source files, a built frontend, the Rust Kernel and startup scripts. For a published app it also includes the database, users and uploads. Imported users keep their password hashes, while sessions are cleared. The combined state limit is 16 MiB and each file is limited to 2 MiB. The archive contains instructions for Docker and native Cargo deployment.

Provider-free testing covers the editor and offline demo. Connect a provider in Agent settings to generate arbitrary apps with a model.

## Next slice

1. Add Kernel owner accounts and workspace sessions to remove the local Admin key from the browser.
2. Add scheduled off-volume backups and a restore workflow for moving a Kernel to a fresh machine.
3. Add immutable release history, rollback, audit records and file uploads backed by the private bucket.

Architecture and security boundaries are described in [docs/architecture.md](./docs/architecture.md).
