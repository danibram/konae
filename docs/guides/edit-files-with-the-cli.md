# Edit the app's files with the CLI

Create a folder with `init`, run it locally with `dev`, then log in and send it to the Kernel with `publish`.

## Before you start

Build the compiled CLI with `bun run scripts/build-konae-binary.ts`, then copy `out/konae-<version>-<os>-<arch>` to a directory on PATH. The published v0.8.0 release includes four Kernel binaries and `SHA256SUMS`, but no CLI binary. The CLI downloads its matching Kernel and verifies the checksum. The compiled macOS arm64 CLI was verified without Bun, Rust or repository reads, and requires macOS 13.0. The other CLI platforms and release workflow remain unverified. To publish you need a Konae installation to talk to, and an account on it. `login` reads `KONAE_EMAIL` and `KONAE_PASSWORD`, and asks for either one that is unset when it runs in a terminal. Required verification asks for email first and the verification page asks the person to choose a password. To create the account in the same step, also set `KONAE_SETUP_KEY` for the installation's first owner, or `KONAE_INVITATION_CODE`, the code the operator issued, for a later one. The examples below run from a repository checkout, so `bun run konae -- dev` uses the checkout Kernel and builds it with Cargo if its binary is absent. The compiled CLI instead downloads its version-matched Kernel and verifies the `SHA256SUMS` entry. Set `KONAE_KERNEL_BIN` to select a local binary with either form.

## Steps

1. Create the app folder. `init` needs an empty folder and no credential. It writes the template into `app/`, `konae.json`, `.gitignore`, `AGENTS.md`, `action-contract.md`, `actions.d.ts`, `compose.yaml`, `.env.example`, `package.json`, `tsconfig.json` and build scripts under `scripts/`. The local builder runs `npm run build` when the folder has a package build script and `node_modules/`; otherwise it uses the built-in builder. An imported npm package must be installed before either path. `build` creates `dist/`; `init` does not create `data/`. The Signup Policy is Invitation only. The app is private unless its template is made for visitors, as `tasks`, `form` and `gallery` are. Without `--template`, `init` writes `notes`, which is private, so its first `dev` serves a sign-in page, and it invites a test account for you, as step 2 says. `tasks` is public, so anyone with the link can use it.

    ```sh
    bun run konae -- init --dir /tmp/my-app --name 'My app' --template tasks
    ```

2. Run it locally. `dev` builds `dist/` with the shared local builder and runs the matching `kernel` binary on the folder, with the folder's `data/`. From a repository checkout it uses that checkout's Kernel and builds it with Cargo only if the binary is absent. The compiled CLI downloads the version-matched Kernel and verifies it against `SHA256SUMS`, unless `KONAE_KERNEL_BIN` names a local binary. It never proxies production. It prints the app address, `http://<slug>.localhost:8796` by default, and an events page on the next port. When the app has no account and its Signup Policy is not Open, `dev` invites `test@example.com` and prints the invitation link.

    ```sh
    bun run konae -- dev --dir /tmp/my-app
    ```

3. Edit the Capsule files in `app/`. Open the URL `dev` prints and reload after each change. Email, invitations and notices included, goes to `.konae/kernel/mail-testbox.jsonl`, and `bun run konae -- events --dir /tmp/my-app` lists it.
4. Optional. Write the Action Contract and, against a test Deployment, check every action's answers.

    ```sh
    bun run konae -- contract --dir /tmp/my-app
    bun run konae -- conform --url <app URL> --contract /tmp/my-app/action-contract.json
    ```

    `conform` runs each mutation once, so point it at a test Deployment.
5. Log in. This saves a 24-hour token with `read`, `write` and `publish` in `<folder>/.konae/credentials.json`, and the installation's address. `--url` is the editor's origin. On a fresh local install, `KONAE_SETUP_KEY=local-development-key` creates the first owner. A reachable installation needs its operator's key, or an invitation code in `KONAE_INVITATION_CODE`. A token cannot widen itself, so ask for `--manage-users` and `--export-data` now, or a second login comes before `pull`, `export --with-data`, `signup_policy_set` or `app_invitations_create`. Keep `.konae` out of Git. `KONAE_MCP_TOKEN` overrides the file. In a terminal, `login` asks for `KONAE_EMAIL` or `KONAE_PASSWORD` when it is unset, and does not echo the password. The form below is for scripts, which have no terminal and fail without both. `login --browser` instead prints an address the owner opens in a browser on the same machine, so no password passes through the terminal or an agent.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... KONAE_SETUP_KEY=local-development-key \
    bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 --manage-users --export-data
    ```

6. Publish. This builds the folder and publishes it with the credential from `login`, to the installation `login` saved. The first publish registers the project there. It prints the Deployment URL. The project opens in the editor with the same `app/` files the folder uses. The project card reads metadata without downloading file bytes; opening it fetches that project's sources. After a folder publish, the editor reads back the released files and advances the observed revision. A later publish refuses if the source revision changed, including a save after release planning.

    ```sh
    bun run konae -- publish --dir /tmp/my-app
    ```

7. Open the Deployment URL and call one Server Action.

## Visibility and signup

`publish` deploys the app as private unless `konae.json` sets `visibility` to `"public"`. `signupPolicy` in the same file names who may create an app account on the first release. A later publish that names another policy warns and changes nothing. Both operations below are owner tools that need `manage_users`, so log in with `--manage-users`. `projectId` is the `id` in `konae.json`. `mode` is `open`, `invitation`, `closed` or `allowlist`, and `allowlist` takes exact emails or `@domain` entries.

```sh
bun run konae -- tool signup_policy_set '{"projectId":"app_...","mode":"open"}' --dir /tmp/my-app
bun run konae -- tool app_invitations_create '{"projectId":"app_...","emails":["ana@example.com"],"idempotencyKey":"first-invite"}' --dir /tmp/my-app
```

`idempotencyKey` is 8 to 128 characters, and a retry with the same key sends nothing again.

## Check it worked

Do not treat the URL `publish` printed as proof. Read the Deployment back through MCP, the same door the CLI wrote through.

- The read-back for `release_apply` is `project_inspect`. It shows the live Deployment and its URL.
- The read-back for `project_create` is `project_inspect` as well.
- Open the Deployment URL, then call one Server Action. `actions_invoke` is its own read-back, so call a query to see a row you wrote.
- Decide what your credential may do from `session_inspect`'s `can` map, not from memory of which flag `login` took.

## What differs from the editor preview

`dev` runs the same Kernel as a Deployment, so HTTP routes, the schedule timer, notice email, Usage and the call outbox all work there. The editor preview has none of them. The Validation Gate checks the schema, `actions.json` and whether any page runs the app. It also refuses a Capsule that uses `miniapp.files` while the `files` module is off.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [CLI reference](../cli.md)
- [The Capsule](../capsule.md)
