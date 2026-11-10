# Concepts

The words an app author meets, in the order the app meets them. `CONTEXT.md` at the root of the repository holds each one with the words to avoid, and the editor and the Kernel use the same words in their messages.

Product terms keep their capitals in a heading. In a sentence these are lowercase: app folder, app user, guard, notice, schedule, invitation, signing secret, API key, callers, call action, outbox, custom domain, path address, contact email, file store, schema change and maintenance. Usage, Signal and Action Contract keep their capitals.

## The two halves

**Konae.** The platform: the editor, the build agent, owner accounts, the owner's MCP on `/mcp` and the project list. An app is made and managed there.

**Kernel.** The server that runs apps: Server Actions, app accounts, files, schedules, notices, routes, calls and Usage. Konae contains one, and the `kernel` binary serves app folders with no Konae around it. It never opens an owner's records.

**Owner.** A person with a Konae account who makes and manages apps, in the editor or through the owner's MCP. An app folder carries no owner, so an export leaves its owner behind.

**App user.** A person with an account in one app. The Kernel keeps the account in the app's `data/identity.sqlite` and gives it no role. An app that needs an administrator keeps roles in its own table and checks them in SQL.

**Runtime Actor.** Whoever calls a Deployment: a visitor without a session, an app user, or a program holding an API key.

## The app and its files

**Project.** An app as Konae's project list knows it: its ID, its name, its owner and the sources the editor saves. Its ID names its app folder and its Deployment.

**App folder.** Everything an app is, in one layout that is the same on a laptop, inside the Kernel and in an export: `konae.json`, `app/`, `dist/`, `data/`, `actions.d.ts`, `action-contract.md`, `AGENTS.md`, `compose.yaml` and `.env.example`. `konae init` writes it, `konae dev` serves it, and the Kernel keeps each app in one under `apps/<id>/`. [The architecture](./architecture.md#the-app-folder) shows every path.

**Capsule.** The app's editable sources, `app/` of the app folder: its pages, styles and scripts, `schema.sql`, `actions.json` and an optional `package.json`. See [The Capsule](./capsule.md).

**Server Action.** A named operation in `actions.json` and the only way a page reaches the database. The Kernel authorizes the Runtime Actor from the action's access and bindings, then runs its one SQL statement, records its Signal or queues its call. See [Server Actions](./server-actions.md).

**Validation Gate.** The editor's check of `schema.sql` and `actions.json` that a change must pass before Apply. It and the Kernel refuse with the same code and sentence. See [The Gate and the Kernel](./gate-and-kernel.md).

**Refusal.** What either door answers when the app's files break a rule: `{ code, message, hint }`, where `hint.fix` says whether the Capsule, the owner or the operator clears it.

**Schema change.** The change the Kernel derives from `schema.sql` to a live database, at publish, at import and when the `kernel` binary starts an app folder. A new table, column or index applies. A dropped table, a dropped column or a retyped column applies only with the confirmation the plan names.

**Build Artifact.** The static files in `dist/` with a manifest of their paths and hashes. The Kernel checks it and serves it under its Content Security Policy.

## The published app

**Deployment.** The published app on one installation: its generated hostname, optional custom domain and path address, visibility, Signup Policy and database. A publish replaces its Build Artifact and keeps its hostname and data.

**Public Deployment.** A Deployment a visitor may open without a session. A visitor may call actions declared `anonymous` and read files uploaded as public. Visibility grants no access to private rows or files.

**Private Deployment.** A Deployment whose pages and runtime need an app session. An HTTP route still answers the callers it declares.

**Signup Policy.** Who may create an app account: Open, Allowlist, Invitation only or Closed. See [Accounts and access](./accounts-and-access.md).

**Sign-in methods.** Password always, and Google once the owner sets the app's own Google client and turns it on.

**Agent Credential.** An owner's delegated token for an agent, with an expiry, a set of projects and the permissions `read`, `write`, `publish`, `manage_users` and `export_data`.

## As the app grows

**Guard.** A mutation's `"guard": "<message>"`, which the Kernel answers as a refusal when the statement changes no row.

**Notice.** The email a mutation declares with `notify`, queued when its statement changes a row. It goes to the app's contact email, to the calling account, or to the accounts a changed row names, and never to an address from the input.

**Contact email.** The address in `konae.json` where notices to `"owner"` go. The owner sets it in App settings or with `contact_email_set`.

**Schedule.** A mutation's `"schedule"`, `{ "every": "15m" }` or `{ "daily": "02:00" }` in UTC, that only the Kernel's timer runs. The mutation takes no input.

**Signal.** A named occurrence a Server Action declares, recorded next to Usage when the action succeeds. Only the owner reads Signals.

**Usage.** The Kernel's daily totals for one Deployment, kept in `data/usage.sqlite`. Only the owner reads it.

**Invitation.** The owner's offer of an account in one app to one email, sent as a link that works once within seven days.

**Callers.** The kinds of caller an HTTP route takes: `signature`, `key` and `session`.

**Signature scheme.** How a route's signature is computed and where it travels: `konae` by default, or a provider's `hmac-body`, `timestamped` or `svix`.

**Signing secret.** A value the owner sets with `secret_set` or `kernel secret set`, for a route, a call or the Google client. It never travels in `actions.json`, a Build Artifact or an export.

**API key.** A token the owner issues for one Deployment so a program can call its routes and read its files. The Kernel keeps only its hash, and an import revokes it.

**Call action.** A Server Action of `"kind": "call"` that declares one HTTP request, which the Kernel queues and sends from its own process.

**Outbox.** The calls a Deployment queued, which the owner reads with `calls_read`.

**SDK module.** A namespace on `window.miniapp`, such as `auth`, `actions` or `files`. [The SDK reference](./sdk.md) is rendered from its contract.

**Action Contract.** The generated description of a Capsule's Server Actions, `action-contract.json` and `action-contract.md`. `konae conform` checks a backend against it.

**Custom domain.** A hostname the owner proves with a TXT record, on which one Deployment answers as its own origin.

**Path address.** `<domain>/p/<id>`, or `<domain>/p/<name>` once named, on which one Deployment answers on an origin it shares with every other path address.

## Taking the app elsewhere

**Export.** The app folder handed over, with its database, accounts, files and Usage. It carries no owner, credential, signing secret, session or `.konae/`, and its API keys arrive revoked. The editor's Export, `konae export`, `konae pull` and `project_export` are the one export.

**Import.** An export taken in: into a new app with its first release, into an existing app with `project_import` or `kernel import`, after a backup and never merging rows. `konae dev --reset` imports what `konae pull` exported.

**Maintenance.** The state of a Deployment that takes no writes. Its host shows a maintenance page, and its email, calls and schedule wait. An owner moves an app with no lost write by maintenance, export and import.

## Words for running an installation

These matter only to whoever runs a Kernel or a Konae installation. `CONTEXT.md` defines each, and [Operate an installation](./operate.md) uses them: installation, operator, Kernel commands, runtime endpoint, owner's MCP, scratch copy, project list, installation home, account state, challenge, verification mode, setup key, invitation code, email job, sender preflight, copy and revocation record.

A copy moves an installation. An export moves one app.

## npm packages

A Capsule imports an npm package by name once its `package.json` lists it with an exact version. The editor's build fetches it from esm.sh, `konae build` reads it from `node_modules`, and either way it goes into the app's bundle.

## Read next

- [The Capsule](./capsule.md)
- [Server Actions](./server-actions.md)
- [Accounts and access](./accounts-and-access.md)
- [Pick the smallest piece](./pieces.md)
