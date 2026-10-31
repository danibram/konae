# An app is one folder that the Kernel serves, with or without Konae

Status: proposed, 2026-11-10. Amends [ADR-0005](0005-build-artifacts-separate-managed-and-external-projects.md) (Eject and the Managed and External split go), [ADR-0007](0007-managed-builds-run-in-the-browser.md) only in naming a second builder outside the editor, [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md) (where the owner's MCP lives), [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md) (what a package holds), [ADR-0014](0014-an-app-migrates-with-one-writer-at-a-time.md) (the live migration goes) and [ADR-0018](0018-a-route-verifies-a-providers-own-signature.md) and [ADR-0019](0019-an-app-calls-another-from-the-kernel.md) (where a secret is stored).

Today one binary serves apps and hosts the editor's operations, and an app exists in three shapes: saved sources in `mcp.sqlite`, a Deployment folder, and a package that only another full installation can open as a signed-in owner. An owner has to learn Eject, Managed, External, package, migration and export to know how to take an app somewhere. The aim of this record is that the owner learns one thing, the app's folder, and that working on it outside the platform is as smooth as inside.

## Names

- **Konae** is the product and the platform: the editor, the build agent, creator accounts and their admission, the list of projects, publishing from the browser.
- **The Kernel** is the library and the server with everything one app needs to run: Server Actions, app accounts and sessions, the Signup Policy, guards, notices, schedules, signals, HTTP routes, call actions, files, Usage, and the checks the Validation Gate shares.

Identifiers take the Konae name in a change of their own, before the split: `KONAE_*` variables, `/_konae/` routes, `konae.json`, the `konae` CLI and the `konae-kernel` binary. This record writes the names the code has today.

## Decision

### One folder is the app

```
my-app/
  app/             sources: pages, actions.json, schema.sql
  dist/            what the build wrote, which is what the Kernel serves
  data/            database.sqlite, identity.sqlite, usage.sqlite, files/
  package.json     an ordinary Vite project: `npm run dev`, `npm run build`
  app.json         name, visibility, Signup Policy, sign-in methods, Kernel version
  compose.yaml     runs the published Kernel image on this folder
  .env             project secrets, never exported and never committed
  AGENTS.md        what the pieces are and where this app declares each rule
  action-contract.md
```

- The folder on the owner's machine, the folder Konae keeps for the project, and what export hands over are the same thing. Saved sources leave `mcp.sqlite` and live in `app/`.
- `docker compose up` in the folder serves the app. The image holds the Kernel, so nobody compiles Rust and nothing in the folder is tied to an operating system.
- The Kernel does not build, as ADR-0007 decided: the process that holds every app's database runs no project code and no third-party package. Outside Konae the folder is an ordinary Vite project. `npm run dev` gives a page that reloads on save and sends `/_littlebox/*` to the Kernel, and `npm run build` writes `dist/`. In Konae the editor's browser build writes the same `dist/`.
- A person on their own machine may build with anything. When a folder arrives, Konae tries the browser build on `app/`. If it builds, the app is edited in the web like any other. If it does not, Konae serves the `dist/` that came with it, checked as untrusted input the way ADR-0007 checks an artifact, and the editor says the app is built on its owner's machine. This is not a kind of project and nobody chooses it. It follows from the sources, and it ends when the sources build in the browser again.
- The Kernel reads `actions.json` and `schema.sql` when they change and refuses them with the Gate's words. The dev page shows that refusal, so there is no separate checking tool to install.
- `app/` and `app.json` are what a person or an agent edits and commits. `data/` is what the running app writes.

### Export and import are the only way in and out

- **Export** downloads the folder. The app on Konae keeps running and nothing about it changes.
- **Import** uploads a folder. Into a new project it creates the app. Into an existing project it replaces the sources, the data or both, as the owner chooses, after Konae takes a backup it can restore. It never merges rows.
- Eject, the Managed and External kinds of project, and the live migration of ADR-0014 are removed. A person who leaves for good exports and deletes the project.
- The owner can put an app in maintenance and take it out, with a button in Konae and on the admin page. Moving an app with no lost write is three steps the owner sees: maintenance, export, import. The maintenance state is the one ADR-0014 introduced; only its automation goes.
- A package never carries a secret, and API keys still travel revoked.

### The owner manages the app from the app

The Kernel serves `/_littlebox/admin` on every app to an app account with the role `owner`. The Kernel writes the page, not the Capsule. It holds accounts, invitations, suspension, the Signup Policy, API keys and SQL Studio, over the functions Konae's owner tools call today.

- The page asks for the owner's password to open and again for each change. It shares an origin with the app's scripts, and a password the scripts never see is the guard that holds on a shared origin.
- The app's scripts get no SDK for any of it.
- It is not served on a path address, where apps of different owners share one origin. The owner opens it on the app's own host.
- `owner <email>` on the machine names the first owner of an app that has none.

Secrets stay with whoever runs the machine, in `.env`. On Konae the owner sets them in the editor as now.

### One library, two binaries

`kernel-rs` becomes a Cargo workspace. The `kernel` crate is a library and a binary that serves a directory of app folders. The `konae` binary links it and adds the owner's MCP, creator accounts and the project list. One library keeps the rule that the Gate and the Kernel refuse with the same words.

The browser SDK reaches the app through `/_littlebox/mcp`, a JSON-RPC endpoint on the app's host that takes only an app session. It belongs to the Kernel. `/mcp`, with the owner's tools, belongs to Konae. `mcp.rs` splits along that line.

| Side | Modules |
| --- | --- |
| Kernel | `http`, `calls`, `schedule`, `notify`, `guard`, `checks`, `files`, `file_store`, `google`, `auth`, `path`, `path_name`, `domain`, `hosts`, `security`, `limits`, `app_db`, `action_shape`, `sql_rules`, `model_rules`, `refusal`, `provider`, `artifact`, `package`, `usage`, `mail`, `invitations`, `app_owner`, app accounts, the runtime half of `mcp`, `copies`, `recovery`, SQL Studio and backups |
| Konae | the owner half of `mcp`, `oauth`, `clerk`, `admission`, `guide`, creator accounts |
| Removed | `eject`, the migration tools except the maintenance state and its guard |

Konae's own data is `owners.sqlite` and `mcp.sqlite`. The Kernel never opens them. Usage stays in each app's `usage.sqlite`, so it leaves with the app, and Konae reads it for its overview.

## Order of work

1. Rename identifiers to Konae. No behaviour changes, and routes that published apps already call keep answering under the old prefix.
2. Inside the current crate, split `mcp.rs`, move `project_secrets` and the saved sources into the app's folder, and cut every reference from Kernel modules to creator accounts and project records. Behaviour does not change.
3. Split into the workspace. The compiler then proves step 2 is complete.
4. Serve a folder with no sign-in, publish the image, and write the folder's Vite project, `compose.yaml` and `AGENTS.md`.
5. The admin page, with the maintenance button.
6. Import into an existing project. Then remove Eject, the two kinds of project and the migration tools.

## Open

- Whether the Runtime Interface becomes plain HTTP routes after the split, and whether an app later lists each Server Action as an MCP tool for a person's agent.
- Whether `owners.sqlite` and `mcp.sqlite` merge.
- Where the Kernel image is published.
- Whether a Capsule builds under Vite unchanged. The editor bundles with esbuild in the browser and reads npm packages from esm.sh; the folder would bundle with Vite from `node_modules`. Step 4 starts by building the `examples/product-fit` apps both ways.
- How a change to `schema.sql` reaches `data/database.sqlite` in development.

## Consequences

- There are two builders, the editor's and Vite, reading the same sources. They already differ today between the editor and `littlebox build`, and AGENTS.md names that divergence.
- An owner who exports, changes rows outside and imports loses the rows Konae gained meanwhile, unless they import sources only. The import dialog says so and the backup undoes it.
- Moving an app between installations keeps one writer only if the owner presses maintenance first. Nothing enforces it.
- Projects that were ejected become ordinary projects whose `app/` holds their sources.
