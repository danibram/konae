# CLI

The `konae` command makes an app in a folder, runs it on a Development Kernel, publishes it, and moves it. Run it from the repository root with Bun.

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
bun run konae -- init --dir ./my-app [--name 'My app'] [--template <id>] [--url <Kernel URL>]
```

Writes a template and `konae.json` into an empty folder, without a credential or a Kernel. The templates are the editor's: `tasks`, `notes` (the default), `members`, `form`, `webhook`, `digest` and `gallery`. Its first `publish` registers the project on the installation.

## dev

```sh
bun run konae -- dev --dir ./my-app [--port 8796] [--events-port 8797] [--reset]
```

Compiles and starts the Rust Kernel with data under `.konae/dev-data`, with its own identities and no link to production. It publishes the build there, sets the signup policy to Open so test accounts can sign up, and rebuilds when the sources change. Its email, notices included, goes to `.konae/dev-data/mailbox.jsonl`. When the command exits, the Kernel stops and the data stays in the folder. `--reset` deletes that data first. A Development Kernel with no data starts from the copy `pull` left, when there is one.

## pull

```sh
bun run konae -- pull --dir ./my-app [--url <Kernel URL>]
```

Downloads the published app's database, accounts and files into `.konae/pull`. Needs a credential from `login --export-data`. Run `dev --reset` to start the local app from the copy. Nothing travels back to the published app.

## sql

```sh
bun run konae -- sql --dir ./my-app [--prod] [--url <Kernel URL>] ("<statement>" | --file <file.sql>)
```

Runs SQL on the app's database through `database_execute`, on the Development Kernel that `dev` is running for the folder. That is where to rehearse a schema change on a pulled copy. Only `--prod` reaches the published Deployment.

## events

```sh
bun run konae -- events --dir ./my-app [--follow] [--json] [--days 7] [--prod]
```

Lists what the app did besides answer its page: email, calls to other services, scheduled runs, routes and Signals, oldest first. On the Development Kernel an email shows its recipient and text, because nothing is sent. `--json` prints one object per line, `--follow` keeps printing, and only `--prod` reads the published Deployment. While `dev` runs, the same list is a page on the port after the app's.

## build

```sh
bun run konae -- build --dir ./my-app
```

Writes `.konae/artifact.json`.

## contract

```sh
bun run konae -- contract --dir ./my-app [--out <folder>]
```

Writes `action-contract.json`, its schema and `action-contract.md`, from `actions.json` and `schema.sql`.

## publish

```sh
bun run konae -- publish --dir ./my-app [--url <Kernel URL>]
```

Builds and publishes to the installation `konae.json` names, or `--url`, or the one `login` saved. Set each signing secret with `secret_set` first, because a release that signs with an unset secret is refused. On a first release it sends `project.manifest.signupPolicy` from `konae.json`.

## conform

```sh
bun run konae -- conform --url <app URL> --contract ./my-app/action-contract.json [--token <app session>] [--key <API key>] [--secret <name>=<value>] [--inputs <file>]
```

Calls every action and checks the answers against the contract. It runs each mutation once, so point it at a test Deployment.

## export

```sh
bun run konae -- export --dir ./my-app [--out <file>] [--with-data]
```

Writes a self-hosted kit. `--with-data` adds the live package and needs a credential from `login --export-data`.

## domain

```sh
bun run konae -- domain --dir <folder> --deployment <id> [--set <hostname> | --clear] [--url <Kernel URL>]
```

Shows, sets or clears the app's custom domain. It reads the token from `<folder>/.konae/credentials.json`, or from `KONAE_MCP_TOKEN` when that is set. `--set` prints the TXT record that proves you control the hostname. Run it again once the record is at your DNS provider.

## Read next

- [Edit files with the CLI](./guides/edit-files-with-the-cli.md)
- [Take the app elsewhere](./guides/take-the-app-elsewhere.md)
- [MCP tools and guide topics](./mcp-tools.md)
