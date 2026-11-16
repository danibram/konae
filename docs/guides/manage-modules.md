# Manage app modules

Modules hold the options and capabilities an app uses. The editor's App settings has a module tab. An outside agent can use the same `modules_read` and `module_set` tools on the owner's MCP. The build agent can use both tools from the editor chat. It asks the owner to confirm each change.

## Read and change a module

Call `modules_read` to see every module, its state, switch availability, options, permission and current behavior. Call `module_set` with the module name, the change and the state you read as `expected`. The tool refuses a stale state with `module.changed`. Read `modules_read` again before retrying. After a successful change, read `modules_read` to confirm the new state.

The contract in `src/sdk/contract.ts` describes each module and its options. `auth`, `actions`, `files`, `calls` and `http` are on and cannot be switched yet. `seo` has a switch and can be configured.

## Configure search listing

Turn `seo` on to list titled pages. Its options choose the canonical address (`domain`, `hostname` or `path`) and an image path from the app's published files. Leave the canonical option unset to use the default address.

The module tab and `modules_read` show the canonical address currently in use, why it was selected, a sitemap link and pages left out because they have no title. The Kernel serves the sitemap at `/sitemap.xml`.

The editor preview writes no canonical, card, `robots.txt` or sitemap. `konae dev` writes them using the local host as the address. Check search listing on `konae dev` or a published Deployment.

## Before the first publish

Before publishing, the module tab shows each module's project state and options as read-only. The first release carries that state. After publishing, the tab reads and changes the Deployment through `modules_read` and `module_set`; changes apply at once.
