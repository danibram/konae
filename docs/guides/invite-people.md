# Invite people

Set the Deployment's signup policy, then send an invitation to each email that should get an app account.

## How it works

The signup policy decides who may create an app account. It is Open (anyone), Invitation only (only through an invitation), Closed (nobody signs up, though an invitation the owner sends can still be accepted) or Allowlist (exact emails or `@domain` entries, an advanced option). A public Deployment starts Open and a private one Closed, and a first release may name another policy. A later publish keeps the policy, except that it turns Open into Closed when it makes the app private.

An invitation is the owner's offer of an account in one app to one email, sent as a link that works for seven days. Accepting it sets the password and activates the account.

## In the editor

1. Open the project and go to Access > Users. After a deploy with the Invitation only policy, the deploy dialog also offers Invite people. After a deploy of a private app with no app account, it offers Invite yourself first, which invites the owner's email.
2. Under Invite people, at the top, enter the email addresses and send. Invite only the people the owner asked for, because this sends email.
3. Resend or revoke an invitation from the list of invitations below it. Under App accounts you can also suspend and resume an account. Suspension ends its sessions and keeps its rows.
4. Below them, check the signup policy under "Who can create an app account" and change it if the app needs another one. Existing accounts keep working.

## Over MCP

A project published from a folder with the CLI shows on the project list with the others, marked "Edited in a folder". Its card opens its data, its Users view and what it did, without the app's files, and its owner can also invite people over MCP.

1. Log in with the `manage_users` permission. Every `app_` tool needs it, and `signup_policy_read` needs only `read`.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir <folder> --url http://localhost:4173 --manage-users
    ```

    The token lands in `<folder>/.konae/credentials.json` for an MCP client to use. Decide what it may call from `session_inspect`'s `can` map. A tool the credential lacks names the permission it needs.
2. Set the policy if the app needs another one. Call `signup_policy_set` with `deploymentId` and a `mode` of `open`, `invitation`, `closed` or `allowlist`, plus the allowlist entries for `allowlist`. The Deployment's ID is the `id` in `konae.json`.
3. Call `mail_status` and look for `sending: true`. Without a mail transport, `app_invitations_create` refuses and creates nothing.
4. Call `app_invitations_create` with `deploymentId`, `emails` and an `idempotencyKey`. It emails each address a link to `/_konae/invitation#invitation=<token>` on the app's host. Call it only when the owner asked for those invitations.
5. The person opens the link and chooses a password, and the page signs them in.
6. `app_invitation_resend` sends a new link that replaces the earlier one. `app_invitation_revoke` ends an invitation and removes the pending account it created, if nobody used it.

## Where the email lands while you try it

- On `bun run dev:local`, every account and invitation email lands in `data/kernel-local/mailbox.jsonl` and goes to no provider.
- On `bun run konae -- dev --dir <folder>`, it lands in `<folder>/.konae/kernel/mailbox.jsonl`. When the policy is not Open, `dev` prints a `kernel invite <email> --app <id>` command that invites a test account. `bun run konae -- events --dir <folder>` lists each email with its link.

The editor preview sends no email, so try invitations on one of these or on a Deployment.

## Your own app account

Your owner account does not sign you in to the app. Sign up, or accept an invitation, with your email like anyone else. When the app keeps roles in its own table, give yourself one with SQL, using the account ID that Access > Users or `app_users_list` shows. Run the SQL in the editor's Data panel, or with `bun run konae -- sql --dir <folder> --prod "<statement>"` for an app published with the CLI.

## Check it worked

- After `signup_policy_set`, read the policy back with `signup_policy_read`.
- After `app_invitations_create`, `app_invitation_resend` or `app_invitation_revoke`, read each invitation's state with `app_invitations_list`.
- After `app_user_suspend` or `app_user_resume`, read the account back with `app_users_list`.

An export carries each invitation as history without its link, so the owner resends it after an import.

## Read next

- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [Refuse a write with guard](./refuse-a-write-with-guard.md)
- [Accounts and access](../accounts-and-access.md)
- [MCP reference](../mcp-tools.md)
