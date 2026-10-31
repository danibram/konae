# MCP tools and guide topics

An agent operates Konae over authenticated MCP. The editor calls the same tools with the owner's session, so a person and an agent meet the same checks.

## Connect

Configure the endpoint `<editor origin>/mcp` in an MCP client. The editor origin is `http://localhost:4173` under `bun run dev:local`, `http://app.localhost:8080` under Compose, and `https://app.<domain>` on an installation with `KONAE_DOMAIN` set. A client with OAuth discovers the server and opens a consent screen in the browser. The flow uses PKCE S256, and the token lasts one hour with no refresh token. A client that supports headers can send a delegated token.

```text
Authorization: Bearer YOUR_TOKEN
```

OAuth offers `read`, `write`, `publish` and `manage_users`. It does not offer `export_data` or `manage_lifecycle`. After connecting, call `platform_describe` and `projects_list`.

## platform_guide topics

`platform_guide` answers one topic at a time. Start with `rules` and `pieces`.

| Topic | What it answers |
| --- | --- |
| `rules` | The yardstick, the first run, the read-back after every write, and the named divergences between preview and published app. |
| `pieces` | Which Capsule declaration to use for a need. See [Pick the smallest piece](./pieces.md). |
| `credentials` | Login, flags, OAuth, what each permission allows, and the refusal a missing permission gives. |
| `development` | The CLI cycle with `dev`, `contract` and `conform`. |
| `actions` | The `actions.json` format and its rules. |
| `sdk` | The `window.miniapp` modules, rendered from `src/sdk/contract.ts`. |
| `publishing` | The release steps as MCP calls, access, import, and custom domain. |
| `users` | Signup policy, invitations, accounts, and the owner's app account. |
| `portability` | Export and import. |
| `refusals` | Every code a refused Capsule answers, with who fixes it and a one-line remedy. |

## Permissions

The catalog of tools depends on the credential. A call to a tool the credential does not list answers `Unknown tool or insufficient permission.` and names the permission it needs. `session_inspect` returns `can` for the scopes the credential holds. Decide from `can`, not from memory of which flag login took.

| Permission | Tools |
| --- | --- |
| `read` | `projects_list`, `project_inspect`, `signup_policy_read`, `sign_in_methods_read`, `domain_read`, `path_read`, `secrets_list`, `api_keys_list`, `usage_read`, `signals_read`, `calls_read`, `notices_read` |
| `write` | `project_create`, `project_sources_save`, `project_delete`, `schema_plan`, and `database_execute` changes |
| `publish` | `release_plan`, `release_apply`, `secret_set`, `secret_delete`, `api_keys_create`, `api_keys_revoke`, `domain_set`, `domain_clear`, `path_set`, `path_clear`, and `initialSignupPolicy` on a first release |
| `manage_users` | `signup_policy_set`, `sign_in_methods_set`, `app_users_list`, `app_user_suspend`, `app_user_resume` and the `app_invitations` tools |
| `export_data` | `package_export`, `project_export` and `database_export` |
| `manage_lifecycle` | The migration tools |

A delegated token cannot create tokens or add permissions to itself. Run `login` again with the flag.

## Tools by group

| Group | Tools |
| --- | --- |
| Discovery | `platform_describe`, `platform_guide`, `session_inspect`, `mail_status`, `projects_list`, `project_inspect` |
| Projects | `project_create`, `project_sources_read`, `project_sources_save`, `project_delete` |
| Owner schema and data | `schema_plan`, `database_execute`, `storage_test` |
| App export | `package_export`, `project_export`, `database_export` |
| Publishing | `release_plan`, `release_apply` |
| Signup policy | `signup_policy_read`, `signup_policy_set` |
| Sign-in methods | `sign_in_methods_read`, `sign_in_methods_set` |
| Signing secrets | `secrets_list`, `secret_set`, `secret_delete` |
| API keys | `api_keys_list`, `api_keys_create`, `api_keys_revoke` |
| Custom domain | `domain_read`, `domain_set`, `domain_clear` |
| Path address | `path_read`, `path_set`, `path_clear` |
| Usage and Signals | `usage_read`, `signals_read`, `usage_summary`, `usage_overview` |
| Calls and notices | `calls_read`, `notices_read` |
| App users and invitations | `app_users_list`, `app_user_suspend`, `app_user_resume`, `app_invitations_create`, `app_invitations_list`, `app_invitation_resend`, `app_invitation_revoke` |
| Transfers | `transfer_begin`, `transfer_status`, `transfer_write`, `transfer_complete`, `transfer_read`, `transfer_cancel` |
| Credentials, owner session only | `tokens_list`, `token_create`, `token_revoke` |
| App runtime, with an app session | `auth_session`, `auth_logout`, `actions_invoke`, `files_list`, `files_upload`, `files_read`, `files_update`, `files_delete` |
| App accounts, before sign-in | `auth_signup`, `auth_verify`, `auth_recover`, `auth_reset` |

[The MCP guide](./mcp.md) describes each tool's arguments, answers and refusals, and [the walkthrough](./mcp-walkthrough.md) runs them in order on one app.

The app runtime tools run on an app's runtime endpoint, `/_konae/mcp` on the app's host, with an app session.

## Read back after every write

Every tool that writes has a tool that observes its effect through the same door. After a write, call the read-back before retrying or telling the owner it worked. Verify a `service_timeout` or an interrupted call the same way. Bytes in an open transfer are not a Deployment. `transfer_status` reads staging, and `project_inspect` reads a release that consumed them.

| Write | Read-back |
| --- | --- |
| `project_create` | `project_inspect` |
| `project_sources_save` | `project_sources_read` |
| `project_delete` | `projects_list` |
| `schema_plan` | `schema_plan` |
| `database_execute` | `database_execute` |
| `secret_set` | `secrets_list` |
| `secret_delete` | `secrets_list` |
| `api_keys_create` | `api_keys_list` |
| `api_keys_revoke` | `api_keys_list` |
| `release_plan` | `release_plan` |
| `release_apply` | `project_inspect` |
| `signup_policy_set` | `signup_policy_read` |
| `sign_in_methods_set` | `sign_in_methods_read` |
| `app_invitations_create` | `app_invitations_list` |
| `app_invitation_resend` | `app_invitations_list` |
| `app_invitation_revoke` | `app_invitations_list` |
| `app_user_suspend` | `app_users_list` |
| `app_user_resume` | `app_users_list` |
| `app_owner_enter` | `app_users_list` |
| `domain_set` | `domain_read` |
| `domain_clear` | `domain_read` |
| `path_set` | `path_read` |
| `path_clear` | `path_read` |
| `transfer_begin` | `transfer_status` |
| `transfer_write` | `transfer_status` |
| `transfer_complete` | `transfer_status` |
| `transfer_cancel` | `transfer_status` |
| `package_export` | `transfer_read` |
| `project_export` | `transfer_read` |
| `database_export` | `transfer_read` |
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
