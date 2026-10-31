# Invite people

Set a signup policy for the Deployment, then send an invitation to each email that should get an account.

## How it works

The signup policy decides who may create an app account. It is one of Open (anyone), Invitation only (only through an invitation), Closed (nobody signs up, and an invitation the owner sends can still be accepted) or Allowlist (exact emails or `@domain` entries, an advanced option). Public Deployments start Open and Private Deployments start Closed. A first release may name another policy. A republish never changes it afterwards, except to close an Open app made private.

An invitation is the owner's offer of an account on one Deployment to one email. It is a link valid for seven days. Accepting it sets the password and activates the account.

## Steps in the editor

1. Open the project and go to Auth > Users. After a first deploy of a Private app with the Invitation only policy, the published state also offers **Invite people**.
2. Check the signup policy. Change it here if you need to. The change keeps existing accounts working.
3. Invite each person by email. Only invite people the owner asked for, because this sends email.
4. Resend or revoke an invitation from the same view. You can also suspend and resume an app account. Suspension ends its sessions and keeps its rows.

## Steps over MCP

1. Log in with the `manage_users` permission. `signup_policy_read` needs only `read`.

    ```sh
    KONAE_EMAIL=you@example.com KONAE_PASSWORD=... \
    bun run konae -- login --dir /tmp/my-app --url http://localhost:4173 --manage-users
    ```

2. Set the policy if the app needs another one. Use `signup_policy_set` with `deploymentId` and `mode` of `open`, `invitation`, `closed` or `allowlist`, plus allowlist entries.
3. Call `mail_status`. Inviting needs `sending` true. Without a mail transport, `app_invitations_create` refuses and creates nothing.
4. Call `app_invitations_create` with `deploymentId`, `emails` and `idempotencyKey`. It emails each address a link to `/_konae/invitation#invitation=<token>` on the app's host. Call it only when the owner asked for those invitations.
5. The person opens the link and chooses a password. The page posts it to `/_konae/auth/accept` and signs them in.
6. Use `app_invitation_resend` to send a new link, which replaces the earlier one. Use `app_invitation_revoke` to end one. Revoking removes the pending account it created, if nobody used it.

## Your own app account

Your own app account is an app account like any other. Sign up or accept an invitation with your email. When the app keeps roles, name yourself its owner with SQL in Data, using the account ID that Auth > Users shows.

## Check it worked

- After `signup_policy_set`, the read-back is `signup_policy_read`.
- After `app_invitations_create`, `app_invitation_resend` or `app_invitation_revoke`, the read-back is `app_invitations_list`. It shows each invitation's status.
- After `app_user_suspend` or `app_user_resume`, the read-back is `app_users_list`.
- On `konae dev`, the invitation email lands in `<folder>/.konae/dev-data/mailbox.jsonl`, and `konae events` lists it with its link.
- Decide whether you may call these tools from `session_inspect`'s `can` map. A tool your credential may not call answers `Unknown tool or insufficient permission.` and names the permission.

The editor preview has no owner app account and sends no email, so test invitations on `konae dev` or on a Deployment. A package keeps the invitation as history without its link, and the owner resends it.

## Read next

- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [Refuse a write with guard](./refuse-a-write-with-guard.md)
- [Accounts and access](../accounts-and-access.md)
- [MCP reference](../mcp-tools.md)
