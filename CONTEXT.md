# Littlebox

Littlebox is the guardrail that lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged. The code, who may open a Deployment, and who may read a row stay separate. The Validation Gate and the Kernel refuse the same Capsule, so a check that passes in the editor still passes on the machine the app leaves on.

## Language

Each term below is capitalized in its heading. In a sentence, in docs, in the editor and in Kernel messages, the terms the code introduced in November 2026 are written in lowercase, because that is how the editor and the Kernel already say them: API key, signing secret, guard, callers, package, copy, migration, maintenance, revocation record, invitation, invitation code, email job, sender preflight, setup key, SDK module, call action and outbox. Usage, Signal and Action Contract keep their capitals, as the older terms do.

**Account**:
The authenticated identity and personal profile used to enter Littlebox and participate in Workspaces. An Account does not directly own runtime data or imply access to every Project.
_Avoid_: Runtime Actor, Project Owner

**Account State**:
Whether an Account or a Runtime Actor's account may hold a session: pending until it proves control of its email, active, or suspended by the operator or the Project Owner. Only an active account gets a session, and suspension keeps the account's ID and rows.
_Avoid_: enabled, verified flag

**Challenge**:
A single-use, expiring proof sent by email, with one purpose (verification, invitation or recovery) and one scope (the creator scope or one Deployment). The Kernel stores only its hash; answering it sets the account's password.
_Avoid_: magic link, reset token

**Invitation Code**:
A single-use, expiring code the operator issues on the server with the Kernel executable, which admits one creator Account. The Kernel stores its hash and the creator it produced. It is not a Challenge, because nobody receives it by email and it proves no address, so the account it creates still verifies its email. The setup key creates only the first creator; once any code exists it creates none.
_Avoid_: invite link, setup key, app invitation (the Deployment's own invitation is a Challenge)

**Setup Key**:
The installation secret in `LITTLEBOX_ADMIN_KEY` that creates the first creator Account while the installation has no account and no invitation code was ever issued. It never authenticates an MCP tool and never enters the editor bundle. The Kernel refuses to start on a reachable address while it is still the development default.
_Avoid_: installation key, admin password, Agent Credential

**Email Job**:
One account email in the installation's queue: its purpose, recipient, scope, idempotency key and state, pending, accepted by the provider, delivered to the recipient's server or failed. It commits with its Challenge and is sent only while that Challenge is open. Delivered says a mail server took it, not that a person read it.
_Avoid_: notification, message log

**Sender Preflight**:
The Kernel's check that its mail transport may send as the configured sender, such as a domain verified in Resend. While it refuses, nothing is queued or sent.
_Avoid_: email health

**Verification Mode**:
The installation's choice between local, where signup creates an active unverified account for development, and required, where every new account starts pending and accounts that never verified return to pending.
_Avoid_: email confirmation setting

**Workspace**:
A collection of Projects that an Account can browse and administer through membership. A personal Workspace may have one member; the term still preserves the future boundary for teams.
_Avoid_: Deployment list, app runtime

**Workspace Home**:
The owner-facing index of Projects and their latest Deployments, used to enter an editor or open a published app.
_Avoid_: App store, runtime dashboard

**Project**:
The source lineage, schema, capability configuration and deployment history owned by a workspace member. Its source authority is either Managed by Littlebox or External under its owner.
_Avoid_: App when referring specifically to editable source

**Capsule**:
The editable source of a Project taken as one unit. It is a nested source tree with an entrypoint, `app.js` or `src/main.ts`, from which relative imports are bundled, plus optional `index.html` and `styles.css`, the Database Model in `schema.ts` and the Server Action declarations in `actions.json`. Its pages are every `.html` at the root and under `pages/`, and files under `public/` are published as they are. It is the shape the editor and the agent read and write; what a Deployment serves is the Build Artifact built from it.
_Avoid_: bundle, repo, the project files

**Database Model**:
The editable declaration of a Project's intended data structure from which schema revisions and migrations are derived. It is source authority, unlike the current database, a generated SQL snapshot or an individual migration.
_Avoid_: Live database, migration, schema dump

**Managed Project**:
A Project whose source and database-schema authority remain in Littlebox, so changes must pass through its editor, agent, managed build and Validation Gate.
_Avoid_: Hosted Project

**External Project**:
A Project created with external source authority or transferred out of Littlebox through Eject. Littlebox retains its identity, capabilities and deployment history but the owner supplies Build Artifacts and accepts schema compatibility responsibility.
_Avoid_: Exported Project, detached Deployment

**Eject**:
The irreversible transfer of a Managed Project's source and database-schema authority to its owner while retaining the Project's Littlebox hosting and administration. It runs on the Kernel against an expected saved-sources revision and needs the `manage_lifecycle` permission. The owner receives the saved sources as a code package before the Project changes; the Kernel then records who ejected it and when, refuses every Managed source change, and keeps accepting Build Artifacts. The Deployment's URL, accounts, sessions, data, invitations and email continue. Returning to managed authoring creates a new Project.
_Avoid_: Export, download

**Build Artifact**:
The validated, immutable static output of a Project revision that a Deployment serves. It contains no source authority, runtime database access or administrative capability. Under the Kernel's Content Security Policy its own inline scripts and same-origin files load, and inline event handler attributes and other origins do not. Every builder makes it by one rule set: each page with the SDK, the files the pages load and `public/`, with binary files as base64 named in `encodings`, and a build that names a file the Capsule does not hold is refused. Its routing mode is `pages` by default, where a path it does not hold answers 404 with its `404.html`, or `spa`, where such a path serves `index.html`. The Validation Gate does not judge an artifact; the builders refuse a bad reference, and the builders and the Kernel refuse a routing mode other than these two with the same words.
_Avoid_: Project source, build cache

**Declared Dependencies**:
The libraries a Managed Capsule states it needs, written as intent by its owner or the agent. Littlebox resolves them against pinned, browser-compatible versions and generates the manifest and lock the build consumes; the generated pair is reviewable and exportable but is not a second editable source of truth. Not built yet: ADR-0007 decides it, and today a Capsule imports only its own files.
_Avoid_: package.json, lockfile (those name the generated form, not the authority)

**Deployment**:
The published, addressable instance of a Project with its own hostname, visibility, signup policy and database. Everything it is lives in one folder on the Kernel volume: its record, Build Artifact, databases, files and backups. Republishing replaces its Build Artifact and keeps its hostname and data; immutable release history does not exist yet.
_Avoid_: Project, preview

**Copy**:
A consistent snapshot of every Deployment folder, with the owner, MCP and mail stores, sent off the Kernel volume to a second disk or an S3 prefix on a schedule. With a copy secret, the files sent to S3 are encrypted. Each copy target also keeps the revocation record. Restore installs one copy into an empty data directory, signs everyone out, ends every open Challenge and applies the revocation record, or holds the installation in maintenance when it cannot read it. A copy moves an installation; a package moves one app.
_Avoid_: backup (a Data panel snapshot of one app database), replica, mirror

**Package**:
The export of one Deployment's state: its database, its accounts without sessions or email links, its API keys revoked at import, its files, and its Usage and Signals when it has any, captured under the Deployment's locks. A versioned manifest lists each component with its size and SHA-256 and names the published revision. Each component travels as its own transfer, so no step holds a whole component in memory. An import verifies every component and installs the package only into a project that has no Deployment yet; the same package imported again returns the first import. Downloading one needs the `export_data` permission. A migration's package is version 2 and imports in maintenance.
_Avoid_: dump, full backup, export file (the kit that carries it is the export)

**Migration**:
The move of one Deployment to another installation so that exactly one installation takes its writes. The origin enters maintenance and captures a package with its live artifact. It is then either confirmed, which retires it for good, shows a link to the new address for seven days and reveals a confirmation, or cancelled, which resumes it. The destination imports the package in maintenance and leaves it only through activation with that confirmation. People sign in again with their passwords; sessions, links and signing secrets do not travel, and API keys arrive revoked.
_Avoid_: replication, sync, transfer (a transfer is one upload or download)

**Maintenance**:
The state of a Deployment that takes no writes. Its host shows a page naming the operation and refuses actions, uploads, signups, sign-ins, recovery, invitations and HTTP routes, the owner cannot change it, and its email waits. A migration keeps it in the Deployment's folder, `migration.json`, so a restart or a restore keeps it. A restore that cannot read the revocation record keeps it for the whole installation in `recovery-hold.json` until `littlebox-kernel reconcile` runs.
_Avoid_: downtime, read-only mode (reads stop too)

**Revocation Record**:
The suspensions of accounts and revocations of API keys, each written to every copy target as it happens. A restore applies it so a copy taken earlier cannot bring back what was revoked since; it never lifts a suspension.
_Avoid_: audit log (that stays on the volume)

**Public Deployment**:
A Deployment whose application shell may be opened without signing in. A signed-out visitor may call Server Actions declared `anonymous`, and may list and read files marked public. Uploads, and changing a file's visibility or deleting it, require a Runtime Actor session and only the uploader may do the last two; private files need a session or an API key that names them. Public visibility of the Deployment grants no database access and no access to private files.
_Avoid_: Public database

**Private Deployment**:
A Deployment whose application shell and runtime require a signed-in Runtime Actor before anything loads or runs. An HTTP route of an action declared `anonymous` answers to its callers. A signed route takes its signature or an API key, and an unsigned one takes an API key or a session unless it declares other `callers`. Its Signup Policy decides who may create that account.
_Avoid_: Password-protected app

**Signup Policy**:
The per-Deployment rule for who may create a Runtime Actor account: Open (anyone), Invitation only (only through an invitation), Closed (nobody signs up, and an invitation the owner sends can still be accepted) or Allowlist (exact emails or `@domain` entries, kept as an advanced option). Public Deployments start Open and Private Deployments start Closed; a first release may name another. The Project Owner chooses it in the publish dialog on a first release and changes it later in the editor's Auth > Users view or with `signup_policy_set`, which needs `manage_users`, and a republish never changes it except to close an Open app made private. The Kernel checks it again when an account verifies and when an invitation is accepted. An External Project names its first release's policy in `littlebox.json` as `project.manifest.signupPolicy`, and `littlebox publish` sends it only then.
_Avoid_: Invite list, access grant

**Invitation**:
The Project Owner's offer of an account on one Deployment to one email, sent as a seven-day Challenge. Accepting sets the password and activates the account in the transaction that rechecks the Signup Policy and the account's state. It never changes an active account's password or resumes a suspended one. A package, a portable export and a restore keep the invitation as history without its link, and the owner resends it.
_Avoid_: invite code (that names creator admission), allowlist entry

**Runtime Actor**:
Whoever calls a Deployment at runtime, of three kinds: an anonymous visitor, a signed-in app user, or a service holding an API key. Its permissions are evaluated by server-side actions and never imply Project ownership. An `actor` binding yields the app user's ID or the key's ID, and nothing for a visitor.
_Avoid_: Owner, editor user

**API Key**:
A secret the Project Owner issues for one Deployment so a program can call it as a Runtime Actor of kind service. It names the Server Actions it may call over their HTTP routes and the file visibilities it may read, and is sent as `Authorization: Bearer`. The runtime endpoint refuses it. The Kernel stores its hash only. A package or a portable export carries its name, scope and hash, and the import marks it revoked at import, so the destination's owner sees which programs to issue new keys for; a restore of a copy keeps it valid.
_Avoid_: token (an Agent Credential), service account

**Signing Secret**:
A value the Project Owner sets for a Project through the Admin Interface and an HTTP route names in its signature. One set with `"use": "call"` instead signs, or holds the API key for, the Project's call actions, and may repeat another project's route secret. A sender proves a request with an HMAC under it of a timestamp, a nonce, the method, the path, the canonical query string and the body. It never appears in `actions.json`, a Build Artifact or an export.
_Avoid_: webhook token, API key

**Project Owner**:
An authenticated workspace member allowed to edit a Project and administer its data, schema, files, Usage and deployments.
_Avoid_: Runtime user, app user

**Server Action**:
A named, deployed operation that authorizes a Runtime Actor before running SQL or recording a Signal. It is the deployed app's only database interface. It runs one SQL statement. A query starts with `SELECT`, a mutation with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`, and a `WITH` clause sits inside the statement, never in front of it. The Validation Gate and the Kernel refuse a statement that breaks a rule, and an action whose name, access, kind or bindings they do not take, with the same words, and a project declares at most 100 actions. A call action runs no SQL; the Kernel sends its request. An action declared `anonymous` may also declare an HTTP route, one method and path on its Deployment host, that runs it for the callers the route takes, of a signed request, an API key and a signed-in user; an `authenticated` action may not. An action whose route does not answer anyone answers only there, never through `miniapp.actions.call`.
_Avoid_: Raw SQL endpoint

**Callers**:
The kinds of caller an HTTP route takes, any of `"signature"` (a service that signs the request with the signing secret), `"key"` (an API key that names the action) and `"session"` (a signed-in app user), declared as `"callers"` on the route. Without the list a signed route takes a signature or a key, an unsigned route on a Private Deployment takes a key or a session, and an unsigned route on a Public Deployment answers anyone. An action whose route does not answer anyone answers only on that route. The Validation Gate and the Kernel parse the list from one case file.
_Avoid_: principals, auth mode, access level (that is `anonymous` or `authenticated` on the action)

**Call Action**:
A Server Action of `"kind": "call"` that declares one HTTP request to another app or an outside service: a method, a URL, a body built from its inputs, an optional `auth` that signs with a signing secret or sends the API key a secret holds, and an optional `then` mutation. Invoking it queues the call and answers `{ callId, queued: true }`; the Kernel sends it from its own process, never from the browser, retries it, and runs the `then` mutation with the answer. A URL is https, or http to an app of this installation, never a private address, the editor or the Kernel's own paths. A call to an app of this installation runs through that app's route code with the same callers check.
_Avoid_: webhook out, fetch action, integration

**Outbox**:
The calls a Deployment's call actions queued, kept in `calls.sqlite` with their attempts, status, error kind and what the `then` mutation did, which the Project Owner reads with `calls_read`. Pending calls stay on the installation; they do not travel in a package.
_Avoid_: call log, queue (the mail queue holds account email)

**Guard**:
A mutation's declared refusal, `"guard": "<message>"`, which the Kernel answers instead of `{ changes: 0 }` when the statement changes no row because the app's own rule in its `WHERE` said no. It is a tool error on the runtime endpoint and 403 on a route, counts under Usage's `refused` and records no Signal.
_Avoid_: check constraint, validation error

**Action Contract**:
The generated description of a Capsule's Server Actions, `action-contract.json` with its JSON Schema and a Markdown rendering: for each action its inputs with types, nullability and limits, the columns a query returns or the `{ changes, lastInsertId }` a mutation returns, the Signal it records, who may call it on the runtime endpoint and on its HTTP route, the errors the Kernel answers with and the limits that apply. It is written from `actions.json` and `schema.sql`, never edited, and travels in the export. The conformance runner (`littlebox conform`) calls every action against a backend and checks the answers against it, which is how a second backend would show it serves the same app.
_Avoid_: API spec, OpenAPI document, schema dump

**SDK Module**:
A namespace on `window.miniapp`, such as `auth`, `actions` or `files`, with a contract: its calls, their arguments, what they return, the messages they refuse with and the limits that apply. The SDK contract in `src/sdk/contract.ts` is the one description; the build agent's prompt, the `sdk` topic of `platform_guide` and `docs/sdk.md` are rendered from it. A module adds a door to an existing primitive and never a new one.
_Avoid_: plugin, library, extension

**Usage**:
The Kernel-owned daily totals for one Deployment's authorized Runtime Interface operations: sessions opened, actions by name, HTTP routes by method and path, uploads, file reads, signups and logins. A refused call counts nothing, except a guard's refusal, which counts under `refused` by action name. It lives in the Deployment's `usage.sqlite`, distinct from application rows, and only the Project Owner reads it.
_Avoid_: analytics, telemetry, audit, metric

**Signal**:
A named occurrence the Capsule declares on a Server Action, with properties taken from that action's input bindings and, as `actor`, the caller's ID. The Kernel records it when the action succeeds, and for a mutation only when it changed a row, next to Usage and distinct from application rows. An action of kind `signal` records one and runs no SQL. The app calls it with `miniapp.actions.call`; only the Project Owner reads Signals.
_Avoid_: event, track, custom metric

**Littlebox Kernel**:
The provider-independent authority that resolves Deployments, authenticates actors, owns sessions, grants, Usage and Signals, applies visibility, and authorizes capabilities.
_Avoid_: Cloudflare auth, provider gateway

**Development Kernel**:
The local development authority that reproduces the Kernel interfaces without granting an application access to production resources.
_Avoid_: Production proxy, mock API

**Runtime Interface**:
The Kernel interface used by a Deployment to open sessions and invoke its declared Server Actions.
_Avoid_: Database API

**Admin Interface**:
The Kernel interface used by a Project Owner for editing, schema changes, SQL Studio, backups, export and Usage.
_Avoid_: Runtime Interface

**Agent Workspace**:
The disposable copy of a Project's source that the agent reads and edits during a conversation. Changes in it are proposals; they reach the Project only through the Validation Gate.
_Avoid_: Project, filesystem

**Validation Gate**:
The check that every agent-proposed change must pass before it is applied to a Project. The schema must be valid, a schema change must come with a migration that converges, and every Server Action that runs SQL must remain executable against the migrated schema. Applying is versioned and reversible.
_Avoid_: linter, preview

**Agent Credential**:
A Project Owner's revocable delegation that permits an external agent to perform selected operations on selected Projects for a limited time. It grants no Runtime Actor identity. Its permissions are `read`, `write`, `publish`, `manage_users`, `export_data` and `manage_lifecycle`. Only `manage_users` lists, suspends and invites app accounts or changes a Signup Policy, only `export_data` downloads accounts and data, and only `manage_lifecycle` ejects or migrates a Project. An OAuth client may hold the first four. A call it may not make is refused with the permission it needs.
_Avoid_: Runtime session, setup key
