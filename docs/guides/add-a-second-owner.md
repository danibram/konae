# Add a second owner

The operator issues an invitation code with the server binary, and the new person creates their owner account with it.

## Before you start

Only the operator can issue a code, because the owners of an installation are the operator's decision ([ADR-0003](../adr/0003-an-account-proves-its-email-before-it-holds-a-session.md)). No owner tool and no editor button does it. The setup key creates the first owner only. After the first code is issued, it creates no account, and revoking every code does not bring it back.

The commands are `owner-code-create`, `owner-code-list` and `owner-code-revoke`. They belong to the `konae` server binary, the one that runs Konae. The TypeScript CLI, `bun run konae --`, directs these commands to the server binary. They need the `KONAE_DATA_DIR` of the running installation and work while Konae runs.

The editor's origin is `http://localhost:4173` under `bun run dev:local`, `http://app.localhost:8080` under Compose, and `https://app.<domain>` on an installation with `KONAE_DOMAIN` set.

## Steps

1. The operator issues the code on the machine that holds the data directory.

    Under `bun run dev:local`, from the repository root:

    ```sh
    KONAE_DATA_DIR=data/kernel-local kernel-rs/target/debug/konae owner-code-create --expires-in 14d --note "For Ada"
    ```

    Under Compose:

    ```sh
    docker compose exec --user konae kernel konae owner-code-create --expires-in 14d --note "For Ada"
    ```

    For the container named `konae` in the [Docker quick start](../../README.md#run-it-in-docker):

    ```sh
    docker exec --user konae konae konae owner-code-create --expires-in 14d --note "For Ada"
    ```

    The command prints the code once, with its ID and expiry. Send the code to the person over a channel you trust. It works once, and `--expires-in` takes `1h` to `90d`.
2. The person opens `<editor origin>/_konae/owner/login`, presses **Create owner account**, and enters their email, a password and the code.

    From a terminal, the same step is `konae login` with the code. It saves a 24-hour token in the folder. Replace `--url` with your editor's origin, such as `http://app.localhost:8080` under Compose or `http://localhost:8787` for the Docker quick start.

    ```sh
    KONAE_INVITATION_CODE='<code>' KONAE_EMAIL=ada@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir ./my-app --url http://localhost:4173
    ```

3. Under `KONAE_ACCOUNT_VERIFICATION=required`, the account stays pending until the person opens the verification link in their email. On a local Konae, add `--verify-from data/kernel-local/mailbox.jsonl` to `login` to answer that link in the same run.

## Check it worked

Read the code back with `owner-code-list`, run the way step 1 ran `owner-code-create`. Under Compose that is:

```sh
docker compose exec --user konae kernel konae owner-code-list
```

The row for the code moves from `open` to `used` and shows the new owner's email with that account's state. Under verification the state is `pending` until the link is opened. A code that stays `open` was never entered. Revoke it with `owner-code-revoke <id>`, and suspend an owner of a used code with `owner-account-suspend <email>`.

The new owner signs in at the same page, and `session_inspect` shows what their credential may do.

## Read next

- [Owner admission](../operate.md#owner-admission)
- [Invite people](./invite-people.md)
- [MCP tools and guide topics](../mcp-tools.md)
