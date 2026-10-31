# Separate runtime and administrative data access

Status: accepted, 2026-10-21. Amends no earlier ADR. ADR-0008 and ADR-0011 amend it, as the notes below say.

Konae deployments will never expose a SQLite file, database credential, unrestricted SQL-over-HTTP endpoint or export capability to browser code. Deployed apps access protected data only through named server actions evaluated by the Konae Kernel using the deployment, actor and grants; the editor uses the Kernel's separate owner-authenticated Admin Interface for SQL Studio, migrations, backups and export. Deployment visibility controls access to the application shell independently from data authorization, so a public app can still hold private data and a private app still applies per-actor data rules. This separation costs more platform work than shipping SQL credentials or a generic query endpoint, but it is required because anything delivered to a browser can be extracted or replayed.

This boundary prevents downloading or querying the database outside declared actions; it cannot make data already returned to an authorized browser uncopyable. Client JavaScript is also inspectable by anyone allowed to open the deployment, including private deployments, so sensitive logic and secrets must remain in server actions.

## Update, 2026-10-23: anonymous actions and signup policy

ADR-0008 first required a session for every runtime operation. That rule is now refined. On a public Deployment, a signed-out visitor may call Server Actions declared with access `anonymous`. Actions declared `authenticated`, files and uploads still require an app session, and on a private Deployment nothing runs without a login. Each Deployment also has a signup policy (Open, Allowlist or Closed) that decides who can create an app account. The boundary in this ADR is unchanged: browsers reach data only through named Server Actions that the Kernel authorizes.

> Update, 2026-11-06: Files changed by their uploader after upload, through `files_update` and `files_delete`, stay on the Runtime Interface under the same per-actor rule. ADR-0008 records the decision.

## Note, 2026-11-07: where the later amendments are

The boundary holds, and three later ADRs changed who reaches a named Server Action. [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md) moved runtime operations to authenticated MCP. [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md) added Invitation only to the signup policy, let an owner's invitation through under Closed, and gave an action declared `anonymous` an HTTP route that answers to its callers, a signature, an API key or a session, on a private Deployment too. [ADR-0010](0010-kernel-owns-usage-and-signals.md) keeps Usage and Signals out of the app database for the reason this ADR keeps SQL out of the browser.
