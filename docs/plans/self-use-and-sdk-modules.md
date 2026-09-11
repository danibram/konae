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
| Databases, accounts and files live only on the Kernel volume | `docs/architecture.md`, README | Losing the volume loses every app's state |
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

### Deliveries

| Delivery | Work | Done when |
| --- | --- | --- |
| 5. Module contract | Write the contract for `auth`, `actions` and `files` in one file under `docs/`. Each call lists inputs, outputs, errors and limits. Add a test that fails when `runtime.js`, `system-prompt.ts` or `platform_guide` name a call the contract does not, or miss one it does | The three readers agree by test, not by review |
| 6. Rendered guidance | The SDK section of the build agent's prompt and the SDK topic of `platform_guide` are rendered from the contract file at build time | Editing the contract changes both; editing either by hand fails the test |
| 7. HTTP module | The `http` key in `actions.json`, parsed in `src/kernel/actions.ts` and `kernel-rs/src/main.rs`. Routing, signature check, owner-set secrets, Gate refusals. A product-fit example that receives a webhook and serves a JSON endpoint | The status feed example accepts a signed POST from `curl` and refuses an unsigned one. The replay in `examples/product-fit/smoke.ts` covers it. The contract file documents the module and the two agents receive it through delivery 6 |
| 8. Portable action contract | For every project, generate a document of its actions with inputs, output shapes and HTTP bindings. Include it in the export kit | A second backend could run the same conformance checks. No second backend is built here |

### Order

Delivery 5 before 7, so the HTTP module is the first one born from the contract. Delivery 6 can follow 5 at any time. Delivery 8 last.

## What this plan leaves out

Creator signup with Clerk for third parties, email verification, invitations, Eject, migration between installations, teams, custom domains and billing stay in the [launch plan](platform-launch.md). A server-side sandbox for arbitrary code stays a research item and gets its own document if it is ever opened.

## Evidence

Every delivery names a command or test as its condition. When a delivery lands, this document records the command and its result under the delivery, and the affected ADRs, `docs/mcp.md` and `CONTEXT.md` are updated in the same change.
