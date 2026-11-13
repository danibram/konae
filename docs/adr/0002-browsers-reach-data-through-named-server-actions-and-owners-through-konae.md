# Browsers reach data through named Server Actions, and owners through Konae

Status: accepted.

Anything delivered to a browser can be read and replayed, including the scripts of a private app. Who may open an app and who may read a row are the app's decisions (job 5), and they have to hold on every machine the app runs on, not only on one host's infrastructure.

## Decision

### The browser reaches data only through named Server Actions

The Kernel never gives browser code a SQLite file, a database credential, a SQL endpoint or an export. A page reads and writes rows only through the Server Actions its `actions.json` names, and the Kernel authorizes each call from the Deployment, the Runtime Actor and the action's bindings. Sensitive logic and secrets stay in Server Actions. Data already returned to an authorized browser can still be copied, and nothing here prevents that.

### The Kernel owns identity and visibility

Sessions, visibility, the Signup Policy and action authorization belong to the Kernel. A hosting provider may supply storage and execution, but its identity or access products never change a Kernel decision, so an app keeps the same rules on any machine.

- Visibility decides who may load the app's shell. A Deployment is public only when its manifest says `"public"`. Any other value, or none, is private, and its shell, assets and runtime need a signed-in app account. A new app is private unless it starts from a template made for visitors without an account.
- The Signup Policy decides who may create an app account: Open, Allowlist (exact emails or `@domain` entries), Invitation only, or Closed. A public app starts Open and a private one starts Closed. The Kernel checks the policy in the transaction that creates the account.
- On a public Deployment a signed-out visitor may call only the actions declared `anonymous`, read the anonymous session, and list and read files uploaded as public. Actions declared `authenticated`, uploads and private files need an app session. On a private Deployment nothing on the runtime endpoint runs without one.
- An HTTP route answers to the callers it declares, on a private Deployment too (ADR-0013). That is the one way an outside program reaches an action.

Visibility and data authorization are separate. A public app can hold private rows, and a private app still applies per-actor rules.

### Owners operate through Konae's authenticated MCP

The editor, the build agent and an outside agent all operate on apps through Konae's MCP on `/mcp`, so one permission check and one set of refusals serve every client. The app's SDK calls the Kernel's runtime endpoint, `/_konae/mcp` on the app's own origin, with the app's session, and a runtime credential never authorizes an owner tool. Login and OAuth keep their HTTP routes for obtaining the first credential.

- The owner's browser holds an HttpOnly session. Other clients use OAuth with authorization code and PKCE, or a delegated token with an expiry, permissions and a set of projects.
- Each tool needs a permission: `read`, `write`, `publish`, `manage_users` or `export_data`. The owner's session holds all of them. OAuth offers `read`, `write`, `publish` and `manage_users`, so an hour-long token from a self-registered client cannot download an app's accounts or data. A client that redirects to `127.0.0.1` or `localhost`, such as `konae login --browser`, may also ask for `export_data` and gets a 24-hour token, because the token lands on the machine where the owner consented. `konae login --manage-users` and `--export-data` ask for the rest.
- A tool the credential lacks answers with the permission it needs and the login flag that asks for it. The answer names no project or Deployment.
- Konae checks the owner's grant on the project for every call, even when the client knows the identifier.
- Konae's audit log keeps the newest 10,000 rows of each owner, plus a 30-day age limit, so one owner calling tools in a loop replaces only their own history.

### Hosted code stays on its own origin

- Konae's MCP, owner sign-in and the OAuth routes refuse a request whose `Origin` is an app host before they read any credential. An app host is a generated hostname, a custom domain or a path address.
- Every response forbids framing. An app's pages carry a Content Security Policy that allows only the artifact's own inline scripts, by hash, and `connect-src 'self'` plus the origins its manifest lists under `connects` (ADR-0013). HTTPS installations send HSTS. [The architecture](../architecture.md#isolation-of-hosted-code) lists the policy.
- A path address shares one origin with every other path address. ADR-0014 says what that costs and how the owner accepts it.

## Rejected options

- **Ship SQL credentials or a generic query endpoint to the browser.** Anything in the bundle can be extracted and replayed, so the endpoint would answer whoever reads the bundle.
- **Use a provider's identity or access product as the source of truth.** An app would then change its rules when it changed host, which breaks job 4.
- **A session for every runtime operation.** A public form, a shared list and a gallery are for visitors without an account. The `anonymous` access level names exactly what such a visitor may call.
- **Public as the default visibility.** An owner who publishes without reading the dialog should expose nothing they did not ask to expose.
- **A permission check per client.** The editor, the build agent and an outside agent would drift. One MCP keeps one check.

## Consequences

- An owner tool reads and changes an app; a runtime tool acts as one of its users. The two never share a credential.
- A Deployment record written without a visibility asks for a login until its owner republishes it as public.
- The editor preview speaks MCP over a private `MessageChannel` with a credential limited to the preview. Its test actors are not real sessions.
