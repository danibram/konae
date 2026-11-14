# Reference

Konae's MCP on `/mcp` is how the editor, the build agent and an outside agent operate apps with an owner's credential. An app's own pages call the runtime endpoint, `/_konae/mcp` on the app's host, with an app session. Both speak MCP over Streamable HTTP, revision `2025-11-25`.

These pages state each tool's arguments, answers and refusals, one topic to a page. [MCP tools and guide topics](../mcp-tools.md) lists the tools, the permission each needs and the read-back after each write. [The walkthrough](../mcp-walkthrough.md) runs them in order on one app, with recorded answers.

| Page | What it states |
| --- | --- |
| [Projects and releases](./projects-and-releases.md) | Create a project, plan and apply a release, read the Action Contract, set the contact email, put an app in maintenance and move bytes in transfers. |
| [Accounts and sign-in](./accounts.md) | Signup Policy, sign-in methods, invitations, verified accounts, owner signup, account email and roles inside an app. |
| [Runtime endpoint and action rules](./actions.md) | The runtime endpoint an app's pages call, the statement rules and the shape of an action. |
| [Routes and calls](./routes-and-calls.md) | HTTP routes with signatures, webhooks and API keys, and calls between apps. |
| [Usage, Signals, schedules and notices](./usage-and-signals.md) | Usage, Signals, guards, scheduled work and notices. |
| [Addresses](./addresses.md) | A custom domain, the generated hostname and a path address. |
| [Export and import](./export-and-import.md) | Export an app folder and import it into a new or an existing app. |

## Credentials

Run `bun run dev:local`, open `http://localhost:4173/_konae/owner/login` and sign in on the owner sign-in page. The editor's `/platform-access` opens the same page, and on an installation with Clerk it holds Clerk's sign-in. The first owner of an installation follows "Create an owner account" and gives the "Installation setup key", which a local install fills in. Every later owner needs an invitation code from the operator, as [Operate an installation](../operate.md#owner-admission) describes.

| Credential | Permissions it can hold | Lifetime |
| --- | --- | --- |
| A token from the owner sign-in page | `read` alone, or `read`, `write` and `publish` | 24 hours |
| `token_create`, from the owner's browser session | Any of `read`, `write`, `publish`, `manage_users` and `export_data` | Up to 30 days, one hour on an installation that signs owners in through Clerk |
| `konae login` | `read`, `write` and `publish`, plus `--manage-users` and `--export-data` | 24 hours |
| An OAuth client | `read`, `write`, `publish` and `manage_users`. Asking for `export_data` answers `Invalid scope.` | One hour, with no refresh token |

The owner's browser session holds every permission. A delegated token cannot create tokens or add a permission to itself, so ask `login` for every permission the work needs. The setup key never authenticates a tool, and Konae refuses to start on a reachable address while the key is the development default ([Before a Kernel is reachable](../operate.md#before-a-kernel-is-reachable)).

Configure `<editor origin>/mcp` in the MCP client, with OAuth or with `Authorization: Bearer <token>`. Then call `session_inspect`, whose `can` map says what this credential may do, and `platform_guide` with topic `rules`. A tool the credential does not list answers `Unknown tool or insufficient permission.` and the permission it needs, such as:

```text
app_invitations_create needs a credential with manage_users. Run konae login with --manage-users, or use a token from token_create that includes it.
```

A tool of the runtime endpoint called on `/mcp` answers that it runs on `/_konae/mcp` with an app session. A token limited to some projects answers `Project is outside this token's grants.` for any other project and cannot create projects.

## Read next

- [Edit the app's files with the CLI](../guides/edit-files-with-the-cli.md)
- [Add a second owner](../guides/add-a-second-owner.md)
- [MCP tools and guide topics](../mcp-tools.md)
- [Operate an installation](../operate.md)
