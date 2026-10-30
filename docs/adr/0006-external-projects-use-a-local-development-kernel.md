# Run External Projects against a local Development Kernel

Status: accepted, 2026-10-21. Builds on ADR-0004 and ADR-0005.

An External Project will develop against a local implementation of the Kernel interfaces instead of embedding production credentials or depending on live production state. Application code keeps using the same relative Runtime Interface for actions, auth and files in local preview and hosted Deployments. Remote production administration remains an explicit owner action, never an implicit part of the development runtime.

## Decision

The local toolchain routes Littlebox runtime requests to a Development Kernel. It shares the production Server Action parser, authorization policy and capability semantics, while local adapters provide a persistent development database, simulated Runtime Actors and local file storage. Browser code receives neither raw database access nor an administrative credential; it continues to call named Server Actions and the file/auth interfaces through `@littlebox/sdk`.

The normal development loop is local and cannot mutate production:

```text
Local app -> Development Kernel -> local database / actors / files
```

An explicit cloud-development mode may create a short-lived Preview with remote auth and storage, optionally initialized from an owner-authorized snapshot. Preview credentials stay in a local gateway or trusted tooling and never enter application JavaScript. Copying production data into that Preview is a separate, auditable owner operation.

Littlebox will not transparently proxy ordinary development requests into a live Deployment. An External Project Owner may separately open a short-lived, owner-authenticated Admin Interface connection for schema administration from local Drizzle tooling. Littlebox keeps that connection out of the Runtime Interface and browser, records the operation and takes infrastructure-level precautions such as backup and quotas; the External Project Owner remains responsible for destructive changes, drift and compatibility under ADR-0004 and ADR-0005. Managed Projects never receive this escape hatch.

## Considered options

- **Use ad hoc mocks for each capability.** Rejected because separate database, auth and file mocks would drift from Kernel policy. One Development Kernel preserves a single interface and concentrates parity work.
- **Connect local apps directly to production.** Rejected because routine edits and tests could mutate live state, require privileged connectivity and become dependent on network state.
- **Require a remote Preview for every edit.** Rejected as the default because it adds latency, availability and infrastructure cost to a workflow that SQLite, simulated actors and local files can reproduce. Remote Preview remains an explicit integration mode.

## Consequences

The Kernel's action parsing and authorization implementation must be reusable by hosted and development adapters. Littlebox must provide local persistence, actor switching, file handling, reset/seed/export tools, Preview lifecycle and secure owner administration for External Projects. Local and hosted storage implementations may vary, but tests must exercise the same Kernel interface so a capability cannot behave differently merely because the app was ejected.


## Note, 2026-11-10: a project starts without a Kernel, and a copy of the published app comes down

`littlebox init` no longer calls `project_create`. It makes the project ID, writes a template from `src/lib/templates` and names an installation only when `--url` does; the first `publish` registers the project, as `publishLocal` already did on every release. A folder can therefore be created, run on a Development Kernel and committed before any account exists.

`littlebox pull` is the owner-authorized snapshot this decision left open. It downloads the Deployment's package (ADR-0013) with `export_data` into `.littlebox/pull`, and a Development Kernel with no data installs it as the `initialPackage` of its first release. `dev --reset` deletes the local data so a copy can take effect; `dev` never replaces data on its own. The copy moves one way. No command sends local rows or accounts to a published app, and the Development Kernel's mail goes to a file.

When the folder's actions need a schema the copy does not have, the first release is refused before any Deployment exists. `dev` then installs the copy under a Capsule with no actions and keeps serving, and `littlebox sql` runs `database_execute` on it. The owner repeats the same statement on the published app with `littlebox sql --prod` before `publish`. `sql` reaches the published database only with that flag, so a rehearsal cannot land there by leaving a word out. That is the short-lived Admin Interface connection of this decision, through the MCP tool that exists, and it adds no Kernel surface.

We considered making `dev` pull on its own when a credential is present. Rejected because a run of the local loop would then read production, and the permission to download accounts would become something every developer needs.

We considered a `push` of local data. Rejected because a migration (ADR-0014) is the one way state reaches a published app, with one writer at a time.

## Note, 2026-11-10: one set of templates, and a feed of what a local run did

`project_create` returned five files of its own, an `items` table, while `init` wrote the editor's `notes` template, so an agent that started a folder through MCP and a person who ran the CLI got different apps. Both now read `kernel-rs/src/project-templates.json`, rendered from `src/lib/templates` by `bun run templates:render`, and `project_create` takes `template`.

`littlebox events` lists the email, calls, scheduled runs, routes and Signals of the Development Kernel, with the entries and words of the editor's What happened view, plus the development mailbox's lines, which name the recipient and hold the text. `--json` gives an agent one object per line. It reads existing owner tools and a file `dev` already wrote, and adds no Kernel surface.

`dev` serves that list as a page on the port after the app's, bound to `127.0.0.1`, from the CLI process and not from the Kernel. `LITTLEBOX_KERNEL_BIN` lets `dev` start a Kernel binary that is already built, which is the seam a packaged CLI needs; the CLI still reads this checkout for the SDK, the templates and the schema helper.
