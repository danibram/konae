# Littlebox

Littlebox is the guardrail that lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged. The code, who may open a Deployment, and who may read a row stay separate. The Validation Gate and the Kernel refuse the same Capsule, so a check that passes in the editor still passes on the machine the app leaves on.

## Language

**Account**:
The authenticated identity and personal profile used to enter Littlebox and participate in Workspaces. An Account does not directly own runtime data or imply access to every Project.
_Avoid_: Runtime Actor, Project Owner

**Account State**:
Whether an Account or a Runtime Actor's account may hold a session: pending until it proves control of its email, active, or suspended by the operator or the Project Owner. Only an active account gets a session, and suspension keeps the account's ID and rows.
_Avoid_: enabled, verified flag

**Challenge**:
A single-use, expiring proof sent by email, with one purpose (verification, invitation or recovery) and one scope (the creator scope or one Deployment). The Kernel stores only its hash; answering it sets the account's password.
_Avoid_: magic link, reset token

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
The editable source of a Project taken as one unit. It is a nested source tree with an entrypoint, `app.js` or `src/main.ts`, from which relative imports are bundled, plus optional `index.html` and `styles.css`, the Database Model in `schema.ts` and the Server Action declarations in `actions.json`. It is the shape the editor and the agent read and write; what a Deployment serves is the Build Artifact built from it.
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
The irreversible transfer of a Managed Project's source and database-schema authority to its owner while retaining the Project's Littlebox hosting and administration. Returning to managed authoring creates a new Project.
_Avoid_: Export, download

**Build Artifact**:
The validated, immutable static output of a Project revision that a Deployment serves. It contains no source authority, runtime database access or administrative capability. Under the Kernel's Content Security Policy its own inline scripts and same-origin files load, and inline event handler attributes and other origins do not.
_Avoid_: Project source, build cache

**Declared Dependencies**:
The libraries a Managed Capsule states it needs, written as intent by its owner or the agent. Littlebox resolves them against pinned, browser-compatible versions and generates the manifest and lock the build consumes; the generated pair is reviewable and exportable but is not a second editable source of truth.
_Avoid_: package.json, lockfile (those name the generated form, not the authority)

**Deployment**:
The published, addressable instance of a Project with its own hostname, visibility, signup policy and database. Everything it is lives in one folder on the Kernel volume: its record, Build Artifact, databases, files and backups. Republishing replaces its Build Artifact and keeps its hostname and data; immutable release history does not exist yet.
_Avoid_: Project, preview

**Copy**:
A consistent snapshot of every Deployment folder, with the owner and MCP stores, sent off the Kernel volume to a second disk or an S3 prefix on a schedule. Restore installs one Copy into an empty data directory and signs everyone out. A Copy moves an installation; an export moves one app.
_Avoid_: backup (a Data panel snapshot of one app database), replica, mirror

**Public Deployment**:
A Deployment whose application shell may be opened without signing in. A signed-out visitor may call Server Actions declared `anonymous`, and may list and read files marked public. Uploads and private files require a Runtime Actor session. Public visibility of the Deployment grants no database access and no access to private files.
_Avoid_: Public database

**Private Deployment**:
A Deployment whose application shell and runtime require a signed-in Runtime Actor before anything loads or runs. Its Signup Policy decides who may create that account.
_Avoid_: Password-protected app

**Signup Policy**:
The per-Deployment rule for who may create a Runtime Actor account: Open (anyone), Allowlist (exact emails or `@domain` entries) or Closed (no new accounts). Public Deployments start Open and Private Deployments start Closed. The Project Owner sets it in the editor's Auth panel or through admin MCP tools. Under required Verification Mode the Kernel checks it again when the account verifies.
_Avoid_: Invite list, access grant

**Runtime Actor**:
The end user interacting with a Deployment. Its permissions are evaluated by server-side actions and never imply Project ownership.
_Avoid_: Owner, editor user

**Project Owner**:
An authenticated workspace member allowed to edit a Project and administer its data, schema, files, Usage and deployments.
_Avoid_: Runtime user, app user

**Server Action**:
A named, deployed operation that authorizes a Runtime Actor before running SQL or recording a Signal. It is the deployed app's only database interface.
_Avoid_: Raw SQL endpoint

**SDK Module**:
A namespace on `window.miniapp`, such as `auth`, `actions` or `files`, with a contract: its calls, their arguments, what they return, the messages they refuse with and the limits that apply. The SDK contract in `src/sdk/contract.ts` is the one description; the build agent's prompt, the `sdk` topic of `platform_guide` and `docs/sdk.md` are rendered from it. A module adds a door to an existing primitive and never a new one.
_Avoid_: plugin, library, extension

**Usage**:
The Kernel-owned totals for one Deployment's authorized Runtime Interface operations, distinct from application rows.
_Avoid_: analytics, telemetry, audit, metric

**Signal**:
A named occurrence the Capsule declares on a Server Action, stored by the Kernel for that Deployment and distinct from application rows.
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
The check that every agent-proposed change must pass before it is applied to a Project: the schema must be valid, a schema change must come with a migration that converges, and every Server Action that runs SQL must remain executable against the migrated schema. Applying is versioned and reversible.
_Avoid_: linter, preview

**Agent Credential**:
A Project Owner's revocable delegation that permits an external agent to perform selected operations on selected Projects for a limited time. It grants no Runtime Actor identity.
_Avoid_: Runtime session, installation key
