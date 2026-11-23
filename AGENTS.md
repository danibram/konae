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
| Another service calls this app | Turn on `http` with `module_set`, then declare it on an anonymous action with callers / signature / key | A call action (that is outbound) |
| Code between an HTTP request and the rows | A server function in `functions.json`, with its actions allowlist | A Server Action with string-built SQL, or a call action |
| This app calls another host | Turn on `calls` with `module_set`, then declare `kind: call` | `fetch` of a secret from the browser |
| Uploaded files | `miniapp.files` | Large bytes in a SQLite column |
| Every signed-in user reads every uploaded file | `files.fileAccess` set to `app-users` | Each upload as public |
| Who may create an app account | Signup Policy and invitations | A homemade users table that bypasses Runtime Actor |
| An administrator inside the app | A role column in the app's own table, checked in SQL | An owner account inside the app |
| Turn an app capability on or set its options | `modules_read` and `module_set`, then read it back with `modules_read` | A separate tool or a setting in the Capsule |
| Be listed in search | Turn on `seo` with `module_set`, after `modules_read` | Assume a public app is listed |
| A short link on the installation's domain | path address (`path_set`) | A custom domain when DNS is not yours |
| Take the app elsewhere | export and import of the app folder | Copy (moves the installation) |
| Debug or rehearse a source change against published rows | `konae pull --data-only`, `dev --reset`, local `dev`, then `rehearse` | Publishing to find out |
| Put data from the folder, or from another system, into the published app | `konae push` (an import of the folder's `data/`, after a backup) | A migration script that drives transfers and imports by hand |
| See the email, calls, scheduled runs and Signals of a local run | `konae events` | Reading the Kernel's files by hand |

Prefer a declaration inside the Capsule over a new platform resource. Topic `pieces` of `platform_guide` is the same table for agents.

## First run

Keep this path short. If it grows, fix the path before adding docs.

1. App that runs: editor apply through the Gate, or `konae init` + `dev`, which need no credential.
2. Credential, to publish or to operate: `konae login` then `publish`, or MCP `platform_guide` topics `rules`, `pieces`, `credentials`.
3. Proof: open the Deployment URL, call one Server Action, then `project_inspect` (and `conform` when checking a contract).

## Release Konae

1. Read [Releasing](./docs/releasing.md). Check open PRs and pending local work, integrate the release scope into `main` and review the diff from the last release tag. Run the editor build and the checks that exercise the changed Kernel and editor paths.
2. Put the user-facing features, upgrade changes and fixes in the version's `CHANGELOG.md` section. Commit that section and the matching JS and Rust versions, tag the release commit and push `main` and the tag. Watch the Release workflow and confirm the GitHub release body matches the changelog, with the binaries and checksums attached.
3. Use the installation's private runbook for any rollout. Confirm the running service uses the published image digest and passes its health check.

## Named divergences

These are not bugs to paper over. Say them when they matter.

- The editor preview applies no Content Security Policy. Inline handlers, other-origin fetches and WebAssembly can still run there; a published app and `konae dev` refuse them. Their `script-src` is `'self'` plus each inline script's hash, with no `'wasm-unsafe-eval'` or `'unsafe-eval'`, so a library that compiles WebAssembly or calls `eval` fails there.
- The preview has no HTTP routes, schedule timer, notice email, Usage store or call outbox. Try those on `konae dev` or a published Deployment.
- The preview opens one `srcDoc` document in an iframe. It has no app URL or page history, so it cannot resolve declared page paths or populate `miniapp.page.params`. Try page paths on `konae dev` or a Deployment. It writes no `<head>` tags for row values. It uses the project's saved `files.fileAccess` option and does not read a published Deployment's module state, so an owner change may not reach the preview until its project state is refreshed.
- The editor preview refuses a file over 2 MiB. A Kernel refuses at its own `KONAE_FILE_MAX_BYTES`, 2 MiB unless the operator raised it, and names the number. `konae dev` applies the same 2 MiB per file, and `KONAE_FILE_MAX_BYTES` raises it there. Its quota for all files is 1 TiB unless `KONAE_FILES_QUOTA_BYTES` names another.
- The editor preview has no Google sign-in. `miniapp.auth.signInWithGoogle()` refuses there; try it on a published Deployment once its owner has set the app's Google client and turned Google on.
- The editor preview writes no canonical, card, `robots.txt` or sitemap. `konae dev` writes them using the local host as the address.
- The editor's build reads npm packages from esm.sh; `konae build` and an app folder's `npm run build` read them from `node_modules`. Both use the exact versions in `package.json`, but the bundles are not byte-identical.
- When actions is on, the Validation Gate checks schema and `actions.json`. When files is on, it checks file access declarations. Both doors refuse a Capsule whose used module is off. The builder refuses a Capsule whose pages are all whole documents that load no script only when a module with a browser call is on. A page that names a missing file fails at build or release, not at `validate_capsule`.
- The editor preview re-runs `schema.sql` on its own test rows and never changes a table it already has. A Kernel derives the schema change at publish. When preview lacks a declared table or column, the Gate reports `PREVIEW_BEHIND`, names the difference and says to try `konae dev` or a Deployment; its summary marks row checks skipped.
- `konae dev` sends no email. Account email and notices land in `.konae/kernel/mail-testbox.jsonl`, and `konae events` lists them.

## Where the work lives

- `src/` is the editor. `src/components/ProjectDashboard.tsx` is the project list, `src/components/AgentPanel.tsx` is the conversation, and `src/App.tsx` is the file editor.
- `src/agent/system-prompt.ts` is what the build agent is told. `src/agent/gate.ts` is the Validation Gate. `src/lib/query-bound.ts` is the row limit both doors share.
- Backend ownership and paths live in [Konae architecture](docs/architecture.md#modules-of-kernel). Start with the module that owns the rule. The Kernel's `app_folders/layout.rs` names the paths in an app folder, and `server/operator.rs` holds the `kernel` commands.
- `kernel-rs/konae` links the Kernel and owns the Owner's MCP and Projects. `guide.rs` defines the `platform_guide` topics, including `rules` and `pieces`.
- `src/kernel/actions.ts` parses `actions.json` in the editor, and `kernel-rs/kernel/src/actions/shape.rs` parses it in the Kernel. Shared case files live in `kernel-rs/kernel/src/`.
- `scripts/konae.ts` is the `konae` CLI. Its remote commands are calls to the owner's MCP, and `src/deployments/release.ts` holds the publish sequence the editor runs too. `src/lib/app-folder.ts` writes the files every app folder carries, its own `AGENTS.md` among them.
- `CONTEXT.md` holds the language. `docs/adr/` holds the decisions. `docs/architecture.md` is the shape. `examples/product-fit/` is the set of apps used to see what the Kernel can publish.
- `docs/` is the documentation, in markdown, and its only source. `docs/site.json` is the navigation of the docs site. `bun run docs:site` writes the pages into `examples/site`, the app that renders them under `/docs/`, and `scripts/docs-site.test.ts` checks every link.
- `examples/site` is the home, the docs and `llms.txt` in one app, answering konae.es as its custom domain. `bun run ui:render` writes its `styles.css` from the design system.

## Critic

Judge the running editor at `http://localhost:4173` against the sentence at the top and the five jobs. The project list is `/`. A project opens at `/app/<id>`. A fresh owner has no projects and lands on the project list; projects live on the installation, not in the browser. `/design-system` is the component gallery. Owner sign-in is `/_konae/owner/login`. The editor opens it directly. `/platform-access` serves Clerk sign-in only. The docs and the architecture map are Konae apps, `examples/site` and `examples/architecture-map`; the README of each says how to publish it on a Kernel. The theme follows the system. Do not clear storage.
