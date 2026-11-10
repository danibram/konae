# Edit the app's files with the CLI

Create a folder with `init`, run it locally with `dev`, then log in and send it to the Kernel with `publish`.

## Before you start

`init` and `dev` need nothing but a checkout of the repository. To publish you need a Konae installation to talk to, and an account on it. `login` reads `KONAE_EMAIL` and `KONAE_PASSWORD`. To create the account in the same step, also set `KONAE_INVITATION_CODE`, the code the operator issued, or `KONAE_SETUP_KEY` for the installation's first owner. The commands below run from a checkout of the repository.

## Steps

1. Create the app folder. `init` needs an empty folder and no credential. It writes the template into `app/`, a `konae.json` with a project ID it makes itself, `AGENTS.md` for a coding agent, `action-contract.md`, `actions.d.ts`, `compose.yaml`, `.env.example` and a `package.json` whose `npm run build` writes `dist/`. The Signup Policy is Invitation only. The app is private unless its template is made for visitors, as `tasks`, `form` and `gallery` are.

    ```sh
    bun run konae -- init --dir /tmp/my-app --name 'My app'
    ```

2. Run it locally. `dev` builds `dist/` and runs the `kernel` binary on the folder, with the folder's `data/`, and starts it again each time you save a file in `app/`. It never proxies production. It prints the app's address, `http://<slug>.localhost:8796` by default, the `kernel invite` command that creates a test account, and a page of what the app did on the next port.

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
5. Log in. This saves a 24-hour token with `read`, `write` and `publish` in `<folder>/.konae/credentials.json`. Keep `.konae` out of Git. `KONAE_MCP_TOKEN` overrides the file.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir /tmp/my-app --url http://localhost:4173
    ```

6. Publish. This builds the folder and publishes it with the credential from `login`, to the installation `login` saved. The first publish registers the project there. It prints the Deployment URL. The project appears on the editor's project list under **Apps without editor files**, because its files live in this folder.

    ```sh
    bun run konae -- publish --dir /tmp/my-app
    ```

7. Open the Deployment URL and call one Server Action.

## Visibility and signup

`publish` deploys the app as private unless `konae.json` sets `visibility` to `"public"`. `signupPolicy` in the same file names who may create an app account on the first release. A later publish that names another policy warns and changes nothing. Change it with `signup_policy_set`, and invite app users to the published app with `app_invitations_create`. Both are owner tools that need `manage_users`, so log in with `--manage-users` first.

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
