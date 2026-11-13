# CLI

The `konae` command makes an app in a folder, runs it on this machine, publishes it, and moves it. Run it from the repository root with Bun.

```sh
bun run konae -- <command> [options]
bun run konae -- help
```

`init`, `dev`, `build` and `contract` need no credential. Every other command except `login` reads its credential from `<folder>/.konae/credentials.json` or from `KONAE_MCP_TOKEN`. Never put a token in app source.

## login

```sh
bun run konae -- login --dir ./my-app --url http://localhost:4173 \
  [--manage-users] \
  [--export-data] \
  [--verify-from <mailbox.jsonl>]
```

Reads `KONAE_EMAIL` and `KONAE_PASSWORD`. Add `KONAE_INVITATION_CODE`, the code the operator issued, to create the account, or `KONAE_SETUP_KEY` for an installation's first owner. It saves a 24-hour token with `read`, `write` and `publish`.

| Flag | Adds | For |
| --- | --- | --- |
| `--manage-users` | `manage_users` | Inviting app users, suspending, changing the Signup Policy |
| `--export-data` | `export_data` | Exporting live data |

A delegated token cannot widen itself later, so the flags are the only way the CLI gets these permissions. On a Kernel that verifies email, creating the account sends a link. `--verify-from` reads that link from a development mailbox on this machine and works only with a Kernel on `localhost` or `127.0.0.1`.

## init

```sh
bun run konae -- init --dir ./my-app [--name 'My app'] [--template <id>]
```

Writes an app folder into an empty folder, without a credential or a Kernel: `konae.json`, the template in `app/`, `AGENTS.md` for a coding agent, `action-contract.md`, `compose.yaml`, `.env.example`, and a `package.json` whose `npm run build` writes `dist/`. `.gitignore` keeps `data/`, `.env` and `.konae/` out of Git. The templates are the editor's: `tasks`, `notes` (the default), `members`, `form`, `webhook`, `digest` and `gallery`. Its first `publish` registers the project on the installation.

## dev

```sh
bun run konae -- dev --dir ./my-app [--port 8796] [--events-port 8797] [--reset] [--confirm-schema <confirmation>]
```

Builds `dist/`, compiles the `kernel` binary, and runs it on the folder. The Kernel reads `konae.json`, `app/`, `dist/` and `data/` where they are, with no owner and no link to production. It checks the folder as a release and brings `data/database.sqlite` to what `app/schema.sql` states. A change that loses data is refused with the plan's confirmation; run `dev` again with `--confirm-schema` and that confirmation to apply it. When a file in `app/` changes, `dev` builds again and restarts the Kernel. The app keeps the Signup Policy in `konae.json`; to create a test account on an app that is not Open, invite it with the `kernel` binary's `invite` command, which `dev` prints. Its email, notices and invitations included, goes to `.konae/kernel/mailbox.jsonl`. `--reset` imports the data `pull` exported with the `kernel` binary's `import` command, which backs up `data/` into `.konae/backups` first, or empties `data/` when there is none. On every start and change it writes `actions.d.ts` beside `konae.json`, which types `miniapp.actions.call`.

## pull

```sh
bun run konae -- pull --dir ./my-app [--url <Kernel URL>]
```

Exports the published app's data, its database, accounts, files and Usage, into `.konae/pull/data`, laid out as an export lays out `data/`. Needs a credential from `login --export-data`. `dev --reset` imports it. `pull` changes nothing on the published app; `push` sends data back.

## push

```sh
bun run konae -- push --dir ./my-app [--url <Kernel URL>] [--yes] [--confirm-schema <confirmation>]
```

Replaces the published app's data with the folder's `data/`, never `.konae/pull/data`. A fresh `pull` waits there until `dev --reset` puts it in `data/`, so `pull` followed by `push` sends the older `data/`. It goes through `project_import` with `replace` set to `data`. It is the same import `kernel import` runs on a folder, aimed at the app's own Deployment. Rows are never merged. It needs `publish`.

It sends `database.sqlite`, `identity.sqlite` when present, `data/files` when that folder exists, and `usage.sqlite` when present. It never sends `secrets.sqlite`. Without `identity.sqlite` the published accounts stay, and without `data/files` the published files stay. Without `usage.sqlite` the published Usage and Signals are removed, as any import does.

Before it sends anything, `push` reads the installation's file limits from `platform_describe` and checks `data/files/index.json` against them. Every listed file needs its `<id>.blob` beside the index at the indexed size, no file may pass the per-file limit (2 MiB unless the operator raised `KONAE_FILE_MAX_BYTES`), and all files together may not pass the quota (20 MiB unless the operator raised `KONAE_FILES_QUOTA_BYTES`). A refusal names the largest offending file and the totals. A file with an `ownerId` must name an account in `identity.sqlite`. It copies each SQLite file with `VACUUM INTO`, so rows a running `dev` still holds in the write-ahead log go too, and builds the files archive itself with `index.json` first and each blob in index order. Nothing else in `data/files`, such as a `._index.json` that macOS leaves, goes with it.

Without `--yes` it prints the rows of each table here against the published rows, what happens to accounts, files and Usage, and sends nothing. With `--yes` it:

1. puts the app in maintenance, unless it already was;
2. uploads each part in the block size `transfer_begin` answers;
3. runs `project_import`, which backs up the published data first;
4. reads back the backup with `list-backups` and the published row counts with `database_execute` `inspect`;
5. takes the app out of maintenance, also when a step fails or you press Ctrl-C.

It prints the backup's id. `database_execute` with `operation` `restore-backup` and that `backupId` brings the published data back. When the published schema would drop a column or a table that `data/` holds, nothing changes. `push` prints the plan and its confirmation, and `--confirm-schema` with that confirmation applies it. [Bring existing data](./guides/bring-existing-data.md) says how to write `data/` from another source.

## sql

```sh
bun run konae -- sql --dir ./my-app [--prod] [--url <Kernel URL>] ("<statement>" | --file <file.sql>)
```

Runs SQL on `data/database.sqlite` with the `kernel` binary's `sql` command, the function `database_execute` calls on Konae. That is where to look at the exported data's rows after a schema change. Only `--prod` reaches the published Deployment, through `database_execute`.

## events

```sh
bun run konae -- events --dir ./my-app [--follow] [--json] [--days 7] [--prod]
```

Lists what the app did besides answer its page: email, calls to other services, scheduled runs, routes and Signals, oldest first. Locally an email shows its recipient and text, because nothing is sent. `--json` prints one object per line, `--follow` keeps printing, and only `--prod` reads the published Deployment. While `dev` runs, the same list is a page on the port after the app's.

## build

```sh
bun run konae -- build --dir ./my-app
```

Builds `app/` into `dist/` by the same rules as the editor's Publish and the folder's `npm run build`.

## contract

```sh
bun run konae -- contract --dir ./my-app [--out <folder>]
```

Writes `action-contract.json`, its schema, `action-contract.md` and `actions.d.ts`, from `app/actions.json` and `app/schema.sql`. `actions.d.ts` types `miniapp.actions.call` by action name, input and the columns a query returns. A page written in TypeScript, or a JavaScript page whose `jsconfig.json` sets `checkJs`, reads it when the folder's TypeScript configuration includes it.

## publish

```sh
bun run konae -- publish --dir ./my-app [--url <Kernel URL>] [--confirm-schema <confirmation>]
```

Builds and publishes to `--url` or the installation `login` saved. Set each signing secret with `secret_set` first, because a release that signs with an unset secret is refused. On a first release it sends `signupPolicy` from `konae.json`.

The Kernel changes the published database to what `schema.sql` states. A new table, column or index applies. When the change drops a table or a column, or changes a column's type, `publish` prints what it loses, the statements and the plan's confirmation, then asks. Without a terminal it publishes nothing; run it again with `--confirm-schema` and that confirmation. A confirmation fits only the plan it was printed for. `.konae/release-plan.json` keeps the last plan.

## conform

```sh
bun run konae -- conform --url <app URL> --contract ./my-app/action-contract.json [--token <app session>] [--key <API key>] [--secret <name>=<value>] [--inputs <file>]
```

Calls every action and checks the answers against the contract. It runs each mutation once, so point it at a test Deployment.

## export

```sh
bun run konae -- export --dir ./my-app --out <empty folder> [--with-data]
```

Writes the app folder into an empty folder: `konae.json`, `app/`, a fresh `dist/` and the files `init` writes. `--with-data` adds the published app's `data/` from `project_export` and needs a credential from `login --export-data`. Secrets and `.konae/` never go with it.

## domain

```sh
bun run konae -- domain --dir <folder> --deployment <id> [--set <hostname> | --clear] [--url <Kernel URL>]
```

Shows, sets or clears the app's custom domain. It reads the token from `<folder>/.konae/credentials.json`, or from `KONAE_MCP_TOKEN` when that is set. `--set` prints the TXT record that proves you control the hostname. Run it again once the record is at your DNS provider.

## Read next

- [Edit files with the CLI](./guides/edit-files-with-the-cli.md)
- [Take the app elsewhere](./guides/take-the-app-elsewhere.md)
- [MCP tools and guide topics](./mcp-tools.md)
