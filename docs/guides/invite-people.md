# Invite people

Set the Deployment's signup policy, then send an invitation to each email that should get an app account.

## How it works

The signup policy decides who may create an app account. It is Open (anyone), Invitation only (only through an invitation), Closed (nobody signs up, though an invitation the owner sends can still be accepted) or Allowlist (exact emails or `@domain` entries, an advanced option). A public Deployment starts Open. A private one starts Closed when its first release names no policy, but the editor's deploy dialog preselects Invitation only for a private app and `konae init` writes `invitation`, so a first release from either door names it. A later publish keeps the policy, except that it turns Open into Closed when it makes the app private.

An invitation is the owner's offer of an account in one app to one email, sent as a link that works for seven days. Accepting it sets the password and activates the account.

## In the editor

1. Open the project and go to Auth > Users. After a deploy with the Invitation only policy, the deploy dialog also offers Invite people.
2. Check the signup policy under "Who can create an app account" and change it if the app needs another one. Existing accounts keep working.
3. Under Invite people, enter the email addresses and send. Invite only the people the owner asked for, because this sends email.
4. Resend or revoke an invitation from the list of invitations below it. Under App accounts you can also suspend and resume an account. Suspension ends its sessions and keeps its rows.

## Over MCP

A project published from a folder with the CLI shows on the project list under "Apps without editor files". **Open data and users** on its card opens the same Users view, without the app's files, and its owner can also invite people over MCP.

1. Log in with the `manage_users` permission. Every `app_` tool needs it, and `signup_policy_read` needs only `read`.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir <folder> --url http://localhost:4173 --manage-users
    ```

    The token lands in `<folder>/.konae/credentials.json` for an MCP client to use. Decide what it may call from `session_inspect`'s `can` map. A tool the credential lacks names the permission it needs.
2. Set the policy if the app needs another one. Call `signup_policy_set` with `deploymentId` and a `mode` of `open`, `invitation`, `closed` or `allowlist`, plus the allowlist entries for `allowlist`. The Deployment's ID is the `id` in `konae.json`.

    ```sh
    bun run konae -- tool signup_policy_set '{"deploymentId":"<id>","mode":"invitation"}' --dir <folder>
    ```

3. Call `mail_status` with `bun run konae -- tool mail_status --dir <folder>` and look for `sending: true`. Without a mail transport, `app_invitations_create` refuses and creates nothing.
4. Call `app_invitations_create` with `deploymentId`, `emails` and an `idempotencyKey`. It emails each address a link to `/_konae/invitation#invitation=<token>` on the app's host. Call it only when the owner asked for those invitations.

    ```sh
    bun run konae -- tool app_invitations_create '{"deploymentId":"<id>","emails":["ana@example.com"],"idempotencyKey":"invite-ana-1"}' --dir <folder>
    ```

5. The person opens the link and chooses a password, and the page signs them in.
6. `app_invitation_resend` sends a new link that replaces the earlier one. `app_invitation_revoke` ends an invitation and removes the pending account it created, if nobody used it.

## Where the email lands while you try it

- On `bun run dev:local`, every account and invitation email lands in `data/kernel-local/mailbox.jsonl` and goes to no provider.
- On `bun run konae -- dev --dir <folder>`, it lands in `<folder>/.konae/kernel/mailbox.jsonl`. When the policy is not Open and the app holds no account yet, `dev` invites `test@example.com` and prints `Test account: open <link> and choose a password.` To invite another address, run the `kernel invite <email> --app <id>` command that `dev` prints. `bun run konae -- events --dir <folder>` lists each email with its link.

The editor preview sends no email, so try invitations on one of these or on a Deployment.

## Your own app account

Your owner account does not sign you in to the app. Sign up, or accept an invitation, with your email like anyone else. When the app keeps roles in its own table, give yourself one with SQL. The `members` template's first owner is this statement, with your account ID from Auth > Users or `app_users_list`:

```sql
UPDATE profiles SET role = 'owner' WHERE user_id = '<account id>';
```

Run it in the editor's Data panel, or with `bun run konae -- sql --dir <folder> --prod "<statement>"` for an app published with the CLI.

## Check it worked

- After `signup_policy_set`, read the policy back with `signup_policy_read`.
- After `app_invitations_create`, `app_invitation_resend` or `app_invitation_revoke`, read each invitation's state with `app_invitations_list`.
- After `app_user_suspend` or `app_user_resume`, read the account back with `app_users_list`.

An export carries each invitation as history without its link, so the owner resends it after an import.

## Read next

- [Add a second owner](./add-a-second-owner.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [Refuse a write with guard](./refuse-a-write-with-guard.md)
- [Accounts and access](../accounts-and-access.md)
- [MCP tools and guide topics](../mcp-tools.md)
- [Accounts and sign-in](../reference/accounts.md)
