# Konae

Konae is a self-hosted workshop for small web apps. You describe an app to an agent or edit its files yourself, and Konae publishes it with a SQLite database, accounts and file uploads already attached. The app can then move to another machine with its accounts, its data and its rules unchanged.

It is built for two people making the same app, one who does not write code and one who does. Both pass the same checks, so a change the editor accepts is also accepted by the server the app later leaves on.

## What it does

- **An editor in the browser.** A project is a tree of files with a live preview. The build runs in the browser with `esbuild-wasm`, so the server never runs project code.
- **A build agent.** It works on a copy of the project with your own provider key (Anthropic, OpenAI or OpenRouter), which stays in the browser. Every change it proposes must pass the Validation Gate before it reaches the project.
- **Server Actions instead of SQL in the browser.** An app declares named, parameterized actions in `actions.json`. A published page knows action names and the fields they return, never SQL or database credentials. An action can also refuse a write with a guard, send an email, run on a schedule, answer an HTTP route for a webhook, or call another service.
- **Accounts per app.** Each app has its own users, sign-in page and signup policy, which is Open, Invitation only, Closed or an Allowlist. The owner invites people by email and can add Google sign-in with the app's own Google client.
- **A Kernel that serves the apps.** One Rust server holds every published app as one folder with its static files, SQLite database, accounts, uploads and backups. It serves each app on its own hostname, on a custom domain, or on a path of the installation's domain.
- **Two ways to work.** The editor is one. The `konae` CLI is the other, with `init`, `dev` and `publish` run from a folder of files in your own Git repository. Agents operate the platform over authenticated MCP, with the same tools the editor calls.
- **A way out.** An owner exports an app as one folder, `konae.json`, its sources, its built files and its database, accounts and files, and imports it on another Kernel.

[The docs overview](./docs/README.md) has the five jobs Konae is measured against, and [Concepts](./docs/concepts.md) defines the words.

## How it fits together

```text
editor (React, in the browser)          konae CLI (a folder of files)
  build agent -> Validation Gate          konae build
        \                                   /
         +-------- authenticated MCP ------+
                         |
                  Kernel (Rust)
   one folder per app: artifact, SQLite, accounts, files
                         |
        visitors and app users, over HTTPS
```

A Capsule is the source of one app: pages, an entrypoint, `schema.sql` and `actions.json`. A build turns it into a static Build Artifact. The Kernel checks the same schema and actions again at release and serves the result as a Deployment. [The architecture](./docs/architecture.md) describes each part and the security boundaries.

## Prerequisites

- [Bun](https://bun.sh) 1.3 or newer. The Docker images pin Bun 1.3.11.
- A Rust toolchain with Cargo, version 1.91 or newer.
- Google Chrome for the Playwright suites, which run with `channel: "chrome"`.
- Docker with Compose, only for the full publishing stack and the MinIO end-to-end test.

## Run it locally

```bash
git clone https://github.com/danibram/konae
cd konae
bun install
bun run dev:local
```

This starts the Rust Kernel on port `8790` and the editor on port `4173`. Open [http://localhost:4173](http://localhost:4173) and create the first owner in **Platform access** with the setup key `local-development-key`. Local data stays under `data/kernel-local`, and `KONAE_DATA_DIR` moves it.

`docker compose up --build` runs the complete publishing stack, with a gateway and S3 storage, at [http://app.localhost:8080](http://app.localhost:8080). [Operate an installation](./docs/operate.md) covers it, along with every environment variable, the data directory, copies and restores, email, and what to set before a Kernel is reachable from outside.

## Make a first app

In the editor, create a project from a template, ask the agent for a change or edit a file, and publish. [First app from the editor](./docs/guides/first-app-from-the-editor.md) walks through it.

From a folder, with the CLI. `init` and `dev` need no credential and no installation:

```sh
bun run konae -- init --dir /tmp/my-app --name "My app"
bun run konae -- dev --dir /tmp/my-app
```

`dev` runs the app on this machine with the `kernel` binary, on the folder's own `data/`. To publish, `login` saves a credential and the installation's address in the folder, and the first `publish` registers the project there. `init` writes the app folder: `konae.json`, the sources in `app/`, `AGENTS.md`, `action-contract.md` and a `package.json` whose `npm run build` writes `dist/`:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 --export-data
bun run konae -- publish --dir /tmp/my-app
```

`publish` deploys the app as private unless `konae.json` says otherwise. To debug against what the published app holds, or to try the next version on it, `pull` exports its database, accounts and files to `.konae/pull` and `dev --reset` imports them into the local Kernel. Nothing travels back:

```sh
bun run konae -- pull --dir /tmp/my-app
bun run konae -- dev --dir /tmp/my-app --reset
```

[Edit files with the CLI](./docs/guides/edit-files-with-the-cli.md) explains each step, and [the CLI reference](./docs/cli.md) covers `sql` and `events` too. `bun link` in this repository puts `konae` on your path, so the commands also run as `konae dev` from any folder.

An app's page calls the SDK the build adds:

```js
const session = await miniapp.auth.session();
const rows = await miniapp.actions.call("tasks.list");
await miniapp.actions.call("tasks.create", { title: "Ship it" });
const files = await miniapp.files.list();
```

## Documentation

The documentation is markdown under [`docs/`](./docs/README.md). The docs site is itself a Konae app, [`examples/docs-site`](./examples/docs-site/README.md), which publishes those files and renders them in the browser.

| To | Read |
| --- | --- |
| Learn the vocabulary | [Concepts](./docs/concepts.md) |
| Write an app | [The Capsule](./docs/capsule.md), [Server Actions](./docs/server-actions.md), [the SDK](./docs/sdk.md) |
| Decide who gets in | [Accounts and access](./docs/accounts-and-access.md) |
| Do one task | The guides in [`docs/guides/`](./docs/guides/) |
| Drive it from an agent or a script | [MCP tools](./docs/mcp-tools.md), [the MCP guide](./docs/mcp.md), [the walkthrough](./docs/mcp-walkthrough.md), [the CLI](./docs/cli.md) |
| Know the numbers and the gaps | [Limits](./docs/limits.md), [Named divergences](./docs/divergences.md) |
| Operate a server | [Operate an installation](./docs/operate.md), [the architecture](./docs/architecture.md) |
| See why it is built this way | [The decision records](./docs/adr/README.md) |

`CONTEXT.md` holds the project's language and `AGENTS.md` the rules for agents that work on this repository.

## Examples

- [`examples/showcase`](./examples/showcase) has six complete apps, each with a replay that publishes it and runs every flow. `starter` is the one to copy first.
- [`examples/product-fit`](./examples/product-fit/README.md) has eight smaller apps and a smoke run.
- [`examples/probes`](./examples/probes/README.md) has small Capsules that each prove one capability.
- [`examples/docs-site`](./examples/docs-site/README.md) is the docs site.
- [`examples/home`](./examples/home/README.md) is the page an installation answers on its bare domain once an app's path is named `konae-home`.
- [`examples/architecture-map`](./examples/architecture-map/README.md) is a static map of the system.

## Tests

```bash
bun test
bun run build
cargo test --manifest-path kernel-rs/Cargo.toml
bun run test:e2e:local
# Requires the Docker stack
bun run test:e2e:minio
```

## Status

Konae runs as a self-hosted installation and has no tagged release yet. These are missing today:

- A production recipe. The repository ships a local stack, and [Operate an installation](./docs/operate.md#before-a-kernel-is-reachable) lists what an operator sets, but TLS and the proxy are the operator's own.
- Teams. A project has one owner, and Workspace membership is deferred.
- Release history. Publishing replaces the app's files, with no data-aware rollback.
- Quotas shown to creators, alerts and account closing.

The Kernel rejects `granted` actions until it supports explicit deployment grants. File bytes in a bucket are tested against SeaweedFS only. [docs/plans](./docs/plans) and [docs/research](./docs/research) hold the working notes behind these.

## License

Konae is released under the MIT License. See [LICENSE](./LICENSE).
