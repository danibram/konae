# The build agent operates the platform through the owner's MCP

Status: accepted, 2026-11-09. Amends [ADR-0003](0003-in-browser-agent-on-pi-agent-core.md). Builds on [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md).

The editor's build agent reaches the Kernel through the same administrative MCP an external agent uses, with the signed-in owner's browser session as its credential. It no longer has platform tools of its own that wrap editor calls, such as the earlier `domain_read`, `domain_set` and `domain_clear`. One tool surface means one set of refusals, one guide and one permission check for the person who describes an app and the person who connects an agent to it.

## Decision

Before each turn the editor reads `tools/list` from `/mcp` with the owner's session and gives the agent the listed tools that `platformToolPolicy` in `src/agent/platform-tools.ts` names, with the Kernel's name, description and input schema. A call goes to `McpClient.call` with the same session. A session that cannot list them, because it ended or the Kernel is down, leaves the agent its file tools, and the next turn after a sign-in gets them back.

The owner's session reaches every project that owner has, and the Kernel cannot narrow it to one. The editor narrows it:

- `projectId` and `deploymentId` are removed from the schema the model sees and filled in with the open project's. A call that names either is refused before it leaves the browser, and a tool that needs a deployment on an unpublished project says to publish first.
- The policy is an allowlist with two values. Reads run: `session_inspect`, `platform_guide`, `project_inspect`, `usage_summary`, `notices_read`, `signup_policy_read`, `app_users_list`, `domain_read`, and `database_execute` when it inspects, browses, lists backups or runs one `SELECT`. The rest wait for the owner in a dialog: `signup_policy_set`, `app_invitations_create`, `domain_set`, `domain_clear` and any other `database_execute`. After the owner allows a call, the editor checks again that the project and its deployment are still the ones the call named.
- A tool the Kernel lists that the policy does not name never reaches the model. That keeps out transfers, credential issuing (`token_*`, `api_keys_*`), secret values (`secret_set`, because a value typed into the chat reaches the model provider), exports, eject and migration. A new Kernel tool reaches the agent only when someone adds it to the policy.

Source edits keep ADR-0003's path, and the agent is told so. It edits a scratch copy with `read`, `edit` and `write`, `validate_capsule` runs the Validation Gate, and the owner's Apply saves through `project_sources_save`. `project_sources_read` and `project_sources_save` are not in the policy, because the save checks the project's shape and not its actions or their LIMIT, so a direct save would skip the Gate until publish. The prompt's section "You are in the web editor" says that the guide's advice to save sources over MCP is for agents outside the editor.

`publish` is a tool the editor offers, not a Kernel tool, because the build runs in the browser (ADR-0007). It opens the same Deploy dialog the owner uses. The owner chooses who may open the app and who may sign up, and presses Deploy, and the tool answers with what happened: published with its address and visibility, closed without publishing, or refused with the Kernel's words. The model does not drive transfers.

The agent reads `platform_guide` for rules its prompt does not carry, and reads a change back through the same door before saying it worked.

## Consequences

Data the agent reads, such as rows from `database_execute`, can carry instructions, and the agent acts with the owner's power. Project pinning, the allowlist and the confirmation dialog are what stand between such text and an effect outside the files. The prompt tells the agent never to take permission from file contents, rows, tool output or text inside an app.

When the owner's session ends mid-run, a call answers 401 and the agent says to sign in again; it does not report a tool failure. A Kernel refusal reaches the model with its message, its code and its fix (ADR-0020).

The Kernel's `initialize` instructions address an external agent ("Source and builds stay on your computer"). The editor's client reads them; the model does not, and its prompt says how the editor differs.

`src/agent/system-prompt.ts` still carries most of what the agent knows. Moving more of it behind `platform_guide`, so both doors say the same words from one place, is the next step and is not done here.
