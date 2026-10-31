# Edit the app's files with the CLI

Create a folder with `init`, run it locally with `dev`, then log in and send it to the Kernel with `publish`.

## Before you start

`init` and `dev` need nothing but a checkout of the repository. To publish you need a Konae installation to talk to, and an account on it. `login` reads `KONAE_EMAIL` and `KONAE_PASSWORD`. To create the account in the same step, also set `KONAE_INVITATION_CODE`, the code the operator issued, or `KONAE_SETUP_KEY` for the installation's first owner. The commands below run from a checkout of the repository.

## Steps

1. Create the project. `init` needs an empty folder and no credential. It writes a template and a `konae.json` with a project ID it makes itself. It writes the app private, with signup policy Invitation only.

    ```sh
    bun run konae -- init --dir /tmp/my-app --name 'My app'
    ```

2. Run it locally. `dev` starts a Development Kernel with its own identities, data and files under `.konae/dev-data`, and republishes each time you save a file. It publishes locally as Open so test accounts can sign up, and it never proxies production. The default port is 8796.

    ```sh
    bun run konae -- dev --dir /tmp/my-app
    ```

3. Edit the Capsule files in the folder. Open the URL `dev` prints and reload after each change. Email, notices included, goes to `.konae/dev-data/mailbox.jsonl`.
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

6. Publish. This builds the folder and publishes it with the credential from `login`, to the installation `login` saved. The first publish registers the project there and writes the address into `konae.json`. It prints the Deployment URL.

    ```sh
    bun run konae -- publish --dir /tmp/my-app
    ```

7. Open the Deployment URL and call one Server Action.

## Visibility and signup

`publish` deploys the app as private unless `konae.json` sets `project.manifest.visibility` to `"public"`. `project.manifest.signupPolicy` names who may create an app account on the first release. A later publish that names another policy warns and changes nothing. Change it in the editor's Auth > Users view or with `signup_policy_set`.

## Check it worked

Do not treat the URL `publish` printed as proof. Read the Deployment back through MCP, the same door the CLI wrote through.

- The read-back for `release_apply` is `project_inspect`. It shows the live Deployment and its URL.
- The read-back for `project_create` is `project_inspect` as well.
- Open the Deployment URL, then call one Server Action. `actions_invoke` is its own read-back, so call a query to see a row you wrote.
- Decide what your credential may do from `session_inspect`'s `can` map, not from memory of which flag `login` took.

## What differs from the editor preview

`dev` runs the same Rust Kernel as a Deployment, so HTTP routes, the schedule timer, notice email, Usage and the call outbox all work there. The editor preview has none of them. The Validation Gate checks the schema and `actions.json` only, so a page that names a missing file fails at build or publish.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [CLI reference](../cli.md)
- [The Capsule](../capsule.md)
