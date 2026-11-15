# CLI

The compiled `konae` command creates an app folder, runs it locally, publishes it and moves it. Build it with `bun run scripts/build-konae-binary.ts`; the script writes `out/konae-<version>-<os>-<arch>`. Copy that binary to a directory on PATH. The published v0.8.0 release has four Kernel downloads and `SHA256SUMS`, but no CLI downloads. The CLI downloads the Kernel matching its version and checks its SHA-256 against `SHA256SUMS`. Only macOS arm64 CLI execution was verified, and its binary requires macOS 13.0. The other CLI platforms and release workflow remain unverified.

Three things carry the name `konae`.

| Name | What it is | Where it runs |
| --- | --- | --- |
| Konae | The platform, which holds the editor, owner accounts and the owner's MCP on `/mcp`. | `bun run dev:local`, Docker Compose |
| This CLI | The compiled Konae CLI, built from `scripts/konae.ts`. | `konae`, or `bun run konae --` in a repository checkout |
| The server binary | `kernel-rs/konae`, with operator commands such as `owner-code-create`. The CLI directs you to that server binary when you enter a server command. | `kernel-rs/target/debug/konae` under `bun run dev:local`, `docker compose exec --user konae kernel konae <command>` under Compose |

```sh
konae <command> [options]
konae help
```

The examples use `bun run konae --` for a checkout. With the compiled binary on PATH, remove `bun run` and the separator `--`.

`init`, `dev`, `build`, `contract`, `conform`, `export` without `--with-data`, and `sql` and `events` without `--prod` need no owner credential. `publish`, `pull`, `push`, `domain`, `hostname`, `secret`, `tool`, `export --with-data`, and `sql` and `events` with `--prod` read a credential from `<folder>/.konae/credentials.json` or `KONAE_MCP_TOKEN`. Check its actual permissions with `tool session_inspect`; `push`, `secret` and `hostname` also need an installation credential. Never put a token in app source.

`--url` names the installation URL, which is the editor origin. `KONAE_URL` sets the default for `login`; without it, `login` uses `http://localhost:4173`. Other remote commands use their `--url` or the installation saved by `login`.

## login

```sh
konae login --dir ./my-app --url http://localhost:4173 \
  [--browser [--no-open]] \
  [--manage-users] \
  [--export-data] \
  [--verify-from <mailbox.jsonl>]
```

With `--browser`, login prints an address and opens it in the system browser, unless `--no-open`. The owner opens it in a browser on the same machine, signs in and allows access within 5 minutes. The browser hands the code to a port on `127.0.0.1` that login listens on, and login trades it for the token with PKCE. Neither the password nor the token passes through the terminal, so an agent can run login and hand the address to the owner. It saves the same 24-hour token as a password login, one hour on an installation that signs owners in through Clerk. When that token expires, the installation answers when it expired and that `login --browser` gets a new one.

Without `--browser`, login reads `KONAE_EMAIL` and `KONAE_PASSWORD`. When one is unset and the command runs in a terminal, it asks for it, and the password is not echoed. Without a terminal it refuses, so a script sets both. Add `KONAE_INVITATION_CODE`, the code the operator issued, to create the account, or `KONAE_SETUP_KEY` for an installation's first owner. It saves a 24-hour token with `read`, `write` and `publish`. `token_create` makes a token of up to 30 days, but only an owner's browser session may call it, so the saved token cannot. The owner sign-in page at `/_konae/owner/login` calls it for you and offers 1, 7 or 30 days. Put such a token in `KONAE_MCP_TOKEN`, which overrides the credentials file.

| Flag | Adds | For |
| --- | --- | --- |
| `--manage-users` | `manage_users` | Inviting app users, suspending, changing the Signup Policy |
| `--export-data` | `export_data` | Exporting live data |

A delegated token cannot widen itself later, so the flags are the only way the CLI gets these permissions. On a Kernel that verifies email, creating the account sends a link. `--verify-from` reads that link from a development mailbox on this machine and works only with a Kernel on `localhost` or `127.0.0.1`.

## init

```sh
bun run konae -- init --dir ./my-app [--name 'My app'] [--template <id>]
```

Writes an app folder into an empty folder, without a credential or a Kernel: `konae.json`, the template in `app/`, `AGENTS.md`, `action-contract.md`, `actions.d.ts`, `compose.yaml`, `.env.example`, `.gitignore`, `package.json`, `tsconfig.json`, and `scripts/build.mjs`, `scripts/static-artifact.mjs`, `scripts/local-builder.mjs` and `scripts/runtime.js`. The package's `npm run build` writes `dist/`. `.gitignore` keeps `data/`, `.env` and `.konae/` out of Git. The templates are the editor's: `tasks`, `notes` (the default), `members`, `form`, `webhook`, `digest` and `gallery`. Its first `publish` registers the project on the installation.

## dev

```sh
bun run konae -- dev --dir ./my-app [--port 8796] [--events-port 8797] [--reset] [--confirm-schema <confirmation>]
```

Builds `dist/` with the shared local builder and runs the matching `kernel` binary on the folder. The compiled CLI downloads the Kernel release for its version and verifies it against `SHA256SUMS`; set `KONAE_KERNEL_BIN` to use a local binary instead. No Cargo or Rust toolchain is needed. If the app imports npm packages, install the exact dependencies in `package.json` with `npm install` first. The Kernel reads `konae.json`, `app/`, `dist/` and `data/` where they are, with no owner and no link to production. It checks the folder as a release and brings `data/database.sqlite` to what `app/schema.sql` states. A change that loses data is refused with the plan's confirmation; run `dev` again with `--confirm-schema` and that confirmation to apply it. When a file in `app/` changes, `dev` builds again and restarts the Kernel. The app keeps the Signup Policy in `konae.json`. When it is not Open and the app holds no account yet, `dev` invites `test@example.com`, waits up to 15 seconds for the invitation to reach the mailbox and prints `Test account: open <link> and choose a password.` When the app holds an account, as after `--reset` with pulled data or on a second run, it invites no one. For another address it prints the `kernel` binary's `invite` command. Its email, notices and invitations included, goes to `.konae/kernel/mailbox.jsonl`. Files are limited as on a Kernel, 2 MiB each, and `KONAE_FILE_MAX_BYTES` raises that. The quota for all files is 1 TiB unless `KONAE_FILES_QUOTA_BYTES` names another, so `--reset` can import the files of a published app. `--reset` imports pulled data after backing up local `data/`, or empties `data/` when there is no pull. On every start and change it writes `actions.d.ts` beside `konae.json`, which types `miniapp.actions.call`.

## pull

```sh
konae pull --dir ./my-app [--url <installation URL>]
```

Pull exports sources, `dist/`, `konae.json` settings and published data into `.konae/pull/`. It needs `export_data`. It installs the incoming sources and settings only if the local versions match their last recorded values. If they differ, pull stages the published versions under `.konae/pull/` and preserves local edits. The marker records the observed source revision and digest for this installation and project. `dev --reset` imports pulled data after backing up local `data/`. Until that import finishes, `push` refuses. Pull does not change the published app.

## push

```sh
konae push --dir ./my-app [--url <installation URL>] [--yes] [--confirm-schema <confirmation>]
```

Replaces the published app's data with the folder's `data/`, never `.konae/pull/data`. If a pull is pending, push refuses until `dev --reset` imports the pulled data. It goes through `project_import` with `replace` set to `data`. It is the same import `kernel import` runs on a folder, aimed at the app's own Deployment. Rows are never merged. It needs `publish`.

It sends `database.sqlite`, `identity.sqlite` when present, `data/files` when that folder exists, and `usage.sqlite` when present. It never sends `secrets.sqlite`. Without `identity.sqlite` the published accounts stay, and without `data/files` the published files stay. Without `usage.sqlite` the published Usage and Signals are removed, as any import does.

Before it sends anything, `push` reads the installation's file limits from `platform_describe` and checks `data/files/index.json` against them. Every listed file needs its `<id>.blob` beside the index at the indexed size, no file may pass the per-file limit (2 MiB unless the operator raised `KONAE_FILE_MAX_BYTES`), and all files together may not pass the quota (20 MiB unless the operator raised `KONAE_FILES_QUOTA_BYTES`). A refusal names the largest offending file and the totals. A file with an `ownerId` must name an account in `identity.sqlite`. It copies each SQLite file with `VACUUM INTO`, so rows a running `dev` still holds in the write-ahead log go too, and builds the files archive itself with `index.json` first and each blob in index order. Nothing else in `data/files`, such as a `._index.json` that macOS leaves, goes with it.

Without `--yes` it prints the rows of each table here against the published rows, what happens to accounts, files and Usage, and sends nothing. With `--yes` it:

1. puts the app in maintenance, unless it already was;
2. uploads each part in the block size `transfer_begin` answers;
3. runs `project_import`, which backs up the published data first;
4. reads back the backup with `list-backups` and the published row counts with `database_execute` `inspect`;
5. takes the app out of maintenance, also when a step fails or you press Ctrl-C, unless it was in maintenance before `push` began.

It prints the backup's id. `database_execute` with `operation` `restore-backup` and that `backupId` brings the published data back. When the published schema would drop a column or a table that `data/` holds, nothing changes. `push` prints the plan and its confirmation, and `--confirm-schema` with that confirmation applies it. [Bring existing data](./guides/bring-existing-data.md) says how to write `data/` from another source.

## sql

```sh
bun run konae -- sql --dir ./my-app [--prod] [--url <installation URL>] ("<statement>" | --file <file.sql>)
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

Builds `app/` into `dist/` by the same rules as the editor's Publish and the folder's `npm run build`. The CLI and app folder build scripts use the same local builder. A fresh folder needs no install if it imports no npm packages. If `app/package.json` declares an npm package, run `npm install` before building so the package exists in `node_modules`. The editor fetches those exact package versions from esm.sh; those bundles can differ from the local build.

## contract

```sh
bun run konae -- contract --dir ./my-app [--out <folder>]
```

Writes `action-contract.json`, its schema, `action-contract.md` and `actions.d.ts`, from `app/actions.json` and `app/schema.sql`. `actions.d.ts` types `miniapp.actions.call` by action name, input and the columns a query returns. A page written in TypeScript, or a JavaScript page whose `jsconfig.json` sets `checkJs`, reads it when the folder's TypeScript configuration includes it.

## publish

```sh
bun run konae -- publish --dir ./my-app [--url <installation URL>] [--confirm-schema <confirmation>]
```

Builds and publishes to `--url` or the installation `login` saved. Set each signing secret with [`secret set`](#secret) first, because a release that signs with an unset secret is refused. On a first release it sends `signupPolicy` from `konae.json`, and prints the generated address the release fixes, from `release_plan`, before it applies.

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
bun run konae -- domain [--dir <folder>] [--deployment <id>] [--set <hostname> | --clear] [--url <installation URL>]
```

Shows, sets or clears the app's custom domain. Without `--deployment`, it uses the `id` in the `konae.json` of `--dir`, which is the Deployment's ID. It talks to `--url` or the installation `login` saved, as `publish` does, with the token from `<folder>/.konae/credentials.json` or `KONAE_MCP_TOKEN`. `--set` prints the TXT record that proves you control the hostname. Run it again once the record is at your DNS provider. A hostname under the installation's app suffix is refused. Move the generated hostname with `hostname` instead.

## hostname

```sh
bun run konae -- hostname [--dir <folder>] [--deployment <id>] [--set <name>] [--url <installation URL>]
```

Prints the generated address Konae gave the app, or with `--set` moves it to `<name>.<app suffix>` through `hostname_set`. Without `--deployment`, it uses the `id` in the `konae.json` of `--dir`, as `domain` does. The old name redirects to the new one and stays the app's. The command prints the Google callback to register and the Kernel's refusal as it comes. [Give the app an address](./guides/give-the-app-an-address.md#move-the-generated-hostname) has the rules.

## secret

```sh
openssl rand -hex 32 | bun run konae -- secret set <name> --dir <folder> [--use route|call|google] [--url <installation URL>]
bun run konae -- secret list --dir <folder> [--url <installation URL>]
```

`secret set` sets one of the project's secrets through `secret_set` and reads the value from stdin, without the final line break. On a terminal it asks for the value and does not show it. An empty value sets nothing. The value never sits in argv, shell history or an agent's transcript. After the write it reads the name back with `secrets_list` and prints its use and time. `google_client_id` and `google_client_secret` take use `google`, which is their default; any other name defaults to `route`. `secret list` prints each name and use, never a value.

## tool

```sh
bun run konae -- tool <name> ['<json arguments>'] [--stdin <key>] [--dir <folder>] [--url <installation URL>]
```

Calls one tool of the owner's MCP and prints its answer as JSON. The arguments are one JSON object and default to `{}`. `--stdin <key>` reads that one argument from standard input, without its final line break, so a secret's value stays off the command line and out of the shell history. It talks to `--url` or the installation `login` saved, as `publish` does, with the token from `<folder>/.konae/credentials.json` or `KONAE_MCP_TOKEN`. When the tool refuses, it prints the tool's own words and exits with 1. [MCP tools and guide topics](./mcp-tools.md) lists the tools, and `tool session_inspect` shows what this credential may call.

```sh
bun run konae -- tool session_inspect --dir ./my-app
```

## konae.json

`init` writes `konae.json`, and the Kernel reads it wherever it serves the folder. These are the fields and what `publish` does with each.

| Field | `publish` reads it | Changed later by |
| --- | --- | --- |
| `id` | Every time. It names the project and its Deployment. | No tool. |
| `name`, `slug`, `manifest` | Every time, as the project's record. | Edit the file and publish. The hostname is not in this file. `hostname_set` moves it. |
| `visibility` | Every time, with the release plan. | Edit the file and publish. |
| `signupPolicy` | On the first release only. A later publish that names another policy warns and changes nothing. | `signup_policy_set` |
| `contactEmail` | Every time it is set. | `contact_email_set`, until the next publish sends the file's value again. |
| `signInMethods` | Never. | `sign_in_methods_set` |
| `kernel` | Never. It is the version of the Kernel that wrote the file. | The Kernel. |

## Read next

- [Edit files with the CLI](./guides/edit-files-with-the-cli.md)
- [Take the app elsewhere](./guides/take-the-app-elsewhere.md)
- [MCP tools and guide topics](./mcp-tools.md)
