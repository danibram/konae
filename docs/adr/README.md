# Architecture decisions

Each record states its status and which earlier record it amends on its first line, and keeps its later changes as dated notes at the end, oldest first. The next free number is 0016. Before taking it, check `main`, because another branch may have taken it.

| ADR | Decides | Amends | Amended by |
| --- | --- | --- | --- |
| [0001](0001-separate-runtime-and-admin-data-access.md) | Browsers reach data only through named Server Actions; owner tools use a separate Admin Interface | | 0008, 0011 |
| [0002](0002-kernel-owns-identity-and-visibility.md) | The Kernel, not a provider, owns sessions, grants, visibility and signup policy | | 0011 |
| [0003](0003-in-browser-agent-on-pi-agent-core.md) | The build agent runs in the browser on pi-agent-core and edits a scratch copy behind the Validation Gate | | 0004 |
| [0004](0004-use-drizzle-schema-as-the-managed-database-model.md) | `schema.ts` in Drizzle is a Managed Project's Database Model; migrations derive from it | 0003 | |
| [0005](0005-build-artifacts-separate-managed-and-external-projects.md) | The Build Artifact is the publishing contract; Eject moves source authority; one folder per Deployment; the static publishing rules | | 0007 |
| [0006](0006-external-projects-use-a-local-development-kernel.md) | External Projects develop against a local Development Kernel | | |
| [0007](0007-managed-builds-run-in-the-browser.md) | Managed builds run in the browser; schema derivation runs in a subprocess, never in the Kernel process | 0005 | |
| [0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md) | Authenticated MCP is the operation protocol for the editor, agents and apps; hosted-code isolation headers; the file lifecycle | 0001, 0002 | 0013 |
| [0009](0009-the-gate-and-the-kernel-share-one-query-bound.md) | A query needs a top-level `LIMIT`, and the Gate and the Kernel refuse statements with the same words | | |
| [0010](0010-kernel-owns-usage-and-signals.md) | The Kernel keeps Usage and Signals per Deployment; a mutation may declare a guard | | |
| [0011](0011-one-verified-account-core-for-creators-and-app-users.md) | One verified account core for creators and app users; invitation codes; app invitations; HTTP routes, callers and API keys | 0001, 0002 | 0014, 0015 |
| [0012](0012-account-email-goes-through-a-persistent-queue.md) | Account email leaves through a persistent queue with a sender preflight and limits | | |
| [0013](0013-an-app-leaves-as-a-chunked-package.md) | An app leaves as a chunked package behind `export_data`; API keys travel revoked | 0008 | 0014 |
| [0014](0014-an-app-migrates-with-one-writer-at-a-time.md) | A migration keeps one writer through maintenance and a confirmation; a restore applies the revocation record | 0011, 0013 | |
| [0015](0015-an-app-calls-another-from-the-kernel.md) | An app calls another app or a service through a call action the Kernel queues and sends; `connects` and open GET routes for direct reads | 0011 | |

## Which record answers a question

| Question | ADR |
| --- | --- |
| Who may open an app, and who may read a row? | 0001, 0002, 0011 |
| Who may call an HTTP route, and how is a request signed? | 0011, update "a route binds its action" |
| How does an app call another app or a service? | 0015 |
| What does the agent run, and where? | 0003 |
| Where does a build run, and what does the Kernel accept from it? | 0005, 0007 |
| How does a schema change reach the database? | 0004, 0007 |
| Which permission does a tool need? | 0008, 0011 (`manage_users`), 0013 (`export_data`), 0005 note on Eject (`manage_lifecycle`) |
| Why do the editor and an exported Kernel refuse the same statement? | 0009 |
| What is counted, and who reads it? | 0010 |
| How does an account prove its email, and how does a creator get in? | 0011, 0012 |
| What leaves with an app, and what stays behind? | 0013, 0014 |
| What does a restore bring back? | 0005 note of 2026-11-06, 0011 notes on API keys, 0014 |
| Which headers keep hosted code on its own origin? | 0008 update of 2026-11-06 |
