# Konae

Konae is the guardrail that lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.

That sentence is the yardstick. A review that scores polish without it can call the workshop finished while a person still cannot tell what the checks refused, or cannot take the app with them.

## Jobs

1. A person who does not write code describes an app and gets one that runs.
2. A person who writes code edits the same app's files.
3. Both of them hit the same checks. A query that reads rows names a LIMIT, in the editor and on the Kernel that leaves with the app.
4. Either person can take the app to another machine with its accounts, its data and its rules.
5. Who may open the app, and who may read a row, stay decisions of the app. The host does not replace them.

## How to decide

A new capability belongs in the product only when the Validation Gate and the Kernel refuse it with the same words, and an export of the app folder can carry the app's accounts, data and rules. Anything else is operator work on the installation, not an app feature.

After any write through MCP or the editor, observe the effect through the same door before retrying or telling the owner it worked. `platform_guide` topic `rules` lists the read-back for each write tool. Decide what this credential may do from `session_inspect`'s `can` map, not from memory of login flags.

`CONTEXT.md` holds the words. An owner has a Konae account; an app user has an account in one app; the app folder is the app.

## Pick the smallest piece

| Need | Use | Not |
| --- | --- | --- |
| Page and rows the app owns | Capsule + Server Actions | Raw SQL from the browser |
| Visitor or app user reads or writes a row | Named Server Action with access and actor bindings | Owner `database_execute` in the app UI |
| Mutation matched no row | `guard` on that mutation | A second check statement |
| Email when a row changes | `notify` on that mutation | Browser mail or a call action for mail |
| Work when nobody is looking | `schedule` on a no-input mutation | External cron unless asked |
| Owner-facing occurrence | `signal` on a mutation, or `kind: signal` | A side table only the agent reads |
| Another service calls this app | `http` on an anonymous action with callers / signature / key | A call action (that is outbound) |
| This app calls another host | `kind: call` | `fetch` of a secret from the browser |
| Uploaded files | `miniapp.files` | Large bytes in a SQLite column |
| Who may create an app account | Signup Policy and invitations | A homemade users table that bypasses Runtime Actor |
| An administrator inside the app | A role column in the app's own table, checked in SQL | An owner account inside the app |
| A short link on the installation's domain | path address (`path_set`) | A custom domain when DNS is not yours |
| Take the app elsewhere | export and import of the app folder | Copy (moves the installation) |
| Debug, or try the next version, on what the published app holds | `konae pull` (an export of the data) + `dev --reset` (its import) | Publishing to find out |
| See the email, calls, scheduled runs and Signals of a local run | `konae events` | Reading the Kernel's files by hand |

Prefer a declaration inside the Capsule over a new platform resource. Topic `pieces` of `platform_guide` is the same table for agents.

## First run

Keep this path short. If it grows, fix the path before adding docs.

1. App that runs: editor apply through the Gate, or `konae init` + `dev`, which need no credential.
2. Credential, to publish or to operate: `konae login` then `publish`, or MCP `platform_guide` topics `rules`, `pieces`, `credentials`.
3. Proof: open the Deployment URL, call one Server Action, then `project_inspect` (and `conform` when checking a contract).

## Named divergences

These are not bugs to paper over. Say them when they matter.

- The editor preview applies no Content Security Policy. Inline handlers and other-origin fetches can still run there; a published app refuses them.
- The preview has no HTTP routes, schedule timer, notice email, Usage store or call outbox. Try those on `konae dev` or a published Deployment.
- The editor preview refuses a file over 2 MiB. A Kernel refuses at its own `KONAE_FILE_MAX_BYTES`, 2 MiB unless the operator raised it, and names the number. `konae dev` sets no file limit unless `KONAE_FILE_MAX_BYTES` or `KONAE_FILES_QUOTA_BYTES` names one.
- The editor preview has no Google sign-in. `miniapp.auth.signInWithGoogle()` refuses there; try it on a published Deployment once its owner has set the app's Google client and turned Google on.
- The editor's build reads npm packages from esm.sh; `konae build` and an app folder's `npm run build` read them from `node_modules`. Both use the exact versions in `package.json`, but the bundles are not byte-identical.
- The Validation Gate checks schema and `actions.json`. A page that names a missing file fails at build or release, not at `validate_capsule`.
- The editor preview re-runs `schema.sql` on its own test rows and never changes a table it already has. A Kernel derives the schema change at publish. When the preview lacks a table or column `schema.sql` declares, the Gate warns and passes. Try such a change on `konae dev` or a published Deployment.
- `konae dev` sends no email. Account email and notices land in `.konae/kernel/mailbox.jsonl`, and `konae events` lists them.

## Where the work lives

- `src/` is the editor. `src/components/ProjectDashboard.tsx` is the project list, `src/components/AgentPanel.tsx` is the conversation, and `src/App.tsx` is the file editor.
- `src/agent/system-prompt.ts` is what the build agent is told. `src/agent/gate.ts` is the Validation Gate. `src/lib/query-bound.ts` is the row limit both doors share.
- `kernel-rs/kernel` is the Kernel: a library and the `kernel` binary, which serves app folders alone. `layout.rs` names every path of an app folder, and `operator.rs` holds the `kernel` commands.
- `kernel-rs/konae` is the `konae` binary, which links the Kernel and adds owners, the owner's MCP and the project list. `guide.rs` is what `platform_guide` answers, including `rules` and `pieces`.
- `src/kernel/actions.ts` parses `actions.json` for the editor. `kernel-rs/kernel/src/action_shape.rs` parses it again for every Deployment, including one served from an export. The case files beside it hold the rules both run.
- `scripts/konae.ts` is the `konae` CLI. `src/lib/app-folder.ts` writes the files every app folder carries, its own `AGENTS.md` among them.
- `CONTEXT.md` holds the language. `docs/adr/` holds the decisions. `docs/architecture.md` is the shape. `examples/product-fit/` is the set of apps used to see what the Kernel can publish.
- `docs/` is the documentation, in markdown, and its only source. `docs/site.json` is the navigation of the docs site. `bun run docs:site` copies the pages into `examples/docs-site`, the app that renders them, and `scripts/docs-site.test.ts` checks every link.
- `examples/home` is the page an installation answers on its bare domain, once its path holds the name `konae-home`. `bun run ui:render` writes its `styles.css` from the design system.

## Critic

Judge the running editor at `http://localhost:4173` against the sentence at the top and the five jobs. The project list is `/`. A project opens at `/app/<id>`. A fresh owner has no projects and lands on the project list; projects live on the installation, not in the browser. `/design-system` is the component gallery and `/platform-access` is owner sign-in. The docs and the architecture map are Konae apps, `examples/docs-site` and `examples/architecture-map`; the README of each says how to publish it on a Kernel. The theme follows the system. Do not clear storage.
