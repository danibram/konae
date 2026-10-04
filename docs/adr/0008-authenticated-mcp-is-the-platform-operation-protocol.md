# Use authenticated MCP for Littlebox operations

Status: accepted before its update of 2026-10-24. Amends [ADR-0001](0001-separate-runtime-and-admin-data-access.md) and [ADR-0002](0002-kernel-owns-identity-and-visibility.md): runtime operations go through authenticated MCP. [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md) amends its export transfers.

The editor, external agents and apps will use authenticated MCP to operate on Littlebox. The user keeps the code and build of External projects on their own machine. Sharing the protocol lets the editor and agents use the same operations and avoids keeping a second permission implementation for each client.

## Decision

The Admin Interface and the Runtime Interface keep their roles and permissions. Administrative MCP authenticates the Project Owner; runtime MCP authenticates the Runtime Actor of the Deployment that the Kernel resolves. A runtime token never authorizes administrative tools. Operations check membership and grants in the Kernel, even when the client knows the resource identifier.

Administrative tools use OAuth with authorization code and PKCE, or delegated credentials with an expiry, scopes and a limited set of projects. The owner's browser uses an HttpOnly session. An app's SDK uses that Deployment's session and wraps deterministic MCP calls, without invoking a model for each action.

Artifact transfers, exports and files also use MCP tools. Each transfer belongs to its actor, has an expected size and checksum, accepts bounded chunks and expires. The old operation routes answer with HTTP 410, including administration by development key. The installation key only allows registering owners and is never built into the frontend.

Login and OAuth keep their HTTP mechanisms for obtaining the first credential. HTTP also serves HTML, CSS, JavaScript and discovery metadata. WebMCP is outside this contract.

## Consequences

An app may keep its shell public, but its data and file operations require authentication. This decision replaces the option of running anonymous operations on the Runtime Interface from ADR-0001 and ADR-0002. Existing action programs remain readable; the MCP adapter requires a session before invoking them.

> Update, 2026-10-24: On a public Deployment, a signed-out visitor may read the anonymous session, call Server Actions declared `anonymous`, and list or read files uploaded as public. Uploads and private files require an app session. On a private Deployment nothing runs without a login. A file is private to its uploader unless the upload sets visibility to public. Administrative tools still always require an owner credential. Each Deployment also gets a signup policy (Open, Allowlist or Closed) that the owner sets in the editor's Auth panel or through admin MCP tools.

> Update, 2026-11-06: The `sdk` topic of `platform_guide` is rendered from the SDK contract in `src/sdk/contract.ts`. `bun run sdk:render` writes it to `kernel-rs/src/sdk-guide.json`, which the Kernel includes at compile time, and writes the build agent's SDK section and `docs/sdk.md` from the same contract. The topic now lists each module and call with its runtime MCP tools, arguments, result, refusals and limits; the `authentication` and `files` prose keys are gone and `examples` stays. The decision itself is unchanged.

> Update, 2026-11-06: Exports move through `package_export` as one transfer per component, and downloading an app's accounts and data needs the `export_data` permission, which `read`, `write` and `publish` do not include. [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md) records the format and the import.

ADR-0005 keeps the static artifact as the publishing contract. ADR-0006 keeps the local Development Kernel as the default option. The local tool runs the same Rust Kernel with data and identities separate from production. The editor preview uses MCP over a private MessageChannel and an ephemeral credential limited to the preview; its test actors are not production sessions.

The implementation pins MCP revision `2025-11-25`, checked with the official SDK `1.32.1`. Revision `2026-07-28` requires further compatibility work. Project revisions and publishing receipts persist, but they are not an immutable artifact history or a data rollback.

Drizzle derivation checks an allowlist of syntax, then evaluates the model in a subprocess with closed input, an empty environment, a timeout and bounded output. On a reachable Kernel that subprocess is a setuid helper that drops to a uid with no access to the data directory before it runs the model. A loopback Kernel may spawn the worker as itself. The Kernel process never evaluates the app's sources.

> Update, 2026-11-06: Administrative MCP, owner sign-in and the OAuth routes refuse a request whose `Origin` is an app host before they read any credential. An app host is a deployment hostname or any host under `LITTLEBOX_HOST_SUFFIX` other than the installation's public or MCP host. Every Kernel response forbids framing, app responses carry a Content Security Policy that allows only the artifact's own inline scripts by hash, and HTTPS installations send HSTS. [The architecture](../architecture.md#isolation-of-hosted-code) lists the policy.

> Update, 2026-11-06 (file lifecycle): A file has a lifecycle after its upload. The runtime tools `files_update` and `files_delete`, behind `miniapp.files.update` and `miniapp.files.delete`, let the uploader alone make a file public or private, or delete it; a deleted file frees its share of the 20 MiB quota and its ID answers `File not found.` through every door. Both need an app session, and a delete is audited like an upload because each file is deleted once. `miniapp.files.url` returns `/_littlebox/files/<id>`, so a page re-reads a file over its own origin instead of a Blob URL that `connect-src 'self'` blocks; the policy is unchanged. The SDK refuses an unknown visibility instead of storing a private file. On the file path, a visitor without a session on a private Deployment gets 401 before any lookup, and every other caller who may not read a file gets 404, as for a missing one; this was already the behaviour and is now the documented rule.

## Note, 2026-11-07: a refusal names the permission, and OAuth names its scopes

A walkthrough of the whole development flow by an agent that read only the docs (docs/mcp-walkthrough.md) found that a tool the credential lacked answered `Unknown tool or insufficient permission.` and nothing else, so the agent could not tell `app_invitations_create` from a typo. The administrative endpoint now finds the permission that would list the tool and adds it, with the `littlebox login` flag that asks for it: `Unknown tool or insufficient permission. app_invitations_create needs a credential with manage_users. Run littlebox login with --manage-users, or use a token from token_create that includes it.` A tool that only the owner's browser session lists, and a runtime tool called on the administrative endpoint, say so instead. A name no credential reaches keeps the bare message. The hint names only the permission, never a project or a Deployment, so it tells a caller nothing it could not read in `platform_guide`.

`littlebox login` gains `--manage-users`, beside `--export-data` and `--manage-lifecycle`, because a delegated token cannot widen itself and the CLI had no way to ask for it.

docs/mcp.md said OAuth clients may request every scope `token_create` takes. The consent screen offers `read`, `write`, `publish` and `manage_users`, as ADR-0013 decided for `export_data`, and the same holds for `manage_lifecycle`: an hour-long token from a self-registered client does not download an app's data, eject it or migrate it. A request for another scope now answers with the scopes OAuth takes and the login flags for the rest. `platform_guide` gains the topics `credentials`, `users` and `portability`, kept in `kernel-rs/src/guide.rs`, and a test there fails when a topic names a tool the Kernel does not serve.

## Note, 2026-11-07: the audit log keeps rows per owner and per Deployment

The audit log kept the newest 10,000 rows of the whole installation. A signed-in app user writes one row for each upload that stores a file and each delete that removes one, and a transfer slot frees as soon as it is cancelled, so an upload and delete loop of about 10,000 pairs pushed every owner's history out. Pruning now keeps the newest 10,000 rows of each owner, and of each Deployment's app users together, plus the 30-day age limit. A creator calling tools in a loop now replaces only their own rows too.

## Note, 2026-11-07: visitors have their own transfer pool

A signed-out visitor of a public app reads a public file through `files_read`, which holds a download transfer for 10 minutes. Visitors counted against the Deployment's pool of 32, two per client address, so 16 addresses or /64s held every slot and no signed-in user could upload. Visitors now share a pool of 16 of their own, and the signed-in pool of 32 does not count them.
