# Manage app modules

Modules hold the options and capabilities an app uses. The editor's App settings has a module tab. An outside agent can use the same `modules_read` and `module_set` tools on the owner's MCP. The build agent can use both tools from the editor chat. It asks the owner to confirm each change.

## Read and change a module

Call `modules_read` to see every module, its state, switch availability, options, permission and current behavior. Call `module_set` with the module name, the change and the state you read as `expected`. The tool refuses a stale state with `module.changed`. Read `modules_read` again before retrying. After a successful change, read `modules_read` to confirm the new state.

## Turn on an app module

1. On an existing Deployment, read `session_inspect.can` and call `modules_read { projectId }`. Check the module's permission and dependencies.
2. Turn on each dependency that is off. Call `module_set { projectId, module, on: true, expected }`. Pass `modules.<name>.state` from `modules_read` as `expected`. Read back each change with `modules_read`. The build agent asks you to confirm each change in the editor.
3. Turn on the requested module, then save and verify the app declarations. Publish, read the active release with `project_inspect`, and test the feature on the app's address.

Read [Documents](../documents.md), [App MCP tools](../mcp-app-tools.md), or [File intake](../file-intake.md) for the module you want. The installation operator sets up the native Typst binary, [the FTPS listener](../ftps.md), and OAuth client registrations for external App MCP clients.

The contract in `src/sdk/contract.ts` lists each switch and its agent rule. Publishing a Capsule with a cron job enables Functions and the Actions its target needs. The module tab lists the published cron jobs. Read [Server functions](../functions.md#run-a-function-on-a-cron-schedule) for the cron setup.

## Configure search listing

Turn `seo` on to list titled pages. Its options choose the canonical address (`domain`, `hostname` or `path`) and an image path from the app's published files. Leave the canonical option unset to use the default address.

The module tab and `modules_read` show the canonical address currently in use, why it was selected, a sitemap link and pages left out because they have no title. The Kernel serves the sitemap at `/sitemap.xml`.

The editor preview writes no canonical, card, `robots.txt` or sitemap. `konae dev` writes them using the local host as the address. Check search listing on `konae dev` or a published Deployment.

## Before the first publish

For a local app or a first CLI publish, set each required module's `on` field under `modules` in `konae.json`.

In the web editor, the module tab shows the project state as read-only until the first publish. If a declaration needs a switch that is off, publish the base app first. Turn on the required modules before adding that declaration.
