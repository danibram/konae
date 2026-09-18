# Use authenticated MCP for Littlebox operations

Status: accepted.

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
