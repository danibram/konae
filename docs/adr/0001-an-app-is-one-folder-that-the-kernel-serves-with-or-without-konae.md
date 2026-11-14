# An app is one folder that the Kernel serves, with or without Konae

Status: accepted.

A person who does not write code and a person who does make the same app, and either of them must be able to take it to another machine with its accounts, its data and its rules. Two kinds of project, or more than one export format, would each be a way out that the others do not share. This record keeps one shape and one way in and out.

## The words

| Word | Meaning |
| --- | --- |
| **Konae** | The platform: the editor, the build agent, owners and the list of apps. An app is made and managed there. |
| **Kernel** | The server that runs apps: Server Actions, app accounts, files, schedules, notices, routes, calls and Usage. Konae contains one. An exported app runs on one with no Konae around it. |
| **Owner** | A person with a Konae account. Owners exist only in Konae. The Kernel keeps no record of them. |
| **App user** | A person with an account in one app. The Kernel keeps the account and its session. Roles are rows in the app's own tables. |
| **App folder** | Everything an app is, in one layout, the same on a laptop, inside Konae and in an export. |
| **Export and import** | Export hands over the app folder. Import takes one. There is no other way in or out. |

## Decision

### One app folder

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
  package.json    npm run build writes dist/ from app/
  scripts/        the build the package runs
```

`konae init` writes it, `konae dev` serves it on the `kernel` binary, an export downloads it, and the Kernel keeps each app in it under `apps/<id>/`. Inside the Kernel the folder also holds `.konae/`, which keeps what the installation knows about the app, such as its record, backups, maintenance state, custom domain and path address. An export never hands `.konae/` over.

Secrets live in `data/secrets.sqlite`, because each secret has a use (`route`, `call` or `google`) and that needs a table, and because a Kernel without Konae must find the app's secrets in the app's own data. An export leaves the file out, and the `secret` command or `secret_set` sets them again on the destination.

A third reason keeps the secrets out of `identity.sqlite`. An export carries only the database, identity, files and usage, the components `COMPONENT_NAMES` lists in `kernel-rs/kernel/src/package.rs`, and an import refuses a package that names any other component. A file off that list never leaves with an export. A table inside `identity.sqlite` would leave too, unless the export emptied it, as it does for sessions, email links and invitation tokens today. Secret values are stored as plain text, so one forgotten table would export them in the open.

### The Kernel is a library, Konae is built on it

`kernel-rs` is a Cargo workspace. `kernel` is a library and a binary that serves a directory of app folders. `konae` links it and adds the owner's MCP on `/mcp`, owner accounts and admission, and the project list. The Kernel serves the runtime endpoint the browser SDK calls and never opens `owners.sqlite` or `mcp.sqlite`. Checks that need to know an owner, such as whether one owner reuses a route secret across apps, run in Konae.

### Owners stay in Konae, users stay in the app

- An app account has no `owner` role. The Kernel stores identity. An app that wants an administrator keeps roles in its own table, as the templates do.
- An owner manages users, invitations, the Signup Policy, API keys, secrets and SQL in Konae's editor or through Konae's MCP. The Kernel serves no admin page on an app's origin.
- An owner may enter the app as one of its users through a one-time link. Konae's audit log records it, and the session is marked so Usage and Signals do not count it as that user.
- Outside Konae the same operations are commands of the `kernel` binary on the machine: `users`, `invite`, `suspend`, `resume`, `signup-policy`, `api-key`, `secret`, `maintenance`, `enter`, `import`, `sql` and `events`. Each calls the function Konae's tool calls, so it refuses with the same words. `enter` labels the entry with the machine's login name and writes it to the Kernel's log, the one record of it without Konae.

## Rejected options

- **Two kinds of project, Managed and External, joined by an Eject.** Once a hand-edited `package.json` with any npm package became normal (ADR-0006), the two kinds had nothing left to separate them, and Eject was a second export with its own format.
- **A live migration with a confirmation handed between installations.** It kept two installations from both taking writes, at the cost of five tools, a second package version and a domain handover that depended on them. An owner moves an app with maintenance, export and import (ADR-0008), and nothing enforces that the origin stays paused. The owner who moves the app decides when the old copy stops.
- **An admin page on the app's origin, guarded by the owner's password.** Any script on that origin can open the page with `window.open` and read the password field, and the app's scripts include npm packages and agent-written code.
- **Owners as accounts with a role inside each app.** An import to another owner's installation left the previous owner holding that role, with SQL over every row. Keeping owners in Konae means an exported app carries no owner at all.
- **Secrets in Konae's own database.** A secret then stayed behind when the app moved to a Kernel without Konae, and a route or call that needed it stopped working with no way to set it there.

## Consequences

- The Kernel reads one app folder format and no other.
- An app built outside the editor arrives with its own `dist/`. When the editor's browser build can build `app/`, the app is edited in the web. When it cannot, Konae serves the `dist/` that came with it and the editor says the app is built on its owner's machine.
- The Kernel image is built with `Dockerfile.kernel`. Releases publish it as `ghcr.io/danibram/konae-kernel:<version>` for `linux/amd64`; see [Releasing](../releasing.md).
- This record leaves open an MCP endpoint per app for a user's agent, a plain HTTP runtime endpoint, and merging Konae's two databases.
