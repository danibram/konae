# MCP tools and guide topics

An agent operates Konae through the owner's MCP on `/mcp`. The editor calls the same tools with the owner's browser session, so a person and an agent meet the same checks.

## Connect

Configure the endpoint `<editor origin>/mcp` in an MCP client. The editor origin is `http://localhost:4173` under `bun run dev:local`, `http://app.localhost:8080` under Compose, and `https://app.<domain>` on an installation with `KONAE_DOMAIN` set. A client with OAuth discovers the server and opens a consent screen in the browser. The flow uses PKCE S256, and the token lasts one hour with no refresh token. A client that supports headers can send a delegated token from `konae login`, from the owner sign-in page at `/platform-access`, or from `token_create`.

```text
Authorization: Bearer YOUR_TOKEN
```

OAuth offers `read`, `write`, `publish` and `manage_users`. It does not offer `export_data`. After connecting, call `session_inspect` and `platform_guide` with topic `rules`.

## platform_guide topics

`platform_guide` answers one topic at a time. Start with `rules` and `pieces`.

| Topic | What it answers |
| --- | --- |
| `rules` | The yardstick, the first run, the read-back after every write, and the named divergences between preview and published app. |
| `pieces` | Which Capsule declaration to use for a need. See [Pick the smallest piece](./pieces.md). |
| `credentials` | `konae login` and its flags, OAuth, what each permission allows, and the refusal a missing permission gives. |
| `development` | The CLI cycle: `init`, `dev`, `pull`, `sql`, `events`, `contract` and `conform`. |
| `actions` | The `actions.json` format and its rules. |
| `sdk` | The `window.miniapp` modules, rendered from `src/sdk/contract.ts`. |
| `publishing` | The release steps as MCP calls, the schema change, the first Signup Policy, custom domain, path address and maintenance. |
| `users` | Signup Policy, Google sign-in, invitations, app accounts, entering as a user, and roles inside an app. |
| `portability` | Export, running the folder on a `kernel` binary without Konae, and import into a new or an existing app. |
| `refusals` | Every code a refused Capsule answers, with who fixes it and a one-line remedy. |

## Permissions

The catalog of tools depends on the credential. A call to a tool the credential does not list answers `Unknown tool or insufficient permission.` and names the permission it needs. `session_inspect` returns `can` for the scopes the credential holds. Decide from `can`, not from memory of which flag login took.

| Permission | Tools |
| --- | --- |
| None | `platform_describe`, `platform_guide`, `session_inspect` and `mail_status` |
| `read` | `projects_list`, `project_inspect`, `project_sources_read`, `storage_test`, `signup_policy_read`, `sign_in_methods_read`, `domain_read`, `path_read`, `maintenance_read`, `contact_email_read`, `secrets_list`, `api_keys_list`, `usage_read`, `signals_read`, `usage_summary`, `usage_overview`, `calls_read`, `notices_read`, and `database_execute` reads |
| `write` | `project_create`, `project_sources_save`, `project_delete` and `database_execute` writes |
| `publish` | `release_plan`, `release_apply`, `secret_set`, `secret_delete`, `api_keys_create`, `api_keys_revoke`, `hostname_set`, `domain_set`, `domain_clear`, `path_set`, `path_clear`, `maintenance_set`, `contact_email_set`, `project_import`, and `initialSignupPolicy` on a first release |
| `manage_users` | `signup_policy_set`, `sign_in_methods_set`, `app_users_list`, `app_user_suspend`, `app_user_resume`, `app_user_enter` from the owner's browser session, and the `app_invitations` tools |
| `export_data` | `project_export` and reading its downloads |
| The owner's browser session | `tokens_list`, `token_create`, `token_revoke` and `app_user_enter` |

The transfer tools follow what they move. `transfer_begin`, `transfer_write` and `transfer_complete` need `write` or `publish`, `transfer_read` and `transfer_status` need `read`, or `export_data` for an export's downloads, and `transfer_cancel` takes any permission.

A delegated token cannot create tokens or add permissions to itself. Run `konae login` again with the flag.

## Tools by group

| Group | Tools |
| --- | --- |
| Discovery | `platform_describe`, `platform_guide`, `session_inspect`, `mail_status`, `projects_list`, `project_inspect` |
| Projects | `project_create`, `project_sources_read`, `project_sources_save`, `project_delete` |
| Owner data | `database_execute`, `storage_test` |
| App export and import | `project_export`, `project_import` |
| Publishing | `release_plan`, `release_apply` |
| Signup policy | `signup_policy_read`, `signup_policy_set` |
| Sign-in methods | `sign_in_methods_read`, `sign_in_methods_set` |
| Signing secrets | `secrets_list`, `secret_set`, `secret_delete` |
| API keys | `api_keys_list`, `api_keys_create`, `api_keys_revoke` |
| Generated hostname | `hostname_set` |
| Custom domain | `domain_read`, `domain_set`, `domain_clear` |
| Path address | `path_read`, `path_set`, `path_clear` |
| Maintenance | `maintenance_read`, `maintenance_set` |
| Contact email | `contact_email_read`, `contact_email_set` |
| Usage and Signals | `usage_read`, `signals_read`, `usage_summary`, `usage_overview` |
| Calls and notices | `calls_read`, `notices_read` |
| App users and invitations | `app_users_list`, `app_user_suspend`, `app_user_resume`, `app_user_enter`, `app_invitations_create`, `app_invitations_list`, `app_invitation_resend`, `app_invitation_revoke` |
| Transfers | `transfer_begin`, `transfer_status`, `transfer_write`, `transfer_complete`, `transfer_read`, `transfer_cancel` |
| Credentials, owner session only | `tokens_list`, `token_create`, `token_revoke` |
| App runtime, with an app session | `auth_session`, `auth_logout`, `actions_invoke`, `files_list`, `files_upload`, `files_read`, `files_update`, `files_delete` |
| App accounts, before sign-in | `auth_signup`, `auth_verify`, `auth_recover`, `auth_reset` |

The app runtime and app account tools run on an app's runtime endpoint, `/_konae/mcp` on the app's host, with an app session. Called on `/mcp`, they answer where they run.

[The MCP guide](./mcp.md) gives each tool's arguments, answers and refusals, and [the walkthrough](./mcp-walkthrough.md) runs them in order on one app.

## Read back after every write

Every tool that writes has a way to observe its effect through the same door. After a write, call the read-back before retrying or telling the owner it worked. Verify a `service_timeout` or an interrupted call the same way. Bytes in an open transfer are not a Deployment. `transfer_status` reads staging, and `project_inspect` reads a release that consumed them.

Two writes answer with their own observation, so no second call follows them. `transfer_write` answers `nextOffset`, where the next block starts; call `transfer_status` only to resume after an interrupted call. `release_plan` changes nothing live, and its answer is the plan; pass its `planId` to `release_apply`, because another `release_plan` makes a new plan with a new `planId`. After `transfer_cancel`, `transfer_status` answers `Transfer not found or expired.`, and that answer is the cancel confirmed.

| Write | Read-back |
| --- | --- |
| `project_create` | `project_inspect` |
| `project_sources_save` | `project_sources_read` |
| `project_delete` | `projects_list` |
| `database_execute` | `database_execute` |
| `secret_set` | `secrets_list` |
| `secret_delete` | `secrets_list` |
| `api_keys_create` | `api_keys_list` |
| `api_keys_revoke` | `api_keys_list` |
| `release_apply` | `project_inspect` |
| `signup_policy_set` | `signup_policy_read` |
| `sign_in_methods_set` | `sign_in_methods_read` |
| `app_invitations_create` | `app_invitations_list` |
| `app_invitation_resend` | `app_invitations_list` |
| `app_invitation_revoke` | `app_invitations_list` |
| `app_user_suspend` | `app_users_list` |
| `app_user_resume` | `app_users_list` |
| `app_user_enter` | `auth_session` |
| `hostname_set` | `project_inspect` |
| `domain_set` | `domain_read` |
| `domain_clear` | `domain_read` |
| `path_set` | `path_read` |
| `path_clear` | `path_read` |
| `maintenance_set` | `maintenance_read` |
| `contact_email_set` | `contact_email_read` |
| `transfer_begin` | `transfer_status` |
| `transfer_complete` | `transfer_status` |
| `transfer_cancel` | `transfer_status` |
| `project_export` | `transfer_read` |
| `project_import` | `project_inspect` |
| `token_create` | `tokens_list` |
| `token_revoke` | `tokens_list` |
| `auth_logout` | `auth_session` |
| `auth_signup` | `auth_session` |
| `auth_verify` | `auth_session` |
| `auth_recover` | `mail_status` |
| `auth_reset` | `auth_session` |
| `files_upload` | `files_list` |
| `files_update` | `files_list` |
| `files_delete` | `files_list` |
| `actions_invoke` | `actions_invoke` |

## Read next

- [Pick the smallest piece](./pieces.md)
- [CLI](./cli.md)
- [Accounts and access](./accounts-and-access.md)
- [The Gate and the Kernel](./gate-and-kernel.md)
