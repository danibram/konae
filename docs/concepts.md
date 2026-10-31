# Concepts

The words an app author meets, in the order the app meets them. They are the terms of `CONTEXT.md` at the root of the repository, and the editor and the Kernel use the same words in their messages.

Product terms are capitalized. Declaration keys stay lowercase: guard, notice, schedule, package, migration and invitation.

## Before the app runs

**Project.** The source lineage, schema, capability configuration and deployment history owned by a workspace member. Its source authority is either Managed by Konae or External under its owner.

**Capsule.** The editable source of a Project taken as one unit. It is a nested source tree with an entrypoint, `app.js` or `src/main.ts`, plus optional `index.html` and `styles.css`, the schema in `schema.sql`, which the Validation Gate and the Kernel check and run, the Database Model in `schema.ts` when the editor's Data panel manages the tables, and the Server Action declarations in `actions.json`. See [The Capsule](./capsule.md).

**Database Model.** The editable declaration of a Project's intended data structure, from which schema revisions and migrations are derived. It is source authority, unlike the current database or a generated SQL snapshot.

**Server Action.** A named, deployed operation that authorizes a Runtime Actor before running SQL or recording a Signal. It is the deployed app's only database interface. It runs one SQL statement. See [Server Actions](./server-actions.md).

**Validation Gate.** The check every agent-proposed change must pass before it is applied to a Project. The Gate and the Kernel refuse the same Capsule with the same words. See [The Gate and the Kernel](./gate-and-kernel.md).

**Build Artifact.** The validated, immutable static output of a Project revision that a Deployment serves. It holds no source authority, runtime database access or administrative capability. Under the Kernel's Content Security Policy its own inline scripts and same-origin files load, and inline event handler attributes and other origins do not.

**Deployment.** The published, addressable instance of a Project with its own hostname, an optional custom domain, visibility, signup policy and database. Everything it is lives in one folder on the Kernel volume. Republishing replaces its Build Artifact and keeps its hostname and data.

**Public Deployment.** A Deployment whose application shell may be opened without signing in. A signed-out visitor may call Server Actions declared `anonymous`. Public visibility grants no database access and no access to private files.

**Private Deployment.** A Deployment whose application shell and runtime require a signed-in Runtime Actor before anything loads or runs. The HTTP route of an `anonymous` action still answers its callers.

**Project Owner.** An authenticated workspace member allowed to edit a Project and administer its data, schema, files, Usage and deployments.

**Runtime Actor.** Whoever calls a Deployment at runtime. It is an anonymous visitor, a signed-in app user, or a service holding an API key. Its permissions are evaluated by server-side actions and never imply Project ownership.

**Signup Policy.** The per-Deployment rule for who may create a Runtime Actor account. It is Open, Invitation only, Closed or Allowlist. See [Accounts and access](./accounts-and-access.md).

**Agent Credential.** A Project Owner's revocable delegation that lets an external agent perform selected operations on selected Projects for a limited time. It grants no Runtime Actor identity.

**Development Kernel.** The local development authority that reproduces the Kernel interfaces without granting an application access to production resources. `konae dev` runs one.

## As the app grows

**Guard.** A mutation's declared refusal, `"guard": "<message>"`. The Kernel answers it instead of `{ changes: 0 }` when the statement changes no row because the app's own rule in its `WHERE` said no.

**Notice.** The email a mutation declares with `notify`, queued as an email job when its statement changes a row. It never goes to an address from the input.

**Schedule.** A mutation's declaration that the Kernel's own timer runs it, for work that must happen when nobody asks. The mutation takes no input.

**Signal.** A named occurrence the Capsule declares on a Server Action. The Kernel records it when the action succeeds, next to Usage and apart from application rows. Only the Project Owner reads Signals.

**Usage.** The Kernel-owned daily totals for one Deployment's authorized Runtime Interface operations. Only the Project Owner reads it.

**Invitation.** The Project Owner's offer of an account on one Deployment to one email, sent as a seven-day Challenge.

**Callers.** The kinds of caller an HTTP route takes, any of `signature`, `key` and `session`.

**Signature Scheme.** How a route's signature is computed and where it travels. `konae` is the default. `hmac-body`, `timestamped` and `svix` follow the providers that send them.

**Signing Secret.** A value the Project Owner sets through the Admin Interface. It never appears in `actions.json`, a Build Artifact or an export.

**API Key.** A secret the Project Owner issues for one Deployment so a program can call it as a Runtime Actor of kind service. It is sent as `Authorization: Bearer`. The Kernel stores its hash only.

**Call Action.** A Server Action of `"kind": "call"` that declares one HTTP request to another app or an outside service. The Kernel sends it from its own process, never from the browser.

**Outbox.** The calls a Deployment's call actions queued, kept in `calls.sqlite` with their attempts and status. The Project Owner reads it with `calls_read`.

**SDK Module.** A namespace on `window.miniapp`, such as `auth`, `actions` or `files`, with a contract of calls, arguments, return values, refusals and limits. [The SDK reference](./sdk.md) is rendered from it.

**Action Contract.** The generated description of a Capsule's Server Actions, `action-contract.json`. It is written from `actions.json` and `schema.sql`, never edited, and travels in the export. `konae conform` checks a backend against it.

**Custom Domain.** A hostname its owner controls on which one Deployment answers next to its generated hostname. The owner proves control with a TXT record.

**Package.** The export of one Deployment's state, which is its database, its accounts without sessions or email links, its API keys revoked at import, its files, and its Usage and Signals when it has any. A versioned manifest lists each component with its size and SHA-256.

**Migration.** The move of one Deployment to another installation so that exactly one installation takes its writes.

**Maintenance.** The state of a Deployment that takes no writes. Its host shows a page naming the operation, and its email waits.

**Managed Project.** A Project whose source and database-schema authority remain in Konae, so changes pass through its editor, agent, managed build and Validation Gate.

**External Project.** A Project created with external source authority. The owner supplies Build Artifacts and accepts schema compatibility responsibility.

## Words for running an installation

These matter only to whoever operates a Kernel. `CONTEXT.md` defines each of them, and [Operate an installation](./operate.md) uses them.

- Konae Kernel, Runtime Interface, Admin Interface
- Agent Workspace, Account, Account State, Workspace, Workspace Home
- setup key, invitation code, Challenge, Verification Mode
- email job, sender preflight, copy, revocation record

A copy moves an installation. A package moves one app.

## npm packages

An npm package is imported by name once the Capsule's `package.json` lists it with an exact version. The editor's build fetches it from esm.sh, `konae build` reads it from `node_modules`, and either way it goes into the app's bundle.

## Read next

- [The Capsule](./capsule.md)
- [Server Actions](./server-actions.md)
- [Accounts and access](./accounts-and-access.md)
- [Pick the smallest piece](./pieces.md)
