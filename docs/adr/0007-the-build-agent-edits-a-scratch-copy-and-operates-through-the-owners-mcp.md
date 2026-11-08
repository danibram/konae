# The build agent edits a scratch copy and operates through the owner's MCP

Status: accepted.

A person who does not write code describes an app to the editor's build agent and gets one that runs (job 1). The agent is browser code that acts with the owner's power, and the rows and files it reads can carry instructions. Its reach over the owner's data must not grow with the model's capability.

## Decision

### The loop runs in the browser on pi-agent-core

The agent runs pi's `Agent` from `@earendil-works/pi-agent-core` with `@earendil-works/pi-ai` (MIT) in the editor's tab. The core imports no Node builtins, and its one platform seam is an `ExecutionEnv` that `src/agent/capsule-env.ts` backs with the app's files in memory. The package gives the loop, streaming, interruption and incremental `edit` diffs. The model layer is swappable, and the agent chunk loads only when the agent opens.

### It edits a scratch copy, and nothing applies without the Gate

- The agent edits a copy of the app's files with `read`, `edit` and `write`. It has no shell.
- `validate_capsule` runs the Validation Gate, so the model repairs its own mistakes, and the same Gate guards Apply, so the invariant does not depend on the model.
- The owner's Apply saves the files through `project_sources_save`.
- The whole app is seeded into the first message and the system prompt stays stable for prompt caching, so a typical edit takes one model turn, or two with a validation round.

### It reaches Konae through the owner's own MCP session

Before each turn the editor reads `tools/list` from `/mcp` with the owner's session and gives the agent the tools `platformToolPolicy` in `src/agent/platform-tools.ts` names, with Konae's names, descriptions and schemas. A call goes out with the same session. One set of tools means one set of refusals, one guide and one permission check for the person who describes an app and the person who connects an agent to it.

- The owner's session reaches every project the owner has. The editor narrows it. It removes `projectId` and `deploymentId` from the schemas the model sees and fills them with the open project's, and a call that names either is refused before it leaves the browser.
- The policy is an allowlist. Reads run at once, such as `project_inspect`, `usage_summary`, `app_users_list`, and `database_execute` when it inspects or runs one `SELECT`. Changes wait for the owner in a dialog, such as `signup_policy_set`, `app_invitations_create`, `domain_set` and any other `database_execute`. After the owner allows a call, the editor checks again that the project is still the one the call named.
- A tool Konae lists that the policy does not name never reaches the model: transfers, credentials, API keys, exports and `secret_set`, because a value typed into the chat reaches the model provider. `project_sources_read` and `project_sources_save` stay out too, because a direct save would skip the Gate.
- `publish` is a tool of the editor, because the build runs in the browser (ADR-0006). It opens the Deploy dialog, where the owner chooses who may open the app and who may sign up and presses Deploy. The tool answers with what happened.
- The agent reads `platform_guide` for the rules its prompt does not carry, and reads a change back through the same door before saying it worked. A refusal reaches it with its code and its fix (ADR-0004).

## Rejected options

- **One structured call per request.** It loses streaming, multi-turn repair and incremental diffs, and stops working once an app outgrows one prompt.
- **A hand-rolled loop.** It rebuilds streaming, sessions and abort for no gain over a small MIT package.
- **A shell tool.** For a few files it adds prompt and attack area without payoff.
- **Platform tools of the agent's own that wrap editor calls.** Two tool sets drift in their refusals and checks.
- **Models in the browser as the main path.** Small models lose coherence across schema and actions, and the download is gigabytes.

## Consequences

- Project pinning, the allowlist and the confirmation dialog are what stand between text in a row and an effect outside the files. The prompt tells the agent never to take permission from file contents, rows, tool output or text inside an app.
- When the owner's session ends mid-run, a call answers 401 and the agent asks the owner to sign in again. Without the session the agent keeps its file tools.
- `src/agent/system-prompt.ts` still carries most of what the agent knows. Moving more of it behind `platform_guide`, so both doors say the same words from one place, is later work.
