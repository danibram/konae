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

The examples use `bun run konae --` from a repository checkout. There, `dev` uses `kernel-rs/target/debug/kernel` and builds it with Cargo if it is absent. The compiled CLI downloads the Kernel binary for its version and verifies the checksum in `SHA256SUMS`. Set `KONAE_KERNEL_BIN` to use a local Kernel with either CLI. With the compiled binary on PATH, remove `bun run` and the separator `--`.

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

Writes an app folder into an empty folder, without a credential or a Kernel: `konae.json`, the template in `app/`, `AGENTS.md`, `action-contract.md`, `actions.d.ts`, `compose.yaml`, `.env.example`, `.gitignore`, `package.json`, `tsconfig.json`, and `scripts/build.mjs`, `scripts/static-artifact.mjs`, `scripts/local-builder.mjs` and `scripts/runtime.js`. The shared local builder runs the package's `npm run build` when a build script and `node_modules/` are present; otherwise it uses its built-in builder. Install imported npm packages before building. `build` creates `dist/`; `init` does not create `data/`. `.gitignore` keeps `data/`, `.env` and `.konae/` out of Git. The templates are the editor's: `tasks`, `notes` (the default), `members`, `form`, `webhook`, `digest` and `gallery`. Its first `publish` registers the project on the installation.

## dev

```sh
bun run konae -- dev --dir ./my-app [--port 8796] [--events-port 8797] [--reset] [--confirm-schema <confirmation>]
```

Builds `dist/` with the shared local builder and runs the matching `kernel` binary on the folder. If the folder has a `package.json` build script and `node_modules/`, the local builder runs `npm run build`; otherwise it uses its built-in builder. An app that imports npm packages needs the exact versions from `package.json` installed with `npm install` first. From a checkout, `dev` uses that checkout's Kernel binary and builds it with Cargo if absent. The compiled CLI downloads the Kernel release for its version and verifies it against `SHA256SUMS`. Set `KONAE_KERNEL_BIN` to use a local binary. The Kernel reads `konae.json`, `app/`, `dist/` and `data/` where they are, with no owner and no link to production. It checks the folder as a release and brings `data/database.sqlite` to what `app/schema.sql` states. A change that loses data is refused with the plan's confirmation; run `dev` again with `--confirm-schema` and that confirmation to apply it. When a file in `app/` changes, `dev` builds again and restarts the Kernel. The app keeps the Signup Policy in `konae.json`. When it is not Open and the app holds no account yet, `dev` invites `test@example.com`, waits up to 15 seconds for the invitation to reach the mailbox and prints `Test account: open <link> and choose a password.` When the app holds an account, as after `--reset` with pulled data or on a second run, it invites no one. For another address it prints the `kernel` binary's `invite` command. Its email, notices and invitations included, go to `.konae/kernel/mail-testbox.jsonl`. Files are limited as on a Kernel, 2 MiB each, and `KONAE_FILE_MAX_BYTES` raises that. The quota for all files is 1 TiB unless `KONAE_FILES_QUOTA_BYTES` names another, so `--reset` can import the files of a published app. `--reset` imports pulled data after backing up local `data/`, or empties `data/` when there is no pull. On every start and change it writes `actions.d.ts` beside `konae.json`, which types `miniapp.actions.call`.

Every `dev` run uses local test mode. The Kernel binds to loopback, uses the folder's `data/` and local file store, captures call actions without resolving or contacting their targets, and pauses schedule timers. Google sign-in is refused. The browser's `connect-src` allows only the app's own origin, even when `konae.json` declares `manifest.connects`. Test mode does not sandbox npm build scripts or other commands you run from the shell.

`dev --reset` acquires the folder's lifecycle lock and checks that no local Kernel is still serving the app. Before it imports pulled data, or empties local data when no pull exists, it backs up `data/` and the local call and mail queues under `.konae/backups/local-reset-<executionId>`. The command retires pending calls and email jobs after the data change. Its result names the backup, reset and snapshot IDs, and the retired call and email counts. If reset fails, it restores the data and queues from the backup. Stop the running `dev` process before another reset.

## pull

```sh
konae pull --dir ./my-app [--data-only] [--url <installation URL>]
```

Pull exports sources, `dist/`, `konae.json` settings and published data into `.konae/pull/`. It needs `export_data`. A clean pull installs `app/` and `konae.json`, while keeping `dist/` staged. If local sources or settings differ from their last recorded values, pull stages the incoming app, artifact and settings and preserves local edits. Preserve local `app/` and `konae.json` in a backup, copy the staged app and settings into place unchanged, then pull again to record the observed revision. Reapply or merge the saved local edits after that, before publishing. The revision is scoped to this installation and project. A pending source revision blocks `publish`; pending pulled data separately blocks `push` until `dev --reset` imports it after a backup. The source folder can be ahead of the applied rules in `konae.json.released`. Pull does not change the published app. [Export and import](./reference/export-and-import.md#export-and-import-an-app) gives a concrete staged-file recovery path and explains the separate data import.

`pull --data-only` refreshes the data snapshot while preserving local `app/`, `konae.json` and the recorded source baseline. The downloaded snapshot stays under `.konae/pull/`; `dev --reset` imports it into `data/` after a backup. `rehearse` checks the snapshot without changing local `data/`.

## test

```sh
bun run konae -- test --dir ./my-app status
bun run konae -- test --dir ./my-app calls
bun run konae -- test --dir ./my-app schedule <action>
bun run konae -- test --dir ./my-app reply <call-id> --stdin
bun run konae -- test --dir ./my-app scenario tests/payments.json
```

These commands require a local `dev` Kernel with test controls. `status` reports the mode and paused schedules. `calls` lists calls awaiting a reply, calls waiting for retry backoff with `nextAttemptAt`, and continuations whose outcome was interrupted. `schedule` runs one declared schedule mutation through the Kernel's normal action path.

`reply` reads JSON from stdin or `--file`. A response names the `responseNumber` shown by `calls` and supplies either an HTTP `status` with a JSON `body`, or a transport `error`:

```json
{"responseNumber":1,"status":200,"body":{"id":"receipt-42"}}
```

HTTP 408, 425, 429 and 5xx responses, plus retryable transport errors, use the Kernel's retry schedule. A call gets at most five attempts. Retry waits are 30, 60, 120 and 240 seconds. Reply again only after `calls` shows the next `responseNumber` as awaiting. A duplicate, stale response number, or call ID from another app leaves the outbox unchanged.

A version 1 scenario file matches each response to exactly one awaiting call by action name and a subset of its call record. Each step supplies one response:

```json
{
  "version": 1,
  "responses": [
    {
      "action": "orders.send",
      "match": {"input": {"orderId": "order-42"}},
      "response": {"responseNumber": 1, "status": 200, "body": {"id": "receipt-42"}}
    }
  ]
}
```

Store scenario files under the app folder's `tests/`. A scenario refuses to guess when a step matches zero or multiple calls, or names a stale response number. For retry sequences, inspect `nextAttemptAt` and submit each response number after the call becomes awaiting again.

The page `dev` prints on its next port shows local email, calls, schedule runs, routes and Signals. Its schedule and reply controls use a random token in the URL fragment, check the loopback host and request origin, and require the token in a request header. `konae events --dir ./my-app` reads the same local records. Neither command targets production.

An interrupted continuation has `then.state: "interrupted"` because the Kernel cannot know whether its mutation committed before a process stopped. The Kernel does not replay it. Reset local data and retest the workflow before relying on the result.

## rehearse

```sh
bun run konae -- rehearse --dir ./my-app [--confirm-schema <confirmation>]
```

Rehearse builds a fresh `dist/` from the current `app/`, then asks a scratch Kernel to import the immutable data snapshot under `.konae/pull/`. It checks the import against the candidate sources and settings, reports candidate and baseline source digests, and deletes the scratch folder. It does not read hand-fixed local `data/` or change the published app. A schema refusal may name a confirmation for this snapshot. Pass that value with `--confirm-schema`; the later production plan can name a different confirmation because it reads the current published rows.

`konae schema fingerprint --dir ./my-app` prints `sha256:<64 lowercase hex characters>` for `app/schema.sql`. Add `--data` to read `data/database.sqlite` read-only, or `--json` to return `{ "fingerprint": "sha256:<64 lowercase hex characters>", "source": "schema" | "data" }`. It does not start the app or create `.konae/`. The Kernel command accepts a direct path: `kernel schema fingerprint --schema <path>` or `kernel schema fingerprint --database <path>`.

[Schema transformations](./guides/schema-transformations.md) explains how to fingerprint each schema and record an explicit transition chain.

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

`publish` sends the current `app/` sources and built `dist/`. It applies the schema change to the rows currently on the Deployment. It never uploads local `data/`. Use `konae push --yes` only when the owner explicitly wants to replace published data with the folder's data after a backup.

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
| `contactEmail` | On the first release only. Later publishes keep the Deployment's address and warn when the folder names another one. | `contact_email_set` |
| `signInMethods` | Never. | `sign_in_methods_set` |
| `kernel` | Never. It is the version of the Kernel that wrote the file. | The Kernel. |

## Read next

- [Edit files with the CLI](./guides/edit-files-with-the-cli.md)
- [Take the app elsewhere](./guides/take-the-app-elsewhere.md)
- [MCP tools and guide topics](./mcp-tools.md)
