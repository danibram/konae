# Edit the app's files with the CLI

Create a folder with `init`, run it locally with `dev`, then log in and send it to the Kernel with `publish`.

## Before you start

`init` needs a checkout of the repository and Bun. `dev` also compiles the Kernel with Cargo, which takes about a minute the first time. To publish you need a Konae installation to talk to, and an account on it. `login` reads `KONAE_EMAIL` and `KONAE_PASSWORD`, and asks for either one that is unset when it runs in a terminal. To create the account in the same step, also set `KONAE_SETUP_KEY` for the installation's first owner, or `KONAE_INVITATION_CODE`, the code the operator issued, for a later one. The commands below run from a checkout of the repository.

## Steps

1. Create the app folder. `init` needs an empty folder and no credential. It writes the template into `app/`, a `konae.json` with a project ID it makes itself, `AGENTS.md` for a coding agent, `action-contract.md`, `actions.d.ts`, `compose.yaml`, `.env.example` and a `package.json` whose `npm run build` writes `dist/` with the scripts in `scripts/`. The Signup Policy is Invitation only. The app is private unless its template is made for visitors, as `tasks`, `form` and `gallery` are. Without `--template`, `init` writes `notes`, which is private, so its first `dev` serves a sign-in page, and it invites a test account for you, as step 2 says. `tasks` is public, so anyone with the link can use it.

    ```sh
    bun run konae -- init --dir /tmp/my-app --name 'My app' --template tasks
    ```

2. Run it locally. `dev` builds `dist/` and runs the `kernel` binary on the folder, with the folder's `data/`, and starts it again each time you save a file in `app/`. It never proxies production. It prints the app's address, `http://<slug>.localhost:8796` by default, and a page of what the app did on the next port. When the app is not open and holds no account, `dev` invites `test@example.com` and prints `Test account: open <link> and choose a password.` Open that link to create the account. The first run compiles the Kernel and says so.

    ```sh
    bun run konae -- dev --dir /tmp/my-app
    ```

3. Edit the Capsule files in `app/`. Open the URL `dev` prints and reload after each change. Email, invitations and notices included, goes to `.konae/kernel/mailbox.jsonl`, and `bun run konae -- events --dir /tmp/my-app` lists it.
4. Optional. Write the Action Contract and, against a test Deployment, check every action's answers.

    ```sh
    bun run konae -- contract --dir /tmp/my-app
    bun run konae -- conform --url <app URL> --contract /tmp/my-app/action-contract.json
    ```

    `conform` runs each mutation once, so point it at a test Deployment.
5. Log in. This saves a 24-hour token with `read`, `write` and `publish` in `<folder>/.konae/credentials.json`, and the installation's address. `--url` is the editor's origin. On a fresh local install, `KONAE_SETUP_KEY=local-development-key` creates the first owner. A reachable installation needs its operator's key, or an invitation code in `KONAE_INVITATION_CODE`. A token cannot widen itself, so ask for `--manage-users` and `--export-data` now, or a second login comes before `pull`, `export --with-data`, `signup_policy_set` or `app_invitations_create`. Keep `.konae` out of Git. `KONAE_MCP_TOKEN` overrides the file. In a terminal, `login` asks for `KONAE_EMAIL` or `KONAE_PASSWORD` when it is unset, and does not echo the password. The form below is for scripts, which have no terminal and fail without both.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... KONAE_SETUP_KEY=local-development-key \
    bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 --manage-users --export-data
    ```

6. Publish. This builds the folder and publishes it with the credential from `login`, to the installation `login` saved. The first publish registers the project there. It prints the Deployment URL. The project appears on the editor's project list marked **Edited in a folder**, because its files live in this folder. Its card opens its database, its app accounts and what it did. New project in the editor shows these commands under **Work from a folder instead**. The editor keeps the sources it saved and does not see the folder's changes. When it holds other files than the published app runs, its Deploy dialog says so, and Deploy from there publishes the editor's sources over the folder's release. To bring the folder back into the editor, choose Import in App settings, under **More**, with a tar of the folder's export, or call `project_import` with `replace` set to `sources`.

    ```sh
    bun run konae -- publish --dir /tmp/my-app
    ```

7. Open the Deployment URL and call one Server Action.

## Visibility and signup

`publish` deploys the app as private unless `konae.json` sets `visibility` to `"public"`. `signupPolicy` in the same file names who may create an app account on the first release. A later publish that names another policy warns and changes nothing. Both tools below are owner tools that need `manage_users`, so log in with `--manage-users`. `deploymentId` is the `id` in `konae.json`. `mode` is `open`, `invitation`, `closed` or `allowlist`, and `allowlist` takes exact emails or `@domain` entries.

```sh
bun run konae -- tool signup_policy_set '{"deploymentId":"app_...","mode":"open"}' --dir /tmp/my-app
bun run konae -- tool app_invitations_create '{"deploymentId":"app_...","emails":["ana@example.com"],"idempotencyKey":"first-invite"}' --dir /tmp/my-app
```

`idempotencyKey` is 8 to 128 characters, and a retry with the same key sends nothing again.

## Check it worked

Do not treat the URL `publish` printed as proof. Read the Deployment back through MCP, the same door the CLI wrote through.

- The read-back for `release_apply` is `project_inspect`. It shows the live Deployment and its URL.
- The read-back for `project_create` is `project_inspect` as well.
- Open the Deployment URL, then call one Server Action. `actions_invoke` is its own read-back, so call a query to see a row you wrote.
- Decide what your credential may do from `session_inspect`'s `can` map, not from memory of which flag `login` took.

## What differs from the editor preview

`dev` runs the same Kernel as a Deployment, so HTTP routes, the schedule timer, notice email, Usage and the call outbox all work there. The editor preview has none of them. The Validation Gate checks the schema and `actions.json` only, so a page that names a missing file fails at build or publish.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [CLI reference](../cli.md)
- [The Capsule](../capsule.md)
