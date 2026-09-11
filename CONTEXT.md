# Littlebox

Littlebox is a workshop for creating and publishing small applications whose code, deployment visibility and data access remain separate concerns.

## Language

**Account**:
The authenticated identity and personal profile used to enter Littlebox and participate in Workspaces. An Account does not directly own runtime data or imply access to every Project.
_Avoid_: Runtime Actor, Project Owner

**Workspace**:
A collection of Projects that an Account can browse and administer through membership. A personal Workspace may have one member; the term still preserves the future boundary for teams.
_Avoid_: Deployment list, app runtime

**Workspace Home**:
The owner-facing index of Projects and their latest Deployments, used to enter an editor or open a published app.
_Avoid_: App store, runtime dashboard

**Project**:
The source lineage, schema, capability configuration and deployment history owned by a workspace member. Its source authority is either Managed by Littlebox or External after Eject.
_Avoid_: App when referring specifically to editable source

**Capsule**:
The editable source of a Project taken as one unit — today the five files `index.html`, `styles.css`, `app.js`, `schema.sql` and `actions.json`. It is the shape the editor and the agent read and write; what a Deployment serves is the Build Artifact built from it.
_Avoid_: bundle, repo, the project files

**Database Model**:
The editable declaration of a Project's intended data structure from which schema revisions and migrations are derived. It is source authority, unlike the current database, a generated SQL snapshot or an individual migration.
_Avoid_: Live database, migration, schema dump

**Managed Project**:
A Project whose source and database-schema authority remain in Littlebox, so changes must pass through its editor, agent, managed build and Validation Gate.
_Avoid_: Hosted Project

**External Project**:
A Project whose source and database-schema authority have moved outside Littlebox through Eject. Littlebox retains its identity, capabilities and deployment history but the owner supplies Build Artifacts and accepts schema compatibility responsibility.
_Avoid_: Exported Project, detached Deployment

**Eject**:
The irreversible transfer of a Managed Project's source and database-schema authority to its owner while retaining the Project's Littlebox hosting and administration. Returning to managed authoring creates a new Project.
_Avoid_: Export, download

**Build Artifact**:
The validated, immutable static output of a Project revision that a Deployment serves. It contains no source authority, runtime database access or administrative capability.
_Avoid_: Project source, build cache

**Declared Dependencies**:
The libraries a Managed Capsule states it needs, written as intent by its owner or the agent. Littlebox resolves them against pinned, browser-compatible versions and generates the manifest and lock the build consumes; the generated pair is reviewable and exportable but is not a second editable source of truth.
_Avoid_: package.json, lockfile (those name the generated form, not the authority)

**Deployment**:
An immutable published version of a Project with its own hostname, visibility and capability grants.
_Avoid_: Project, preview

**Public Deployment**:
A Deployment whose application shell may be opened without membership. Public visibility grants no database or private-file access by itself.
_Avoid_: Public database

**Private Deployment**:
A Deployment whose application shell and runtime require an authenticated actor with an explicit grant.
_Avoid_: Password-protected app

**Runtime Actor**:
The end user interacting with a Deployment. Its permissions are evaluated by server-side actions and never imply Project ownership.
_Avoid_: Owner, editor user

**Project Owner**:
An authenticated workspace member allowed to edit a Project and administer its data, schema, files and deployments.
_Avoid_: Runtime user, app user

**Server Action**:
A named, deployed operation that authorizes a Runtime Actor before reading or changing application data. It is the deployed app's only database interface.
_Avoid_: Raw SQL endpoint

**Littlebox Kernel**:
The provider-independent authority that resolves Deployments, authenticates actors, owns sessions and grants, applies visibility, and authorizes capabilities.
_Avoid_: Cloudflare auth, provider gateway

**Development Kernel**:
The local development authority that reproduces the Kernel interfaces without granting an application access to production resources.
_Avoid_: Production proxy, mock API

**Runtime Interface**:
The Kernel interface used by a Deployment to open sessions and invoke its declared Server Actions.
_Avoid_: Database API

**Admin Interface**:
The Kernel interface used by a Project Owner for editing, schema changes, SQL Studio, backups and export.
_Avoid_: Runtime Interface

**Agent Workspace**:
The disposable copy of a Project's source that the agent reads and edits during a conversation. Changes in it are proposals; they reach the Project only through the Validation Gate.
_Avoid_: Project, filesystem

**Validation Gate**:
The check that every agent-proposed change must pass before it is applied to a Project: the schema must be valid, a schema change must come with a migration that converges, and every Server Action must remain executable against the migrated schema. Applying is versioned and reversible.
_Avoid_: linter, preview
