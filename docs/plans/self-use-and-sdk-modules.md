# Put Littlebox to use, then grow the SDK by modules

Revision of November 6, 2026. This plan has two parts. The first takes the current repository to a hosted installation the owner can trust with real data. The second makes the backend pieces reusable across projects by documenting them as modules of one SDK that both agents understand. It does not change accepted ADRs and does not describe functions as implemented when they are not.

The [launch plan](platform-launch.md) describes a public beta for invited creators. That scope stays valid and comes after this plan. Nothing here contradicts it; this plan is the shorter path to a single operator using Littlebox for their own projects.

## Part 1. Use it now

### Starting point

The editor builds, the Kernel passes its tests and the local stack works. What stands between the repository and a trusted installation is small and known.

| Found | Where | Effect |
| --- | --- | --- |
| The work tree holds the whole Clerk session uncommitted | `git status` | Nothing from it is durable |
| The export kit omits `clerk.rs` while `main.rs` declares it | `src/lib/export-project-sources.ts` | An exported app does not compile on another machine. Job 4 is broken |
| Sign-out over app MCP revoked the session but dropped the clearing cookie, so the browser kept a dead cookie | `kernel-rs/src/main.rs`, `auth_logout` | Fixed in delivery 1 |
| One test expected an older permission message and one used stale input | `transfer_cancel` scope message, harness test | Fixed in delivery 1 |
| Databases, accounts and files live only on the Kernel volume | `docs/architecture.md`, README | Losing the volume loses every app's state. Addressed in delivery 2 by scheduled copies and `littlebox-kernel restore` |
| Docker images not verified on a remote host | `docs/research/product-fit.md` | No production recipe exists |

### The disk is the authority

Everything an app is lives on the Kernel's data directory, a Docker volume in production. Today a Deployment's folder holds `deployment.json`, `database.sqlite`, `identity.sqlite` and `files/`, while its Build Artifact and its backups sit in sibling trees keyed by the same ID, and S3 may hold a copy of the artifact and the record. This plan makes the folder the unit.

```text
deployments/<id>/
├── deployment.json     # record, visibility, signup policy
├── artifact/           # the compiled static HTML, CSS and JS the Deployment serves
├── database.sqlite     # the app's rows
├── identity.sqlite     # its accounts and sessions
├── files/              # uploads
├── usage.sqlite        # Usage and Signals, ADR-0010
└── backups/            # consistent snapshots of the three databases
```

A copy of the folder is the app. The export kit is that folder plus the Kernel. The scheduled copy off the volume is that folder sent to a second disk or to S3; both are copy targets, neither is required to serve. S3 stops being the artifact's home and becomes one optional destination. Static by default means exactly this: no process per app, and nothing an app needs that is not a file in its folder.

### Deliveries

| Delivery | Work | Done when |
| --- | --- | --- |
| 1. Durable state | Commit the current tree. Add the missing Kernel source to the export kit. Fix the HTTPS cookie path and the two stale tests | `bun test`, `cargo test` and `bun run build` pass. `bun run tests/smoke/portable-export.ts` compiles the exported Kernel and signs in with the original account |
| 2. One folder per app, copied off the volume | Move the artifact and the backups into the Deployment's folder. A consistent snapshot of the folder's three databases, then a copy of the whole folder to a second disk or an S3 prefix on a schedule, plus a restore command that installs a folder into an empty data directory. The Kernel already reads `databases/<id>/head.json` from the data bucket at startup and nothing writes it; this delivery replaces that path with the folder copy. Sessions are invalidated on restore | A Kernel started on a fresh volume with the copied folders serves the same apps with their artifact, accounts, rows and files. The check runs in a test, not by hand, against a disk target and an S3 target |
| 3. Production recipe | One documented host with HTTPS, a real `LITTLEBOX_ADMIN_KEY`, `LITTLEBOX_TRUSTED_PROXIES`, S3 for artifacts and the data bucket from delivery 2. Verify the two Docker images on that host | The owner signs in, publishes a project from the editor and from the CLI, and the app answers on its hostname over HTTPS. The steps are in the README |
| 4. Scope statement | Record in the README what this installation is for and what it is not: a single operator, known app users, Allowlist without email verification | A reader knows that strangers need the launch plan's deliveries 1 and 2 before they are let in |

Clerk stays dormant while its variables are unset. The local owner account with the installation key is the operator account for this installation. Creator identity stays provider-independent: when creators beyond the operator are admitted, they get the same email and password accounts the Kernel already issues to app users, and Clerk or any OIDC provider is an optional adapter, never a requirement to sign in or to run an installation. Email, invitations, Eject and migration are not in this part.

### Delivery 2 record

Landed on 2026-11-06 on branch `deliv/2-app-folder-copies`. The artifact and backups now live in `deployments/<id>/artifact/` and `deployments/<id>/backups/`; a Kernel moves the older layout on start and refuses to start when both locations hold files. `LITTLEBOX_COPY_DIR` and `LITTLEBOX_COPY_S3_PREFIX` turn on the scheduled copy, `LITTLEBOX_COPY_INTERVAL_SECONDS` sets its period, and `/_littlebox/health` reports it under `copies`. `littlebox-kernel restore --from <dir | s3://bucket/prefix>` installs a copy into an empty data directory. The startup read of `databases/<id>/head.json` and of `deployment.json` records from the data bucket is removed, and the Kernel no longer writes those records there.

| Command | Result |
| --- | --- |
| `cargo test --manifest-path kernel-rs/Cargo.toml` | 67 passed, 9 of them in `copies.rs`: migration, migration conflict, copy IDs, retention, settings, copy and restore of artifact, rows, accounts and files, refusal of damaged, unfinished and escaping copies, a copy waiting on a write lock, a failed copy in health |
| `bun test tests/copy-restore.integration.test.ts` | Directory target passed against the real binary. The S3 case is skipped when MinIO does not answer on `LITTLEBOX_TEST_S3_ENDPOINT`, default `http://127.0.0.1:9000` |
| `bun test` | 338 passed, 1 skipped (S3 case), 5 failed. The 5 are `database-lifecycle/server.test.ts`, which cannot write `/tmp` inside the sandbox the run used |
| `bun run build` | Passed |
| `bun run tests/smoke/portable-export.ts --port 19450` | Passed |

Two parts of the plan's text did not match the code. ADR-0010's Usage store does not exist yet, so a Deployment folder has two databases today; the copy takes `usage.sqlite` too once it appears. The done condition asks for an S3 run; MinIO was not reachable for this delivery, so the S3 case ran only against a throwaway S3 emulator and still needs one run against MinIO with `docker compose up minio minio-init`.

### Order

Delivery 1 first, alone. Delivery 2 before any data the owner cares about. Deliveries 3 and 4 together.

## Part 2. The SDK as modules

### What exists

The published app and the preview share one host surface, `window.miniapp`, in `src/mcp/runtime.js`. It has three modules today: `auth`, `actions` and `files`. The build agent learns it from a ten-line section of `src/agent/system-prompt.ts` that ends with "there is nothing else". An external agent learns the platform from `platform_describe` and `platform_guide` over MCP.

Those three descriptions are written by hand in three places. They agree today because the surface is small. They will stop agreeing as soon as a fourth module lands.

### Decision

One SDK, several modules. There is no separate library per backend piece. A module is a namespace on `miniapp` with a contract: its calls, their inputs, their outputs, the errors they return and the limits that apply. The contract is written once and the three readers derive from it.

| Reader | What it needs | Source today | Source after this plan |
| --- | --- | --- | --- |
| Browser runtime | The calls and their transport | `runtime.js` | `runtime.js`, checked against the contract by a test |
| Build agent in the editor | What each call does, returns and refuses | `system-prompt.ts` | Rendered from the contract |
| External agent over MCP | The same, plus how to publish | `platform_guide` in `kernel-rs/src/mcp.rs` | Rendered from the contract |

The two agents differ in what they may do, not in what they know. The editor agent proposes through the Validation Gate. The external agent publishes through the Kernel. Both read the same module contract. That is how the same app comes out of a chat and out of a code editor.

A module adds a door to an existing primitive. It does not add a primitive. The product rule in `docs/architecture.md` stands: a mini-app creates no process, VM or container. Free-form server functions were rejected in the [portable apps plan](portable-personal-apps.md) and stay rejected.

### The first new module: HTTP on actions

Product-fit already names it. A Server Action may declare an HTTP binding.

```json
{
  "kind": "mutation",
  "access": "anonymous",
  "http": { "method": "POST", "path": "/hooks/orders", "signature": { "header": "X-Signature", "secret": "orders_webhook" } },
  "statement": "INSERT INTO orders (external_id, total) VALUES (?1, ?2)",
  "bindings": [
    { "source": "input", "key": "id", "type": "string", "maxLength": 64 },
    { "source": "input", "key": "total", "type": "number" }
  ]
}
```

The Kernel routes the path to that action on the Deployment's hostname and nothing else. The action keeps its access level, bindings, query bound, rate limit and Usage. A signed binding verifies the request body against a secret the owner sets through the Admin Interface; the secret never enters `actions.json` or the artifact. A `query` action with `"method": "GET"` returns its rows as JSON. Unknown POST paths keep returning 405; GET routing stays as it is today.

Visibility does not decide whether an HTTP binding answers; the action's access level does. A private Deployment still runs nothing in the browser without an app session, but an action declared `anonymous` with an `http` binding answers on that path whatever the Deployment's visibility, and its signature, when declared, is the check. An `authenticated` action keeps requiring an app session, so in this delivery it may not carry an `http` binding; the Gate and the Kernel refuse that combination. Delivery 7 records this rule as a dated note on ADR-0002 before any code, since that ADR ties anonymous actions to public Deployments today.

The Validation Gate parses the `http` key the same way the Kernel does, as it does for `signal` under ADR-0010. A path collision, a `GET` on a mutation or an `http` binding on an `authenticated` action is refused in the editor and by the Kernel with the same message.

This is the module that lets a project expose an API and receive a webhook without a server of its own, which is the "cloud functions" need in practice.

#### Who may call a route, and who may fetch an asset

An HTTP route is reached by three kinds of caller, and the Kernel resolves which one before any SQL runs:

| Caller | Proof | Resolves to |
| --- | --- | --- |
| A third-party service | The signature declared on the binding | The action's `anonymous` access |
| A program the owner wrote | An API key the owner issued for that Deployment, sent as a bearer token | A Runtime Actor of kind `service`, with the key's own ID, bound to the actions and file visibilities the key names |
| A signed-in person | The app session cookie | The usual Runtime Actor |

API keys are a third kind of Runtime Actor next to the anonymous visitor and the signed-in app user. The owner issues, scopes and revokes them from the Admin Interface and MCP; the Kernel stores the hash only; a key is bound to one Deployment and never to the owner. An action's `actor` binding yields the key's ID, so rows written by a key have an owner like any other. Export carries keys' metadata and hashes with the identity file; migration revokes them like sessions.

Assets stay on the platform. Uploads already have no HTTP download path and are read through the SDK with a session and per-file visibility; the artifact's static files are served by the Kernel and a private Deployment withholds them before login. This delivery adds one HTTP read path for files, `GET /_littlebox/files/<id>`, that accepts the same three callers and applies the file's visibility, so an image can be embedded or fetched by a program without copying it anywhere public. There is no unauthenticated object URL and nothing is served from the bucket directly. Secure by default means the three principals are the only way in, for routes and for files alike.

### Deliveries

| Delivery | Work | Done when |
| --- | --- | --- |
| 5. Module contract | Write the contract for `auth`, `actions` and `files` in one file under `docs/`. Each call lists inputs, outputs, errors and limits. Add a test that fails when `runtime.js`, `system-prompt.ts` or `platform_guide` name a call the contract does not, or miss one it does | The three readers agree by test, not by review |
| 6. Rendered guidance | The SDK section of the build agent's prompt and the SDK topic of `platform_guide` are rendered from the contract file at build time | Editing the contract changes both; editing either by hand fails the test |
| 7. HTTP module | The `http` key in `actions.json`, parsed in `src/kernel/actions.ts` and `kernel-rs/src/main.rs`. Routing, signature check, owner-set secrets, API keys as a `service` Runtime Actor, the file read path, Gate refusals. A product-fit example that receives a webhook, serves a JSON endpoint and embeds a private image through a key | The status feed example accepts a signed POST from `curl` and refuses an unsigned one. The replay in `examples/product-fit/smoke.ts` covers it. The contract file documents the module and the two agents receive it through delivery 6 |
| 8. Portable action contract | For every project, generate a document of its actions with inputs, output shapes and HTTP bindings. Include it in the export kit | A second backend could run the same conformance checks. No second backend is built here |

### Evidence for deliveries 5 and 6

Recorded on 2026-11-06.

The contract is `src/sdk/contract.ts`, a typed literal, not a file under `docs/`. The prompt, the generator and the test import it and `tsc` checks its shape; `docs/sdk.md` is its rendering under `docs/`. A module has `calls` and `declarations`, so the HTTP module of delivery 7 can be one entry that declares a key in `actions.json` without a browser call.

`bun run sdk:render` writes `src/agent/sdk-section.generated.ts`, `kernel-rs/src/sdk-guide.json` and `docs/sdk.md`. The Kernel serves the second with `include_str!`.

| Check | Command | Result |
| --- | --- | --- |
| `runtime.js` and the preview client expose the contract's calls and no other; `runtime.js` calls exactly the contract's runtime tools; each documented message exists in the Kernel, `runtime.js` or the preview source; the three rendered files are current; the prompt and `platform_guide` name no other call | `bun test src/sdk/contract.test.ts` | 45 pass |
| `platform_guide` returns the included JSON, every tool a call names is a runtime tool, and calls open to signed-out visitors use only tools they get | `cargo test --manifest-path kernel-rs/Cargo.toml platform_guide_sdk_topic_is_the_rendered_contract` | 1 pass |
| The export kit carries `sdk-guide.json` and every other file the exported Kernel build reads with `include_str!` | `bun test src/lib/export-project-kit.test.ts` | 13 pass; removing the JSON from the kit fails it |
| A hand edit to `sdk-section.generated.ts` or `sdk-guide.json`, a call added to or replacing the section in `system-prompt.ts`, a hand-written `sdk` arm in `mcp.rs` and a call removed from `runtime.js` each fail | edit, `bun test src/sdk`, revert | 1 or 2 failures each, as expected |

Delivery 5 found that the hand-written `sdk` topic never mentioned `miniapp.auth.signOut`.

The contract records these differences between the editor preview and the Kernel. None is fixed here.

| Case | Kernel | Editor preview |
| --- | --- | --- |
| Undeclared action | `Server action not declared.` | `Server action not declared: <name>` |
| `input` is not an object | `Arguments must be an object.` | `Action input must be an object.` |
| Required input omitted | `Missing action input: <key>` | The type refusal, e.g. `<key> must be a non-empty string.` |
| Number input of the wrong type | `<key> must be a number.` | `<key> must be a finite number.` |
| Query over 1000 rows or 2 MiB | `Query result exceeds 2 MiB or 1000 rows.` | Two messages, and it counts value bytes instead of JSON bytes |
| Upload over 2 MiB | `Transfer size is outside the limit.` | `Local prototype files are limited to 2 MiB.` |
| Empty upload | Refused with `Transfer size is outside the limit.` | Accepted |
| Private Deployment, no session | Every call refused | Visibility ignored; anonymous actions run |
| Rate limits, 2 s statement deadline | Applied | Not applied |
| Database cap | 256 MiB | 10 MiB |
| `auth.signIn()`, `auth.signOut()` | Navigates to the login page; returns `{ ok: true }` | Selects Alice and returns her session; returns the anonymous session |

`platform_describe` lists five runtime tools and leaves out `auth_logout`, which the runtime endpoint serves.

### Order

Delivery 5 before 7, so the HTTP module is the first one born from the contract. Delivery 6 can follow 5 at any time. Delivery 8 last.

## What this plan leaves out

Creator signup with Clerk for third parties, email verification, invitations, Eject, migration between installations, teams, custom domains and billing stay in the [launch plan](platform-launch.md). A server-side sandbox for arbitrary code stays a research item and gets its own document if it is ever opened.

## Evidence

Every delivery names a command or test as its condition. When a delivery lands, this document records the command and its result under the delivery, and the affected ADRs, `docs/mcp.md` and `CONTEXT.md` are updated in the same change.
