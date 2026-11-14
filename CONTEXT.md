# Konae

Konae is the guardrail that lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged. The Validation Gate and the Kernel refuse the same Capsule with the same words, so a check that passes in the editor still passes on the machine the app leaves on.

## Language

Read "Words an app author uses" to make an app, edit its files or take it elsewhere. Read "Words for running an installation" only to operate a Kernel or a Konae installation. Each entry gives the meaning now and the words to avoid; the decision records in `docs/adr/` hold the rules.

Each term below is capitalized in its heading. In a sentence, in docs, in the editor and in Kernel messages, these terms are written in lowercase, because that is how the editor and the Kernel say them: API key, signing secret, guard, callers, app folder, copy, maintenance, revocation record, invitation, invitation code, email job, sender preflight, setup key, custom domain, path address, notice, schedule, signature scheme, call action, outbox, SDK module, sign-in methods, Google client, file store, app user, contact email and schema change. Usage, Signal and Action Contract keep their capitals.

## Words an app author uses

### The two halves of the product

**Konae**:
The platform: the editor, the build agent, owner accounts, the owner's MCP on `/mcp` and the project list. It is the `konae` binary in `kernel-rs/konae`, which links the Kernel, and the `konae` CLI in `scripts/konae.ts`. An app is made and managed there (ADR-0001).
_Avoid_: the Kernel (when the owner's side is meant), the builder

**Kernel**:
The server that runs apps: Server Actions, app accounts, files, schedules, notices, routes, calls and Usage. It is a library and the `kernel` binary in `kernel-rs/kernel`, which serves a directory of app folders with no Konae around it. Konae contains one, and `konae dev` runs the binary on one folder. It never opens an owner's records (ADR-0001).
_Avoid_: Konae Kernel, development Kernel, runtime server

**Owner**:
A person with a Konae account who makes and manages apps, in the editor or through the owner's MCP. Owners exist only in Konae, and an app folder carries none, so an export leaves its owner behind (ADR-0001). The account proves its email before it holds a session (ADR-0003).
_Avoid_: Project Owner, admin, creator

**App User**:
A person with an account in one app. The Kernel keeps the account and its sessions in the app's `data/identity.sqlite`. The Kernel keeps no role for it; an app that needs an administrator keeps roles in its own table and checks them in SQL against the actor binding (ADR-0001).
_Avoid_: member (that is a role an app may keep), runtime user, owner

**Runtime Actor**:
Whoever calls a Deployment at runtime: a visitor without a session, an app user, or a program holding an API key. An `actor` binding of `id` yields the app user's ID or the key's ID, and null for a visitor or a schedule's run.
_Avoid_: owner, editor user

### The app and its files

**Project**:
An app as Konae's project list knows it: its ID, its name, the owner who holds it and the sources the editor saves. Its ID names its app folder and its Deployment.
_Avoid_: workspace, repo, app (when the editable source is meant)

**App Folder**:
Everything an app is, in one layout that is the same on a laptop, inside the Kernel and in an export: `konae.json`, `app/`, `dist/`, `data/`, `actions.d.ts`, `action-contract.md`, `AGENTS.md`, `compose.yaml` and `.env.example`. `konae init` writes it, `konae dev` serves it, an export hands it over, and the Kernel keeps each app in one under `apps/<id>/`. Inside the Kernel it also holds `.konae/`, what the installation knows about the app, which never leaves. `kernel-rs/kernel/src/layout.rs` names every path (ADR-0001).
_Avoid_: kit, package, workspace, repo

**Capsule**:
The app's editable sources, `app/` of the app folder: its pages, styles and scripts, `schema.sql`, `actions.json` and an optional `package.json`. The editor and the build agent read and write it; a Deployment serves the Build Artifact built from it.
_Avoid_: bundle, repo, the project files

**Server Action**:
A named operation in `actions.json` and the only way a page reaches the database. The Kernel authorizes the Runtime Actor from the action's access and bindings, then runs its one SQL statement, records its Signal, or queues its call. Its kinds are `query`, `mutation`, `signal` and `call` (ADR-0002, ADR-0004).
_Avoid_: raw SQL endpoint, database API

**Validation Gate**:
The editor's check of `schema.sql` and `actions.json`, in `src/agent/gate.ts`, that a change must pass before Apply. It and the Kernel refuse with the same code and sentence, which is what lets an app leave with its rules (ADR-0004).
_Avoid_: linter, preview

**Refusal**:
What either door answers when the app's files break a rule: `{ code, message, hint }`, where `hint.fix` says whether the Capsule, the owner or the operator clears it. `kernel-rs/kernel/src/refusal-codes.json` holds every code (ADR-0004).
_Avoid_: validation error, lint warning

**Schema Change**:
The change the Kernel derives from `schema.sql` to a live database, at publish, at import and when the `kernel` binary starts an app folder. A new table, column or index applies. A dropped table, a dropped column or a retyped column loses data and applies only with its confirmation, the digest of the plan's statements that `release_plan` returns. An owner passes it as `confirmSchema`, `--confirm-schema` or `KONAE_CONFIRM_SCHEMA`, and it fits only the plan it came from. A rename reads as a drop and an add (ADR-0005).
_Avoid_: migration, schema diff

**actions.d.ts**:
The types of `miniapp.actions.call` by action name, input and returned columns, generated from the Action Contract into the app folder by `konae init`, `konae contract`, `konae dev` and an export. It is never edited (ADR-0005).
_Avoid_: SDK types, API client

**Build Artifact**:
The static files in `dist/` with a manifest of their paths, hashes, types, `encodings` and `routing`. Any builder may write it; the Kernel checks it and serves it under its Content Security Policy. `src/lib/static-artifact.js` decides which files of the Capsule it holds (ADR-0006).
_Avoid_: project source, build cache

**Declared Dependencies**:
The npm packages a Capsule imports by name, listed with exact versions under `dependencies` in its `package.json`. The editor's build reads them from esm.sh and `konae build` from `node_modules`; the published app loads nothing from another origin (ADR-0006).
_Avoid_: library allowlist, vendored copy

### The published app

**Deployment**:
The published app on one installation: its generated hostname, optional custom domain and path address, visibility, Signup Policy and database. It shares its Project's ID and lives in that app folder. A publish replaces its Build Artifact and keeps its hostname and data, all or nothing (ADR-0005).
_Avoid_: project, preview

**Public Deployment**:
A Deployment whose `konae.json` says `"public"`. A visitor without a session may open its shell, call actions declared `anonymous` and read files uploaded as public. Visibility grants no access to private rows or files (ADR-0002).
_Avoid_: public database

**Private Deployment**:
A Deployment with any other visibility, or none. Its shell, assets and runtime endpoint need an app session; an HTTP route still answers the callers it declares (ADR-0002). A new app is private unless its template is made for visitors.
_Avoid_: password-protected app

**Signup Policy**:
The rule for who may create an app account: Open, Allowlist, Invitation only or Closed. A public app starts Open and a private one Closed. The owner chooses it on the first release and changes it with `signup_policy_set`, which needs `manage_users` (ADR-0002, ADR-0003).
_Avoid_: invite list, access grant

**Sign-in Methods**:
The ways an app account signs in: `password` always, and `google` once the owner sets the app's Google client and turns it on with `sign_in_methods_set`. `konae.json` carries the list; the client stays behind (ADR-0003).
_Avoid_: social login, SSO

**Google Client**:
An app's own Google OAuth client, which its owner makes in their own Google Cloud console and sets as the signing secrets `google_client_id` and `google_client_secret` with `"use": "google"`. The installation has none, and an export never carries it (ADR-0003).
_Avoid_: OAuth app, installation Google client

**Agent Credential**:
An owner's delegated token for an agent: an expiry, a set of projects and the permissions `read`, `write`, `publish`, `manage_users` and `export_data`. OAuth offers the first four. A tool the credential lacks answers with the permission it needs (ADR-0002).
_Avoid_: runtime session, setup key

### As the app grows

**Guard**:
A mutation's `"guard": "<message>"`, which the Kernel answers as a refusal when the statement changes no row (ADR-0010).
_Avoid_: check constraint, validation error

**Notice**:
The email a mutation declares with `"notify"`, queued when its statement changes a row. It goes to the app's contact email, to the calling account, or to the accounts a changed row names in a column, and never to an address from the input (ADR-0012).
_Avoid_: notification, alert, webhook

**Contact Email**:
The address in `konae.json` where notices to `"owner"` go. The owner sets it in App settings or with `contact_email_set`; a first publish from Konae fills it with the owner's email (ADR-0012).
_Avoid_: owner email, admin address

**Schedule**:
A mutation's `"schedule"`, `{ "every": "15m" }` or `{ "daily": "02:00" }` in UTC, that only the Kernel's timer runs. The mutation takes no input, and a page polls a query to see what it did (ADR-0011).
_Avoid_: cron, job, worker, background task

**Signal**:
A named occurrence a Server Action declares, with properties from its input bindings or `actor`, recorded next to Usage when the action succeeds. Only the owner reads Signals (ADR-0010).
_Avoid_: event, track, custom metric

**Usage**:
The Kernel's daily totals for one Deployment: sessions, page views, actions, routes, uploads, file reads, signups, logins, refusals, notices, calls and scheduled runs, with the activity of each signed-in app account and what the app stores. It lives in `data/usage.sqlite`, and only the owner reads it (ADR-0010).
_Avoid_: analytics, telemetry, audit, metric

**Invitation**:
The owner's offer of an account in one app to one email, sent as a seven-day challenge. Open, Invitation only and Closed accept it; an Allowlist still has to list the address (ADR-0003).
_Avoid_: invite code (that admits an owner), allowlist entry

**Entering as a User**:
An owner signing a browser into the app as one of its active accounts, through a link that works once for two minutes. `app_user_enter` needs the owner's browser session, and Konae's audit log records it; the `kernel` binary's `enter` writes it to the Kernel's log. Usage and Signals do not count that session as the user (ADR-0001).
_Avoid_: impersonation, owner role

**Callers**:
The kinds of caller an HTTP route takes, `"signature"`, `"key"` and `"session"`, resolved before any SQL runs (ADR-0013).
_Avoid_: principals, auth mode, access level (that is `anonymous` or `authenticated` on the action)

**Signature Scheme**:
How a route's signature is computed and where it travels: `konae` by default, or a provider's `hmac-body`, `timestamped` or `svix` (ADR-0013).
_Avoid_: webhook format, provider adapter

**Signing Secret**:
A value the owner sets with `secret_set` or `kernel secret set`, with one use: `route` verifies a route, `call` signs an outgoing call, `google` holds the Google client. It lives in the app's `data/secrets.sqlite`, is never returned, and never travels in `actions.json`, a Build Artifact or an export (ADR-0013).
_Avoid_: webhook token, API key

**API Key**:
A token the owner issues for one Deployment so a program can call its routes and read its files as a Runtime Actor. The Kernel keeps only its hash. An export carries it revoked at import (ADR-0013).
_Avoid_: token (that is an Agent Credential), service account

**Call Action**:
A Server Action of `"kind": "call"` that declares one HTTP request, which the Kernel queues, sends from its own process and answers with its `then` mutation (ADR-0013).
_Avoid_: webhook out, fetch action, integration

**Outbox**:
The calls a Deployment queued, in the installation's `calls.sqlite`, which the owner reads with `calls_read`. An export does not carry it.
_Avoid_: call log, queue (the mail queue holds email)

**SDK Module**:
A namespace on `window.miniapp`, such as `auth`, `actions` or `files`, described once in `src/sdk/contract.ts`, from which the build agent's prompt, `platform_guide` topic `sdk` and `docs/sdk.md` are rendered.
_Avoid_: plugin, library, extension

**Action Contract**:
The generated description of a Capsule's Server Actions, `action-contract.json` and `action-contract.md`: inputs, returned columns, Signals, callers, errors and limits. `konae conform` checks a backend against it (ADR-0004).
_Avoid_: API spec, OpenAPI document, schema dump

**Custom Domain**:
A hostname the owner proves with a TXT record `_konae.<hostname>` and on which one Deployment answers as its own origin, next to its generated hostname (ADR-0014).
_Avoid_: vanity URL, CNAME, domain mapping

**Path Address**:
`<domain>/p/<id>`, and `<domain>/p/<name>` once named, on which one Deployment answers on the origin it shares with every other path address. An app with accounts, or a private app, needs the owner to accept that sharing first (ADR-0014).
_Avoid_: short link, vanity path, app URL

**File Store**:
Where an installation keeps the bytes of uploaded files: each app folder's `data/files/`, or an S3 bucket. The index in the app folder decides which files exist and who may read them (ADR-0009).
_Avoid_: bucket (one kind of store), storage

### Taking the app elsewhere

**Export**:
The app folder handed over: `konae.json`, `app/` and `dist/` in one download, and each part of `data/` (database, accounts, files, Usage) as its own download with a manifest of sizes and SHA-256. It carries no owner, credential, secret, session or `.konae/`, and its API keys arrive revoked. `project_export`, `konae export`, `konae pull` and the editor's Export are the one export, and downloading accounts and data needs `export_data`. Tool arguments call the manifest and its downloads a package (ADR-0008).
_Avoid_: kit, dump, full backup

**Import**:
An export taken in. Into a new app the data travels with the first release as `initialPackage`; into an existing app `project_import` replaces the sources, the data or both, and `kernel import` the data, after a backup and never merging rows. `konae dev --reset` imports what `konae pull` exported (ADR-0008).
_Avoid_: migration, sync, restore (that installs a copy)

**Maintenance**:
The state of a Deployment that takes no writes: its host shows a maintenance page, owner tools that would change it are refused except `maintenance_set` and `project_import`, and its email, calls and schedule wait. It lives in `.konae/maintenance.json`. An owner moves an app with no lost write by maintenance, export and import (ADR-0008).
_Avoid_: downtime, read-only mode (reads stop too), migration

## Words for running an installation

**Installation**:
One running Kernel, with or without Konae, and its data directory: `apps/`, the mail queue, the outbox and, under Konae, the owner and MCP stores.
_Avoid_: instance, cluster

**Operator**:
The person who runs an installation: its settings, its copies, its file store and, under Konae, who may become an owner.
_Avoid_: owner, admin

**Kernel Commands**:
What the `kernel` binary does on the machine instead of serving. On one app: `users`, `invite`, `suspend`, `resume`, `signup-policy`, `api-key`, `secret`, `sql`, `events`, `maintenance`, `enter` and `import`, each calling the function the owner tool calls, with the same refusals (`kernel-rs/kernel/src/operator.rs`). On the installation: `restore`, `reconcile`, `files-migrate` and `files-reconcile`. The `konae` binary runs the installation commands and adds `owner-code-create`, `owner-code-list`, `owner-code-revoke`, `owner-account-suspend`, `owner-account-resume` and, with Clerk, `owner-suspend`. Under Konae the owner tools do what the commands on one app do.
_Avoid_: admin page, CLI (that is `konae`)

**Runtime Endpoint**:
`/_konae/mcp` on an app's own origin, which the SDK calls with the app's session. No runtime credential authorizes an owner tool (ADR-0002).
_Avoid_: Runtime Interface, database API

**Owner's MCP**:
Konae's MCP on `/mcp`, which the editor, the build agent and an outside agent all use with an owner's credential (ADR-0002).
_Avoid_: Admin Interface, admin API

**Scratch Copy**:
The copy of a Capsule the build agent edits in the browser. Its changes reach the Project only through the Validation Gate and the owner's Apply (ADR-0007).
_Avoid_: Agent Workspace, sandbox

**Project List**:
The editor's home at `/`, listing the projects the signed-in owner holds. Projects live on the installation, not in the browser.
_Avoid_: Workspace Home, dashboard, app store

**Installation Home**:
The app whose path address holds the name `konae-home`. It answers the installation's bare domain (ADR-0014).
_Avoid_: landing page, project list

**Account State**:
Whether an owner or app account may hold a session: pending until it proves its email, active, or suspended. Only an active account gets one, and suspension keeps the account's rows (ADR-0003).
_Avoid_: enabled, verified flag

**Challenge**:
A single-use, expiring proof sent by email, with one purpose (verification, invitation or recovery) and one scope (owners, or one app). The Kernel keeps only its hash (ADR-0003).
_Avoid_: magic link, reset token

**Verification Mode**:
`KONAE_ACCOUNT_VERIFICATION`: `local`, where signup creates an active unverified account for development, or `required`, where every account proves its email first (ADR-0003).
_Avoid_: email confirmation setting

**Setup Key**:
`KONAE_SETUP_KEY`, which creates Konae's first owner while no owner and no invitation code exists. It never authenticates an MCP tool, and Konae refuses to start on a reachable address while it is the development default (ADR-0003).
_Avoid_: installation key, admin password, Agent Credential

**Invitation Code**:
A single-use, expiring code the operator issues with `konae owner-code-create`, which admits one owner. The account it creates still proves its email (ADR-0003).
_Avoid_: invite link, setup key, invitation (that is an app's)

**Email Job**:
One email in the installation's queue, `mail.sqlite`: purpose, recipient, scope and state (pending, accepted, delivered or failed). An export does not carry it (ADR-0012).
_Avoid_: notification, message log

**Sender Preflight**:
The Kernel's check that its mail transport may send as the configured sender. While it refuses, account email is not queued and a notice fails (ADR-0012).
_Avoid_: email health

**Copy**:
A snapshot of every app folder and installation store, sent on a schedule to a directory or an S3 prefix. `restore` installs one into an empty data directory. A copy moves an installation; an export moves one app (ADR-0008).
_Avoid_: backup (one app's snapshot before an import or on request), replica, mirror

**Revocation Record**:
Every suspension of an account and revocation of an API key, written to each copy target before the tool answers, so a restore cannot bring back what was revoked since (ADR-0008).
_Avoid_: audit log
