# Pick the smallest piece

Pick the smallest piece that does the job. Do not invent a second process, a second database door or a host rule the Capsule can already declare.

This is the same table as the `pieces` topic of `platform_guide`.

## The table

| Need | Use | Not |
| --- | --- | --- |
| Page and rows the app owns | Capsule + Server Actions | Raw SQL from the browser |
| Visitor or member reads or writes a row | Named Server Action with access and actor bindings | Owner `database_execute` in the app UI |
| Mutation matched no row | `guard` on that mutation | A second check statement |
| Email when a row changes | `notify` on that mutation | Browser mail or a call action for mail |
| Work when nobody is looking | `schedule` on a no-input mutation | External cron unless asked |
| Owner-facing occurrence | `signal` on a mutation, or `kind: signal` | A side table only the agent reads |
| Another service calls this app | `http` on an anonymous action with callers, signature or key | A call action (that is outbound) |
| This app calls another host | `kind: call` | `fetch` of a secret from the browser |
| Uploaded files | `miniapp.files` | Large bytes in a SQLite column |
| Who may create an app account | Signup Policy and invitations | A homemade users table that bypasses Runtime Actor |
| A short link on the installation's domain | Path address (`path_set`) | A custom domain when DNS is not yours |
| Take the app elsewhere | Export and import of the app folder | Copy (moves the installation) |
| Debug, or try the next version, on what the published app holds | `konae pull` (an export of the data) + `dev --reset` (its import) | Publishing to find out |
| See the email, calls, scheduled runs and Signals of a local run | `konae events` | Reading the Kernel's files by hand |

## The unit

A Deployment folder on the Kernel is the app. It holds the artifact, database, identity, files and Usage. Prefer a declaration inside the Capsule over a new platform resource.

## Where each piece is explained

- `guard`, `notify`, `schedule`, `signal`, `http` and `kind: call` are on [Server Actions](./server-actions.md).
- Signup Policy and invitations are on [Accounts and access](./accounts-and-access.md).
- Package export and import are in the [take the app elsewhere](./guides/take-the-app-elsewhere.md) guide.

## Read next

- [Server Actions](./server-actions.md)
- [Concepts](./concepts.md)
- [MCP tools and guide topics](./mcp-tools.md)
