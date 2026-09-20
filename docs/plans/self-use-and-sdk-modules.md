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

Two parts of the plan's text did not match the code. ADR-0010's Usage store did not exist at this delivery, so a Deployment folder had two databases; the copy already took `usage.sqlite` when present, and the Usage delivery below added the file. The done condition asks for an S3 run; MinIO was not reachable for this delivery, so the S3 case ran only against a throwaway S3 emulator and still needs one run against MinIO with `docker compose up minio minio-init`.

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

Visibility does not decide whether an HTTP binding answers; the action's access level does. A private Deployment still runs nothing in the browser without an app session, but an action declared `anonymous` with an `http` binding answers on that path whatever the Deployment's visibility, and its signature, when declared, is the check. An `authenticated` action keeps requiring an app session, so in this delivery it may not carry an `http` binding; the Gate and the Kernel refuse that combination. Delivery 7 recorded this rule as a dated update on ADR-0011, which holds both amendments to ADR-0002, before any code.

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

### Evidence for delivery 7

Recorded on 2026-11-06 on branch `deliv/7-http-module`.

The decision is a dated update on ADR-0011, not a note on ADR-0002. The launch plan asks for one ADR that records both amendments to ADR-0002, and ADR-0011 already records the first. The Kernel side is `kernel-rs/src/http.rs`; `main.rs` and `mcp.rs` gained about 50 lines between them. [The MCP guide](../mcp.md#http-routes) states the signature scheme a sender computes.

| Check | Command | Result |
| --- | --- | --- |
| The Gate and the Kernel refuse the same `http` declarations with the same messages: two actions on one route, GET on a mutation, POST on a query, `http` on an authenticated action, a reserved path in any letter case, a malformed path, a malformed signature, a secret value written into `actions.json` | `bun test src/kernel/actions.test.ts` and `cargo test --manifest-path kernel-rs/Cargo.toml bindings_refuse_what_the_editor_refuses` | 20 cases in `kernel-rs/src/http-binding-cases.json`, run by both. `bun test src/agent/gate.test.ts` adds the Gate's own refusal through `validate` |
| A signature verifies once inside 300 seconds and fails when stale, early, tampered, under another secret, with a moved timestamp or without a timestamp; a release naming an unset secret is refused; a listing never shows a value | `cargo test --manifest-path kernel-rs/Cargo.toml http::` | 3 pass |
| Against a real Kernel: the unset-secret refusal and the publish refusals by message; an anonymous route answering on a private Deployment while its shell stays behind sign-in; 405 for unknown POST paths; signed, unsigned, tampered, stale, foreign, replayed and oversized POSTs; typed query strings; API keys as actor, scope, invalid and revoked; the cookie refused from another site; the file read path for a key, the uploader, another user, a visitor and a public-only key; an export carrying keys and the import revoking them; the anonymous rate limit | `LITTLEBOX_HTTP_TEST_PORT=<free> bun test tests/http-routes.integration.test.ts` | 10 pass |
| The feed accepts a signed POST from `curl`, refuses the unsigned one and its replay, serves `GET /api/incidents`, lets a key read a private badge a visitor cannot, and loads its badge from `/_littlebox/files/<id>` in Chrome with no policy violation | `LITTLEBOX_PRODUCT_FIT_PORT=19443 bun run examples/product-fit/smoke.ts` | 11 checks pass |
| The http module is in the contract; each of its 18 messages is found in the Kernel source | `bun test src/sdk` | 63 pass in `contract.test.ts`; changing one route message in the Kernel fails it |
| The export kit carries `http.rs` and its case file, and the exported Kernel compiles | `bun test src/lib/export-project-kit.test.ts`, `bun run tests/smoke/portable-export.ts --port 19455` | Pass |
| Everything else | `cargo test --manifest-path kernel-rs/Cargo.toml`, `bun run build` | 97 pass; build passes |
| Whole suite | `bun test --timeout 60000` | 428 pass, 2 fail. The two are the sign-in rate-limit tests in `tests/mcp.integration.test.ts`; at a load average of 25 their per-minute buckets refill while the password hashes run. Both pass alone. With the default 5 second timeout the same run had 10 failures, all from tests cut off at 5 seconds |

Where this differs from the text above:

- Signing secrets belong to the Project, in `mcp.sqlite`, and are set with `secret_set`. The plan said the owner sets them through the Admin Interface but did not say where they live; a first publish must find them before its Deployment exists, so they cannot live in the Deployment folder. A portable export does not carry their values; the owner sets them again on the destination before the release.
- The signature is one header, `t=<seconds>,v1=<HMAC>`, rather than a separate timestamp header, so moving the timestamp breaks the MAC. A signature is accepted once; the memory of seen signatures lives in the Kernel process, so a restart inside the 300 second window forgets it.
- An API key that names a signed action stands in for the signature. A session cookie does not, and it counts only from the app's own origin, because app hosts are same-site with each other.
- A route refuses inputs the action does not declare, as `actions.call` does. A sender whose payload carries more fields needs an action that declares them or an adapter.
- `littlebox-kernel restore` keeps API keys live: it restores the same installation, and `copies.rs` is unchanged. An import revokes them.
- ADR-0010's Usage store did not exist at this delivery, so routes recorded no Usage. They share the runtime endpoint's rate-limit buckets. The Usage delivery below counts them under their method and path.
- The editor's Auth panel has no API key screen. Keys and secrets are managed over admin MCP only.
- `smoke.ts` cancelled a download transfer that `McpClient.download` already cancels, which failed its portfolio check on `main` too. The second cancel is removed.

#### Hardening, recorded 2026-11-06 on branch `deliv/http-hardening`

Four open items of this delivery and one from the invitations delivery, closed. The decision is a dated note on [ADR-0011](../adr/0011-one-verified-account-core-for-creators-and-app-users.md) under the HTTP update, and the inviter is a dated note on [ADR-0012](../adr/0012-account-email-goes-through-a-persistent-queue.md). The bullet above about the in-process replay memory no longer holds.

| Check | Command | Result |
| --- | --- | --- |
| A signature verifies once, and a second connection to the same `identity.sqlite` refuses it, as a restarted Kernel would; another path keeps its own memory; entries past 600 seconds are pruned. A static file request does not parse `actions.json`: a parse counter stays at 1 across static and route requests, a republish parses once and drops the old path, and a record with another revision parses on its first request. `HEAD` on a route answers 405 with `Allow: GET` and never opens the app database | `cargo test --manifest-path kernel-rs/Cargo.toml http::` | 4 pass |
| Unknown keys drain a per-client, per-Deployment bucket of 10; another client and another Deployment keep theirs; 1,000 presentations of a valid key take nothing | `cargo test --manifest-path kernel-rs/Cargo.toml failed_api_keys` | 1 pass |
| The invitation template names the inviter with line breaks removed and HTML escaped, and only an invitation names one; a real invitation through the queue stores the inviter on the job and the sent message names the owner's account email; an owner with no email, as under Clerk, is not named; a queue file from before the column gains it | `cargo test --manifest-path kernel-rs/Cargo.toml -- invitations:: mail::` | 37 pass |
| Against a real Kernel: a signed POST accepted, the Kernel killed and started on the same data directory, the same request refused with 409 and one row stored; `HEAD /api/orders` 405 with `Allow: GET` and an empty body while `HEAD /` answers 200; `POST` on a GET route says `Allow: GET`; nine unknown keys on a route and one on the file read path answer 401, the eleventh 429 with `Retry-After`, a valid key from that address 429, the same key from another address 200 | `LITTLEBOX_HTTP_TEST_PORT=<free> bun test tests/http-routes.integration.test.ts` | 13 pass |
| The contract lists the HEAD refusal and the key bucket under `rate-limited`; `bun run sdk:render` regenerated the three readers | `bun test src/sdk` | 72 pass |
| The feed's signed route, replay, unknown key and unbound POST still behave; every example conforms | `LITTLEBOX_PRODUCT_FIT_PORT=19833 bun run examples/product-fit/smoke.ts` | 12 checks pass; the contracts 175 checks, none skipped |
| Everything else | `cargo test --manifest-path kernel-rs/Cargo.toml`, `bun run test`, `bun run build`, `bun run tests/smoke/portable-export.ts --port 19823` | 163 pass; 551 pass, 0 fail; build passes; export smoke passes |
| The editor, whose `src/sdk/contract.ts` and generated agent section changed through `sdk:render`, including the invitation from the Users view | `LITTLEBOX_EDITOR_URL=http://127.0.0.1:4391 LITTLEBOX_TEST_KERNEL_PORT=19841 bun run test:e2e:local` | 7 pass |

Where this differs from the request:

- `limits.rs` and `accounts.rs` changed though the delivery named neither. The key bucket belongs with the other per-client limits, and the inviter reaches the outbox only through `accounts::deliver_issued`.
- The key bucket is its own limiter, 10 failures a minute per client address and Deployment, rather than the login bucket, so a client's wrong keys do not cost it sign-in attempts. Once it is empty, a valid key from that address is refused too, because the refusal comes before the lookup.
- `HEAD` is refused with 405 rather than answered with the `GET` headers, which depend on the caller, the rate limit and the input.
- The route table is cached in a static map in `http.rs`, keyed by Deployment ID and `artifact_version`, not in `AppState`, so no fixture constructor changed.
- The inviter is the email of the owner's account in `owners.sqlite`. A Clerk owner has none, and that invitation names only the app.
- A restore brings back `route_signatures` as the copy held it, so a signature accepted after the copy can be replayed once more while its `t` is still within 300 seconds of the Kernel's clock.

### Evidence for delivery 8

Recorded on 2026-11-06 on branch `deliv/8-action-contract`.

The Action Contract is `action-contract.json`, checked by `src/lib/action-contract.schema.json`, with `action-contract.md` beside it. `src/lib/action-contract.ts` writes all three from `actions.json` and `schema.sql`; `bun run littlebox -- contract --dir <dir>` writes them beside `littlebox.json`, the export kit carries them under `littlebox/`, and the editor lists `action-contract.md` in Files as generated and read-only. `bun run littlebox -- conform --url <app URL> --contract <file>` is the conformance runner, `src/lib/action-conformance.ts`. [The MCP guide](../mcp.md#action-contract) describes both, CONTEXT.md defines the term and ADR-0009 has a dated note. The Kernel is unchanged: it needed no new read tool.

| Check | Command | Result |
| --- | --- | --- |
| Every product-fit example produces a contract that passes the schema check, reads back equal, converts back to the program `parseServerActionSource` returns and writes the same bytes twice; the 16 queries name exactly the columns SQLite's `getColumnNames` returns; 10 more statements cover `*`, `t.*`, aliases with and without `AS`, `DISTINCT`, a compound select, a subquery source and unnamed expressions; types, nullability, a LEFT JOIN, the schema check's refusals and the Kernel's limit constants | `bun test src/lib/action-contract.test.ts` | 29 pass |
| The Gate passes a query that returns `count(*)` without `AS`, with one warning, and `validate_capsule` prints it; the starter has none | `bun test src/agent/gate.test.ts` | 20 pass |
| The runner passes a fake backend that follows each example's contract, and fails variants that accept an undeclared input, refuse a missing one with another message, return an extra column, return null in a NOT NULL column, run an authenticated action for a visitor, accept an unsigned webhook or refuse it with 403; it fails when the contract does not list the refusal it probes, skips rather than passes what needs a session, waits out `rate-limited`, and names a statement error on a valid call | `bun test src/lib/action-conformance.test.ts` | 14 pass |
| The CLI writes the three files, `build` does not pick them up, and a folder without `actions.json` is refused | `bun test scripts/littlebox-contract.test.ts` | 2 pass |
| The export kit carries the contract, its Markdown and schema, and its README names them; an `actions.json` that does not parse stops the export | `bun test src/lib/export-project-kit.test.ts` | 14 pass |
| Every example conforms on a real Kernel: each action's valid call, its refusals, and the feed's signed route, replay, unsigned POST, unknown key, non-JSON body and unbound POST path. The CLI run from a contract file on disk agrees | `LITTLEBOX_PRODUCT_FIT_PORT=19481 bun run examples/product-fit/smoke.ts` | 12 checks pass. The contracts: 175 checks pass, none skipped (launch 17, journal 20, board 45, portfolio 22, poll 11, feed 29, docs 7, probe 24); the CLI on launch: 17 passed, 0 failed, 0 skipped |
| Files shows `action-contract.md` as generated and read-only, and Delete is disabled for it | `bun run test:e2e:local` | 5 pass |
| Everything else | `bun run test`, `cargo test --manifest-path kernel-rs/Cargo.toml`, `bun run build`, `bun run tests/smoke/portable-export.ts --port 19511` | 494 pass; 150 pass; build passes; export smoke passes |

The first live run found what the plan did not foresee. The probe example's `items.jsonin` reads its string input with `json_each`, and the runner's generated string is not JSON, so SQLite refused the valid call. A contract types an input but cannot say what the statement does with it. `conform` therefore takes `--inputs <file>`, values by action that replace the generated ones, and the smoke passes `[1, 2]` for that input. A valid call refused with a message the contract does not list fails and says so.

Decisions the plan left open:

- The contract carries each action's statement and binding order. The plan asked for inputs and output shapes. Without the SQL a second backend could not run the action, and with it the contract converts back to the same `actions.json` program, which is the round-trip the tests check.
- Error messages are not written in the generator. It picks them from the SDK contract, which now records the HTTP status of each route refusal, so `src/sdk/contract.test.ts` keeps them true to the Kernel source.
- An unnamed column is a Gate warning, not a refusal, and the Kernel does not check it. The ADR-0009 note says why and what a refusal would cost.
- `--token` is an app session token, the value of the app session cookie, because the runtime endpoint accepts it as `Authorization: Bearer` and only a Runtime Actor can invoke actions. An owner credential cannot. `LITTLEBOX_APP_EMAIL` and `LITTLEBOX_APP_PASSWORD` sign in for one.
- `conform` writes rows: each mutation runs once with real input. Generated strings carry a per-run suffix so a UNIQUE column does not refuse a second run.
- The SQL tokenizer now records each token's offsets, so the contract quotes an unnamed expression as written.
- The `signal` output shape is in the schema and the types and is not generated, because no action declares a Signal yet.

Not verified: a second backend, which this delivery does not build; types of columns read from a subquery, a table-valued function or an expression other than a known function, which the contract states as `unknown`; nullability through a RIGHT or FULL JOIN on the left side. The editor's agent panel does not show Gate warnings to the person; the agent reads them in `validate_capsule` and the person sees them in `action-contract.md`.

### Order

Delivery 5 before 7, so the HTTP module is the first one born from the contract. Delivery 6 can follow 5 at any time. Delivery 8 last.

### Evidence for Usage and Signals, ADR-0010

Landed on 2026-11-06 on branch `deliv/usage-signals`. `kernel-rs/src/usage.rs` keeps `deployments/<id>/usage.sqlite` with daily totals of session, action, route, upload, file_read, signup and login, and the Signals that actions declare. A queue and one writer thread keep it off the request path. `usage_read` and `signals_read` give it to the owner behind `read`. The scheduled copy and `package_export` carry it. The ADR-0010 note of 2026-11-06 records the bounds and the rules.

| Check | Command | Result |
| --- | --- | --- |
| The Gate and the Kernel refuse the same `signal` declarations with the same messages: a signal on a query, a property that is not an input key, a property listed twice, more than 8 properties, a bad name, a malformed declaration, a signal action without `signal`, with a statement or with `http` | `bun test src/kernel/actions.test.ts` and `cargo test --manifest-path kernel-rs/Cargo.toml signals_refuse_what_the_editor_refuses` | 21 cases in `kernel-rs/src/signal-cases.json`, run by both. `bun test src/agent/gate.test.ts` adds three refusals through `validate`. A signal action counts toward the Gate's cap of 50 |
| The store writes off the request path, keeps 400 days and 100,000 rows of totals and 50,000 Signals, drops the oldest first, cuts a string property at 128 bytes on a character boundary, and refuses bad ranges and cursors | `cargo test --manifest-path kernel-rs/Cargo.toml usage::` | 5 pass |
| A restore keeps API keys live, the small and the package import revoke them, and `usage.sqlite` travels in a copy | `cargo test --manifest-path kernel-rs/Cargo.toml a_restore_keeps_api_keys_live` | Pass |
| Against a real Kernel: each kind counted once per allowed call; Signals from a mutation, a signal action and a route, with their inputs; no count and no Signal for a missing session, an undeclared input, a missing input, a CHECK failure, a bad route body, a wrong password, a repeated signup or an unreadable file; the runtime endpoint and a token for another project refused; range and cursor refusals; a release with a bad property refused by the Kernel; a signal action as the 101st action refused by the Kernel's cap | `LITTLEBOX_USAGE_TEST_PORT=<free> bun test tests/usage-signals.integration.test.ts` | 7 pass |
| A package carries `usage` and the imported app shows the source's totals | `bun test tests/package.integration.test.ts` | 6 pass |
| An exported kit deploys a package with `usage` | `bun run tests/smoke/portable-export.ts --port 19473` | Pass, after `deploy.mjs` learned the component |
| Everything else | `cargo test --manifest-path kernel-rs/Cargo.toml`, `bun run build`, `LITTLEBOX_EDITOR_URL=http://127.0.0.1:4391 LITTLEBOX_TEST_KERNEL_PORT=8891 bun run test:e2e:local` | 156 pass; build passes; 5 of 5 pass |
| Whole suite | `bun run test` | 465 pass, run before the Kernel cap test was added; that test passes in its own file. An earlier run at a load average of 19 failed one sign-in rate-limit test in `tests/mcp.integration.test.ts`, which passed alone, as delivery 7 recorded |
| The Development Kernel records the same way | `bun run littlebox -- dev --dir <project> --port 8899`, one signup and one call of a signal action | `usage.sqlite` held `signup 1`, `action page.like 1` and the Signal `page.liked {"page":"/home"}` |

Where this differs from the brief:

- A route call counts under `route` with its method and path and not again under `action`, so the two kinds add up to the calls made.
- `session` counts `auth_session`, which the SDK calls when an app opens. `signup` counts an account that becomes active, so a signup in required mode counts when its link is answered, and an accepted invitation counts too. `login` counts password sign-ins and answered recovery links.
- A signal action may not declare `http`. Both parsers already refuse it with the method message, because a signal action is neither a query nor a mutation.
- Property values are the inputs the action ran with. Actor, kernel and literal bindings have no key, so a Signal cannot carry who caused it.
- A package lists `usage` only when the store exists, and a Kernel from before this delivery refuses a package that lists it. The exported kit carries the new Kernel, so a kit is unaffected.
- The action cap was 50 in the Gate and 100 in the Kernel before this delivery. A signal action counts toward each; the two numbers still differ.

## What this plan leaves out

Creator signup with Clerk for third parties, email verification, invitations, Eject, migration between installations, teams, custom domains and billing stay in the [launch plan](platform-launch.md). A server-side sandbox for arbitrary code stays a research item and gets its own document if it is ever opened.

## Evidence

Every delivery names a command or test as its condition. When a delivery lands, this document records the command and its result under the delivery, and the affected ADRs, `docs/mcp.md` and `CONTEXT.md` are updated in the same change.
