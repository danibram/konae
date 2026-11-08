# Architecture decisions

Each record states one standing decision as it holds now, with its reason and the options it rejected. When a decision changes, its record changes, and the git history keeps the earlier text. A new decision that fits no record gets the next free number, 0015.

| ADR | Decides |
| --- | --- |
| [0001](0001-an-app-is-one-folder-that-the-kernel-serves-with-or-without-konae.md) | One app folder everywhere; the Kernel is a library and a binary, Konae is built on it; owners live in Konae and users in the app |
| [0002](0002-browsers-reach-data-through-named-server-actions-and-owners-through-konae.md) | Browsers reach data only through named Server Actions; the Kernel owns visibility and the Signup Policy; owners operate through Konae's MCP; hosted code stays on its own origin |
| [0003](0003-an-account-proves-its-email-before-it-holds-a-session.md) | One verified account core for owners and app users; invitations; owner admission; Google sign-in with the app's own client |
| [0004](0004-the-gate-and-the-kernel-refuse-with-the-same-code-and-words.md) | The Validation Gate and the Kernel share every rule, the query bound and one table of refusal codes |
| [0005](0005-schema-sql-is-the-only-model-and-a-release-is-all-or-nothing.md) | `schema.sql` is the only model and the Kernel derives the change; a release swaps everything or nothing; WAL with kept connections |
| [0006](0006-dist-is-the-publishing-contract-and-the-editor-builds-in-the-browser.md) | `dist/` is the publishing contract and its static rules; the editor builds in the browser; npm packages through `package.json` |
| [0007](0007-the-build-agent-edits-a-scratch-copy-and-operates-through-the-owners-mcp.md) | The build agent runs in the browser, edits a scratch copy behind the Gate, and uses the owner's MCP pinned to the open project |
| [0008](0008-an-app-leaves-by-export-and-returns-by-import.md) | Export and import of the app folder; maintenance; copies, restore and the revocation record |
| [0009](0009-file-bytes-go-through-one-store-and-the-index-stays-with-the-app.md) | Files belong to their uploader; quotas are installation settings; the bytes go through one file store |
| [0010](0010-the-kernel-keeps-usage-and-signals.md) | Usage and Signals in the Kernel's own store; a mutation's guard |
| [0011](0011-the-kernel-runs-a-declared-schedule.md) | A mutation may declare a schedule that only the Kernel's timer runs |
| [0012](0012-email-leaves-through-one-queue-to-addresses-the-app-has-proved.md) | One persistent mail queue with limits; a mutation's notice to the contact email, the actor or the accounts its rows name |
| [0013](0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md) | HTTP routes, callers, API keys, signature schemes and secrets; call actions the Kernel queues and sends |
| [0014](0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md) | `KONAE_DOMAIN` names the hosts; a custom domain proved by TXT; a path address on a shared origin the owner accepts |

## Which record answers a question

| Question | ADR |
| --- | --- |
| What is an app, and what does the Kernel do without Konae? | 0001 |
| Who may open an app, and who may read a row? | 0002 |
| Which permission does a tool need? | 0002 |
| Which headers keep hosted code on its own origin? | 0002 |
| How does an account prove its email, and how does an owner get in? | 0003 |
| How does an app account sign in with Google? | 0003 |
| Why do the editor and an exported Kernel refuse the same statement? | 0004 |
| What shape does a refusal have, and who fixes it? | 0004 |
| How does a schema change reach the database? | 0005 |
| What happens when a publish fails half way? | 0005 |
| Where does a build run, and what does the Kernel accept from it? | 0006 |
| What does the agent run, and where? | 0007 |
| What leaves with an app, and what stays behind? | 0008 |
| What does a restore bring back? | 0008 |
| How does an owner move an app without losing a write? | 0008 |
| Where do uploaded files live, and who may read one? | 0009 |
| What is counted, and who reads it? | 0010 |
| What runs when nobody calls the app? | 0011 |
| Who may an app email, and when? | 0012 |
| Who may call an HTTP route, and how is a request signed? | 0013 |
| How does a route take a webhook from an outside service? | 0013 |
| How does an app call another app or a service? | 0013 |
| Where does a secret live? | 0013 |
| How does an app answer on its own domain? | 0014 |
| Which host serves what, and how does an app answer on a path? | 0014 |
