# Architecture decisions

Each record states its status and which earlier record it amends on its first line, and keeps its later changes as dated notes at the end, oldest first. The next free number is 0028. Before taking it, check `main`, because another branch may have taken it.

| ADR | Decides | Amends | Amended by |
| --- | --- | --- | --- |
| [0001](0001-separate-runtime-and-admin-data-access.md) | Browsers reach data only through named Server Actions; owner tools use a separate Admin Interface | | 0008, 0011 |
| [0002](0002-kernel-owns-identity-and-visibility.md) | The Kernel, not a provider, owns sessions, grants, visibility and signup policy | | 0011 |
| [0003](0003-in-browser-agent-on-pi-agent-core.md) | The build agent runs in the browser on pi-agent-core and edits a scratch copy behind the Validation Gate | | 0004, 0022 |
| [0004](0004-use-drizzle-schema-as-the-managed-database-model.md) | Superseded by 0027. `schema.ts` in Drizzle is a Managed Project's Database Model; migrations derive from it | 0003 | 0027 |
| [0005](0005-build-artifacts-separate-managed-and-external-projects.md) | Superseded by 0027 except the static publishing rules. The Build Artifact is the publishing contract; Eject moves source authority; one folder per Deployment; the static publishing rules | | 0007, 0027 |
| [0006](0006-external-projects-use-a-local-development-kernel.md) | Superseded by 0027. External Projects develop against a local Development Kernel | | 0027 |
| [0007](0007-managed-builds-run-in-the-browser.md) | Managed builds run in the browser; schema derivation runs in a subprocess, never in the Kernel process | 0005 | |
| [0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md) | Authenticated MCP is the operation protocol for the editor, agents and apps; hosted-code isolation headers; the file lifecycle | 0001, 0002 | 0013 |
| [0009](0009-the-gate-and-the-kernel-share-one-query-bound.md) | A query needs a top-level `LIMIT`, and the Gate and the Kernel refuse statements with the same words | | 0020 |
| [0010](0010-kernel-owns-usage-and-signals.md) | The Kernel keeps Usage and Signals per Deployment; a mutation may declare a guard | | 0017 |
| [0011](0011-one-verified-account-core-for-creators-and-app-users.md) | One verified account core for creators and app users; invitation codes; app invitations; HTTP routes, callers and API keys | 0001, 0002 | 0014, 0026 |
| [0012](0012-account-email-goes-through-a-persistent-queue.md) | Account email leaves through a persistent queue with a sender preflight and limits | | 0016 |
| [0013](0013-an-app-leaves-as-a-chunked-package.md) | An app leaves as a chunked package behind `export_data`; API keys travel revoked | 0008 | 0014, 0025 |
| [0014](0014-an-app-migrates-with-one-writer-at-a-time.md) | Superseded by 0027 except the maintenance state and the revocation record. A migration keeps one writer through maintenance and a confirmation; a restore applies the revocation record | 0011, 0013 | 0027 |
| [0015](0015-a-deployment-answers-on-one-custom-domain.md) | A Deployment answers on one custom domain proved by a TXT record; a package carries the name without its token | 0008, 0013, 0014 | |
| [0016](0016-a-mutation-may-notify-the-owner-or-the-actor.md) | A mutation may queue one email to the Project Owner or to the caller's verified email when it changes a row | 0010, 0012 | 0021 |
| [0017](0017-the-kernel-runs-a-declared-schedule.md) | A mutation may declare a schedule that the Kernel's timer runs once a period; `subscribe` is refused | 0010 | 0021 |
| [0018](0018-a-route-verifies-a-providers-own-signature.md) | A route verifies a provider's own signature scheme and reads its nested body through `fields` | 0011 | |
| [0019](0019-an-app-calls-another-from-the-kernel.md) | An app calls another app or a service through a call action the Kernel queues and sends; `connects` and open GET routes for direct reads | 0011 | |
| [0020](0020-a-refusal-carries-a-stable-code.md) | Every refusal from the Gate and the Kernel is `{ code, message, hint }` from one shared table | 0009 | |
| [0021](0021-a-notice-lists-its-rows-and-reaches-the-accounts-they-name.md) | A notice lists the rows its mutation changed, shows the run's time, and may reach each verified account a column of those rows names | 0016, 0017 | |
| [0022](0022-the-build-agent-operates-the-platform-through-the-owners-mcp.md) | The editor's build agent reaches the Kernel through the owner's own MCP session, pinned to the open project, and still edits files only behind the Gate | 0003 | |
| [0023](0023-an-app-database-runs-in-wal-mode-with-kept-connections.md) | An app Database runs in WAL mode with `synchronous=NORMAL`; Server Actions borrow kept connections, queries run side by side, mutations one at a time | | |
| [0024](0024-an-installation-names-its-hosts-from-one-domain-and-an-app-may-answer-on-a-path.md) | Accepted. `KONAE_DOMAIN` derives `app.<domain>` and the app hosts, and the app whose path is named `konae-home` is the home; an owner may put an app on `/p/<id>` or `/p/<name>`, sharing one origin | | |
| [0025](0025-a-deployments-file-bytes-may-live-outside-its-folder.md) | File quotas are installation settings; a Deployment's file bytes may live in an S3 bucket while its index stays in its folder; a copy carries the bytes | 0013 | |
| [0026](0026-an-app-account-may-sign-in-with-the-apps-own-google-client.md) | An app account may sign in with the app's own Google client, on the app's host, matched by an email Google manages; the owner's unproven account is not taken back | 0011 | |
| [0027](0027-an-app-is-one-folder-that-the-kernel-serves-with-or-without-konae.md) | Proposed. One app folder everywhere; export and import only; owners live in Konae and users in the app; the Kernel is a library Konae is built on; `schema.sql` is the only model; `dist/` is the publishing contract | 0001, 0004, 0005, 0006, 0008, 0011, 0012, 0013, 0014, 0015, 0016, 0019, 0026 | |

## Which record answers a question

| Question | ADR |
| --- | --- |
| Who may open an app, and who may read a row? | 0001, 0002, 0011 |
| How does an app account sign in with Google? | 0026 |
| Who may call an HTTP route, and how is a request signed? | 0011, update "a route binds its action" |
| How does an app call another app or a service? | 0019 |
| What does the agent run, and where? | 0003, 0022 |
| Where does a build run, and what does the Kernel accept from it? | 0005, 0007 |
| How does a schema change reach the database? | 0004, 0007 |
| Which permission does a tool need? | 0008, 0011 (`manage_users`), 0013 (`export_data`), 0005 note on Eject (`manage_lifecycle`) |
| Why do the editor and an exported Kernel refuse the same statement? | 0009 |
| What is counted, and who reads it? | 0010, 0017 (`scheduled`) |
| Who may an app email, and when? | 0012, 0016, 0021 |
| What runs when nobody calls the app? | 0017 |
| How does an account prove its email, and how does a creator get in? | 0011, 0012 |
| What leaves with an app, and what stays behind? | 0013, 0014, 0024 |
| What does a restore bring back? | 0005 note of 2026-11-06, 0011 notes on API keys, 0014 |
| Which headers keep hosted code on its own origin? | 0008 update of 2026-11-06 |
| How does an app answer on its own domain? | 0015 |
| Which host serves what, and how does an app answer on a path? | 0024 |
| How does a route take a webhook from an outside service? | 0011, 0018 |
| What shape does a refusal have, and who fixes it? | 0009, 0020 |
