# An app turns on the modules it uses

Status: accepted.

Every app carries a database, accounts, files and routes whether it uses them or not. The Konae home is HTML, CSS and one script, and it still ships an `actions.json` with no action, a `schema.sql`, and the SDK in every page. Each option of an app has a tool of its own, so a new one needs a tool, an entry in the build agent's allowlist and a form in the editor, and the permission it needs lives in code under the tool's name. Search listing (ADR-0016) is the next capability, and it should not add a fourth way to set an option.

## Decision

### A module is one capability of an app

- A module holds four things: the calls a page makes on `window.miniapp`, the keys it adds to Capsule files, the options an owner sets, and what the Kernel does for it. `auth`, `actions`, `files`, `calls` and `http` are modules today, and `calls` and `http` have no browser call.
- `src/sdk/contract.ts` describes every module once. It gains, for each module, its options, the permission a change needs, whether the build agent may change it after the owner confirms or never, the modules it requires, and how to tell that an app uses it.
- `bun run sdk:render` writes the readers: the build agent's prompt, the `sdk` topic of `platform_guide`, `docs/sdk.md` and `kernel-rs/kernel/src/modules.json`. The Kernel reads the last one, so the `kernel` binary checks an option the way Konae does. `src/sdk/contract.test.ts` fails when a reader disagrees.

### `konae.json` says which modules are on

- `konae.json` holds state only for modules whose switch is available: `"modules": { "<name>": { "on": true, "<option>": <value> } }`. An export carries it. A secret never goes there.
- A switchable module is off until someone turns it on. `konae init` and the editor's templates turn on what the template uses.
- `modules_read` and `module_set` expose auth options beside the module switch. In `konae.json`, Signup Policy and sign-in methods stay in the existing top-level `signupPolicy` and `signInMethods` fields, and the `auth` entry stores only `on`. A public app defaults to Open and a private app to Closed. A private first release turns auth on, as does a first release that turns on files. Later changes apply at once and have no draft. This keeps an export readable by the release that predates module settings.
- A module that `konae.json` does not name has never been decided for that app. Only an app made before the module existed is in that state.

### The published app never uses a module that is off

- A release is refused when the Capsule uses a module that is off. The Validation Gate and the Kernel answer `module.off` with the same sentence, which names the module and says where to turn it on.
- Turning a module off is refused with `module.in-use` while the published app uses it.
- Turning a module on is refused with `module.requires` while a module it requires is off. `http`, `calls` and a page route (ADR-0017) require `actions`. A private app requires `auth`.
- `http` is in use when the installed release declares an HTTP route. `calls` is in use when the installed release declares a call action. A source save after release does not change either decision.

### Two tools change every module

- `modules_read { projectId }` needs `read`. It answers every module with its state, its options, what in the app uses it, and what it does right now.
- `module_set { projectId, module, on?, options?, expected? }` needs the permission the contract names for that module. `expected` is the state the caller read, and a call whose `expected` no longer matches is refused with `module.changed`. The read-back is `modules_read`.
- The editor's modules tab is drawn from the contract and calls the same two tools. The build agent gets them through the allowlist of ADR-0007, and a change waits for the owner in a dialog. The `kernel` binary has `kernel module`, which calls the same function.
- `konae dev` reads the folder's `konae.json`. A publish sends `modules` only with the first release. After that the installation's state decides, and `konae pull` brings it into the folder.

### An app made before a module keeps working

- When a Kernel opens an app whose `konae.json` does not name a module, it decides once. It turns the module on when the app uses it, off when it does not, and on when the folder cannot tell. It writes the answer and logs it. An import does the same.
- For `files`, the Kernel decides on when `data/files/index.json` lists a file, the old `manifest.fileAccess` option is set, or a built file names `miniapp.files`. An unreadable file index or artifact means on. An empty or missing index plus a readable artifact with no `miniapp.files` means off.
- For `auth`, the Kernel decides on when the old folder is private, its identity database has accounts or invitations, its old file policy admits all app users, its app uses files, or its sources or artifact use auth. Otherwise a readable public static app with no accounts decides off. An unreadable folder or database means on.
- On a first release with no saved module state, the Kernel makes the same decision from the sources and artifact. It turns `files` on when the sources use `miniapp.files` or name a `/_konae/files/` URL.
- The operator reads what each app got with `kernel modules`, before and after the upgrade.
- The `kernel` field of `konae.json` cannot answer this. Every release rewrites it with the running version.

### The files module owns file access

- `fileAccess` is an option of `files`, not the Capsule manifest. Setting it takes effect at once, including on files already uploaded. The option keeps the prior access rule that every signed-in app user may read every file.
- `files` requires `auth` because an upload needs a signed-in app user. A later decision may allow uploads without an account, with its own rules for ownership and access.
- `connects` stays in the Capsule manifest. It adds origins to the page's `connect-src` and does not govern what a call action can reach.

### Modules move one at a time

- `seo` comes first, because it is new and replaces no tool. Then `files`, `auth`, `actions`, and last `http` and `calls`.
- Moving a module puts its options under `module_set` and removes the tools they had in the same change. Until a module moves, it behaves as it does today.

## Rejected options

- **Plugins the Kernel loads, or a layer in Konae that switches behaviour.** The `kernel` binary serves an exported folder with no Konae around it, so the behaviour has to be Kernel code.
- **One tool for each option.** It is what exists. Each option costs a tool, an allowlist entry and a form, and two options can disagree about who may change them.
- **Options in the Capsule manifest, applied at a release.** The build agent may not save the manifest (ADR-0007), and an agent with MCP alone cannot publish, so it could not operate a module.
- **Leave the database, accounts and files always on.** A static app keeps a database it never opens, and nobody can read from the folder what an app is able to do.
- **Read which modules are on from the Capsule at each release, and store nothing.** The owner could not refuse a capability, options would have no place, and `files` and `seo` add no key to read.
- **Turn on a required module without asking.** An app would gain accounts because someone turned on files.
- **Decide from the `kernel` field whether an app predates a module.** The next publish overwrites the field, and the app would lose its modules.

## Consequences

- After `actions` moves, an app with every module off is its static files: no `actions.json`, no `schema.sql`, no SDK script and no database. Until then the home keeps what it ships today.
- A publish can now be refused for a module that is off. The refusal names the module and the tool, so one call clears it.
- The Validation Gate needs the project's module state. The editor reads it with `modules_read`, and the CLI reads the folder's `konae.json`.
- A tool that a moved module replaces stops existing. An agent that still calls it gets the name of `module_set` from `platform_guide`.
- Words change with the decision. `CONTEXT.md` gets a module entry, and the SDK module entry becomes the part of a module a page calls.
