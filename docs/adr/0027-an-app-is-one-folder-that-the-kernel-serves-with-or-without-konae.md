# An app is one folder that the Kernel serves, with or without Konae

Status: accepted, 2026-11-11. Supersedes [ADR-0004](0004-use-drizzle-schema-as-the-managed-database-model.md), [ADR-0005](0005-build-artifacts-separate-managed-and-external-projects.md) except its static publishing rules, [ADR-0006](0006-external-projects-use-a-local-development-kernel.md) and [ADR-0014](0014-an-app-migrates-with-one-writer-at-a-time.md) except the maintenance state and the revocation record. Amends [ADR-0001](0001-separate-runtime-and-admin-data-access.md), [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md), [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md), [ADR-0012](0012-account-email-goes-through-a-persistent-queue.md), [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md), [ADR-0015](0015-a-deployment-answers-on-one-custom-domain.md), [ADR-0016](0016-a-mutation-may-notify-the-owner-or-the-actor.md), [ADR-0019](0019-an-app-calls-another-from-the-kernel.md) and [ADR-0026](0026-an-app-account-may-sign-in-with-the-apps-own-google-client.md) where each one names an owner inside an app, a secret's home, a migration or a kind of project.

No app runs on any installation yet, so nothing here keeps an old format alive.

## The concepts

| Word | Meaning |
| --- | --- |
| **Konae** | The platform: the editor, the build agent, owners, the list of apps. It is where an app is made and managed. |
| **Kernel** | The server that runs apps: Server Actions, app accounts, files, schedules, notices, routes, calls, Usage. Konae contains one. An exported app runs on one with no Konae around it. |
| **Owner** | A person with a Konae account. An owner exists only in Konae. The Kernel has no record of owners. |
| **App user** | A person with an account in one app. The Kernel keeps the account and its session. Roles are rows in the app's own tables. |
| **App folder** | Everything an app is, in one layout, the same on a laptop, inside Konae and in an export. |
| **Export / import** | Export hands over the app folder. Import takes one. There is no other way in or out. |
| **Gate** | The checks the editor runs before it accepts a change. The Kernel runs the same checks when the app runs. |

Removed: Managed Project, External Project, Eject, migration, pulled copy, Development Kernel, package as a separate format, Database Model and `schema.ts`, the `owner` role of an app account, `manage_lifecycle`.

## Decisions

### 1. One app folder

```
my-app/
  app/            pages, actions.json, schema.sql
  dist/           the built static files the Kernel serves
  data/           database.sqlite, identity.sqlite, usage.sqlite, secrets.sqlite, files/
  konae.json      name, visibility, Signup Policy, sign-in methods, contact email
  actions.d.ts    types for miniapp.actions.call, generated
  compose.yaml    runs the Kernel image on this folder
  .env.example    the installation settings the Kernel reads
  AGENTS.md       for a coding agent who opens the folder cold
  action-contract.md
```

`konae init` writes it, `konae dev` serves it, export downloads it, and the Kernel keeps each app in it under `apps/<id>/`. Before this record `init`, the export kit and the Kernel's `deployments/<id>/` were three layouts and saved sources sat in `mcp.sqlite`.

Secrets live in `data/secrets.sqlite`, because each one has a use (`route`, `call`, `google`) and that needs a table. An export leaves the file out.

### 2. Export and import

- Export hands over the app folder with everything of the app and nothing of Konae: no owner account, no credential, no project record, no secret.
- Import takes a folder. Into a new app, Konae assigns it to the owner who imports. Into an existing app, it replaces the sources, the data or both, as the owner chooses, after a backup that can be restored. It never merges rows. Sessions and API keys survive an import into the same app, as they survive a restore. Sessions of accounts the new data does not hold end.
- `project_export`, `database_export`, the self-host kit, the Eject package and the chunked package are one thing now. The chunked upload stays as how a folder travels over MCP.
- An owner can put an app in maintenance and take it out. Moving an app with no lost write is maintenance, export, import. ADR-0014 rejected trusting the operator to do that; this record accepts it for one concept fewer.

### 3. Owners stay in Konae, users stay in the app

- An app account has no `owner` role. The Kernel stores identity. An app that wants an administrator keeps roles in its own table, as the templates do.
- `"notify": { "to": "owner" }` sends to the app's contact email. A publish takes it from `konae.json`, then from what the app already holds, then from the owner's account. The owner can set it in App settings, and the editor says when an app declares an owner notice and has none.
- Managing users (list, invite, suspend, Signup Policy), API keys and SQL is done in Konae's editor. The Kernel serves no admin page on an app's origin.
- An owner may enter the app as one of its users through a one-time link. The audit log records it and the session is marked so Usage and Signals do not count it as that user. This replaces "Open as owner".
- Outside Konae the same operations are commands of the `kernel` binary on the machine: `users`, `invite`, `suspend`, `resume`, `signup-policy`, `api-key`, `secret`, `maintenance`, `enter`, `import`, `sql` and `events`. Each calls the function Konae's tool calls, so the refusals are the same words.

### 4. The Kernel is a library, Konae is built on it

`kernel-rs` becomes a Cargo workspace. `kernel` is a library and a binary that serves a directory of app folders. `konae` links it and adds the owner's MCP on `/mcp`, owner accounts and admission, and the app list. The Kernel keeps the runtime endpoint the browser SDK calls. The Kernel never opens `owners.sqlite` or `mcp.sqlite`. Secrets move from `mcp.sqlite` to the app's data, and the check that one owner does not reuse a secret across apps stays in Konae.

### 5. `schema.sql` is the only model

- `schema.sql` states what the database should be. The Kernel compares it with the live database, using SQLite itself to read both, and derives the change. Adding a table or a column applies. Dropping or retyping one shows the plan and asks.
- `schema.ts`, Drizzle, `schema_plan` and the bun subprocess in the Kernel go.
- `actions.d.ts` is generated from the Action Contract, so `miniapp.actions.call` is typed by action name, input and returned columns.

### 6. The publishing contract is `dist/`

Any builder may write `dist/`. The Kernel checks it as untrusted input against the static publishing rules of ADR-0005 and ADR-0007 and refuses what breaks them. The folder's default is an esbuild script whose `npm run build` passes. The Kernel does not build. When a folder arrives in Konae and the editor's browser build can build `app/`, the app is edited in the web. When it cannot, Konae serves the `dist/` that came with it and the editor says the app is built on its owner's machine.

### 7. A release is all or nothing

A publish stages and uploads first. It then copies the database, applies the schema change to the copy, and swaps the database, the record, `dist/` and the Deployment in memory together, undoing each earlier swap if a later one fails. Server Actions wait from the copy to the swap. A publish that fails leaves the old actions answering on the old rows.

### 8. One implementation of the checks, in part

The Gate is TypeScript and the Kernel is Rust, held together by shared case files. A spike compiled the Kernel's checks of `actions.json` to WebAssembly: they are pure, weigh 178 KB gzipped, and give the Gate's code and sentence on every shared case. The checks that compare actions with the schema need SQLite, and the editor's SQLite has no authorizer, so those stay two implementations joined by case files. The spike is kept at the tag `spike/checks-wasm` and is not merged. Adopting the pure half is later work.

## What was built

The rename to Konae; the removals; the one folder; the `kernel` and `konae` crates; schema changes derived by the Kernel with `actions.d.ts`; import into an existing app, maintenance and entering as a user; the `kernel` binary's commands and image; the all-or-nothing release. The image is built locally and not published: that needs a registry, a name, a build for more than one architecture and a version tag.

## Not in this record

An MCP endpoint per app for a user's agent, a plain HTTP runtime endpoint, merging Konae's two databases, and the merge of older records into fewer topic records.
