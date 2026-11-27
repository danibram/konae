# Pick the smallest piece

Pick the smallest piece that does the job. Prefer a declaration inside the Capsule over a new platform resource, a second process or a second database door.

This is the table the `pieces` topic of `platform_guide` returns, and the one in `AGENTS.md`.

## The table

| Need | Use | Not |
| --- | --- | --- |
| Page and rows the app owns | Capsule + Server Actions | Raw SQL from the browser |
| Visitor or app user reads or writes a row | Named Server Action with access and actor bindings | Owner `database_execute` in the app UI |
| Mutation matched no row | `guard` on that mutation | A second check statement |
| Email when a row changes | `notify` on that mutation | Browser mail or a call action for mail |
| Run one SQL mutation on an interval or at a UTC time | `schedule` on a no-input mutation | External cron |
| Run scheduled JavaScript or use a cron expression | `crons.json` and a Function with scheduled access | External cron |
| Owner-facing occurrence | `signal` on a mutation, or `kind: signal` | A side table only the agent reads |
| Another service calls this app | Turn on `http` with `module_set`, then declare it on an anonymous action with callers / signature / key | A call action (that is outbound) |
| This app calls another host | Turn on `calls` with `module_set`, then declare `kind: call` | `fetch` of a secret from the browser |
| Uploaded files | `miniapp.files` | Large bytes in a SQLite column |
| Every signed-in user reads every uploaded file | `files.fileAccess` set to `app-users` with `module_set` | Each upload as public |
| Who may create an app account | Signup Policy and invitations | A homemade users table that bypasses Runtime Actor |
| An administrator inside the app | A role column in the app's own table, checked in SQL | An owner account inside the app |
| Turn an app capability on or set its options | `modules_read` and `module_set`, then read it back with `modules_read` | A separate tool or a setting in the Capsule |
| Be listed in search | Turn on `seo` with `module_set`, after `modules_read` | Assume a public app is listed |
| A short link on the installation's domain | Path address (`path_set`) | A custom domain when DNS is not yours |
| Take the app elsewhere | Export and import of the app folder | Copy (moves the installation) |
| Debug, or try the next version, on what the published app holds | `konae pull` (an export of the data) + `dev --reset` (its import) | Publishing to find out |
| Put data from the folder, or from another system, into the published app | `konae push` (an import of the folder's `data/`, after a backup) | A migration script that drives transfers and imports by hand |
| See the email, calls, scheduled runs and Signals of a local run | `konae events` | Reading the Kernel's files by hand |

## The unit

The app folder is the app: `konae.json`, `app/`, `dist/` and `data/` with its database, accounts, files and Usage. The Kernel keeps it under `apps/<id>/`, and an export hands the same folder over.

## Where each piece is explained

- `guard`, `notify`, `schedule`, `signal`, `http` and `kind: call` are on [Server Actions](./server-actions.md).
- `crons.json` and scheduled Function access are on [Server functions](./functions.md#run-a-function-on-a-cron-schedule).
- Signup Policy, invitations and roles kept by the app are on [Accounts and access](./accounts-and-access.md).
- Export and import are in the [take the app elsewhere](./guides/take-the-app-elsewhere.md) guide.
- `miniapp.files` and the `files.fileAccess` module option are in the [upload files](./guides/upload-files.md) guide.

## Read next

- [Server Actions](./server-actions.md)
- [Concepts](./concepts.md)
- [MCP tools and guide topics](./mcp-tools.md)
