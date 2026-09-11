# Product fit from building eight apps

Date: 2026-10-24.

Littlebox can support small applications built from a static frontend, named SQL actions, app accounts and shared attachments. The strongest cases are private trackers, small team tools, forms, polls and dashboards. It is not yet a general backend for websites and APIs. Public media, conventional webhooks, account recovery and replacing app authentication remain missing capabilities.

This assessment comes from building and publishing eight examples through MCP, then testing their operations against an isolated Kernel. The source and replay live in [examples/product-fit](../../examples/product-fit/README.md). These are capability probes. Successful MCP calls do not establish visual quality, accessibility or production readiness.

## Coverage

| Example | Working path | Remaining constraint |
| --- | --- | --- |
| Launch page and waitlist | Anonymous signup action, unique email, public count | No email verification or outbound notification; no dedicated build contract for page metadata |
| Private journal | Account signup, actor-scoped create/read/delete | No password reset; account recovery needs operator intervention |
| Team board | Three accounts, SQL role checks, admin promotion, outsider refused by signup policy | Owner SQL bootstraps the first admin; denied mutations return zero changes rather than an authorization error |
| Portfolio | Authenticated upload/download, public content rows | A private upload stays with the uploader. Visitors can read a file only when the upload set visibility to public. The inline image copy is still how a private photo reaches the gallery |
| Poll and dashboard | Anonymous votes, SQL choice checks, grouped totals and time buckets | No reliable one-person-one-vote guarantee without identity; reports must fit the supported query grammar |
| Status feed and ingestion | Anonymous JSON-RPC reads/writes, bounded request rate | A third-party webhook POST to an arbitrary path returns 405; consumers must speak MCP |
| Documentation site | Multi-file artifact, explicit HTML paths, SQL search | CLI builds a single HTML entry; unknown routes fall back to home, extensionless paths do not resolve HTML, XML has a generic content type |
| Action probes | Bound inputs, actor isolation and SQL JSON functions | CTE actions remain unsupported; optional filters now need explicit nullable bindings; mutation RETURNING is refused before publication |

The portfolio and documentation examples expose product gaps that a CRUD-only demonstration would miss. Neither should be presented as a fully supported use case yet.

## Changes from this iteration

Failed publications now cancel their completed upload in a `finally` block in the CLI, editor and exported deploy script. Planning and apply failures previously retained transfers until expiry, exhausting an owner's quota after repeated attempts. Cleanup failures preserve the original publishing error and do not turn a successful release into a failure.

The self-hosted archive now includes `kernel-rs/src/limits.rs`. Without it, the exported `main.rs` declared a module absent from the archive and a fresh export could not compile. A regression check follows each module declaration in the exported Kernel so the same omission is caught when another module is added.

External projects now have a CLI export path. Source-only export works offline; `--with-data` explicitly requests the deployed database, accounts and attachments through MCP. The command refuses to overwrite an existing archive and creates it with owner-only filesystem permissions.

```sh
bun run littlebox -- export --dir /tmp/my-app --out /tmp/my-app-selfhost.tar
bun run littlebox -- export --dir /tmp/my-app --out /tmp/my-app-with-data.tar --with-data
```

Live-state exports also carry the app's signup policy. A fresh Kernel imports it with the first deployment, so a public app whose signup was Closed or Allowlist does not start Open. Existing deployments refuse initial-state imports; later releases retain their current policy.

An independent review found two export issues, both corrected in this iteration. Extracted initial-state files now have mode `0600` and generated Git rules exclude them. Exports with live data use the visibility captured with the state snapshot rather than a potentially stale local public manifest or an earlier deployment inspection. Source-only exports still use the local manifest.

Input bindings can opt into `"nullable": true`. An omitted key or explicit JSON null then binds SQL NULL. Existing required inputs and non-empty string checks keep their behavior. A search can use `WHERE (?1 IS NULL OR title LIKE '%' || ?1 || '%')` without a separate action for each filter combination. This contract must agree in the editor preview and hosted Kernel.

Mutation actions still return `{changes, lastInsertId}`. A mutation with SQL `RETURNING` now fails validation instead of publishing successfully and failing when invoked. Use a separate query when the caller needs the resulting row.

## Portability and replacement

Downloading runnable code and replacing individual capabilities are different tests. A self-hosted copy still uses the Littlebox Kernel. It proves independence from the original installation, not independence from every Littlebox interface.

| Capability | Exit available today | Missing interface |
| --- | --- | --- |
| Source and frontend build | Editable HTML/CSS/JS/TS and static artifact in the kit | Full page metadata and multi-page CLI build support |
| Hosting and database state | Export Kernel, SQLite, identity hashes and attachments; start another installation | Stable hostname/custom-domain mapping; continuous state replication and restore verification |
| Frontend framework | Rewrite the frontend while calling the same named actions | Framework choice does not solve origin and session configuration |
| Frontend on another host | Serve through one origin with a reverse proxy | Documented proxy behavior that preserves origin checks; direct foreign-origin browser calls are refused |
| Backend | Replace SDK calls with an app-owned implementation | No typed, generated action contract or conformance suite for another backend |
| App login | Keep the exported app accounts and sign in again | No external identity adapter; an outside JWT or OAuth login does not produce a Kernel Runtime Actor |
| Upload storage | Upload to an external provider and save its URL using an action | Kernel file tools have no storage adapter; private external files require a backend to authorize/sign requests |

No supported exit should require removing Origin headers, copying administrative credentials into browser code, or making shadow accounts with derived passwords. Earlier experiments demonstrated that a proxy can bypass the current origin check; that is a finding about the missing interface, not a recipe to publish.

An archive containing live state includes user password hashes and attachments. Treat it as private data. Active sessions should be replaced by fresh sign-ins on the target installation. Initial imports must run only when a Deployment has no state, so redeploying does not restore an old snapshot over new user data.

The native migration check rebuilt the exported Kernel sources, signed in with the original account/password, read the original row and attachment, refused the old session and retained new data through a rebuild and redeploy. The generated hostname changed, so existing links still need a separate domain migration plan.

Repeat that check without Docker from the repository with Node.js, Bun, Cargo and `tar` available:

```sh
bun run tests/smoke/portable-export.ts --port 19450
```

The source uses the chosen port and the imported Kernel uses the next one. The replay creates temporary state and random credentials, compiles the sources in the generated archive, and removes its data and processes on exit. Cargo uses the existing dependency cache in offline mode, and the generated app build uses the repository's installed esbuild.

## Keep the primitive set small

The current unit is useful: source, artifact, named action, actor, file and release. Keep SQL and authorization on the server, and keep actions deterministic. Avoid adding a general server-code sandbox merely to implement one unsupported integration.

The next additions should remove repeated work across these examples.

1. Make identity usable outside a demonstration. Add account recovery and an external identity adapter with a stable app user ID. Verify identity on the server before producing actor bindings. Do not treat a supplied email or client header as identity.
2. Define file visibility separately from the public shell. A public image needs an explicit publication decision; a private attachment needs authorization. Preserve the current authenticated-file default. Add download/delete to the editor and test access across users and Deployments.
3. Offer a conventional HTTP adapter for selected actions. Declare method, path and input mapping, preserve the action's authorization and rate limits, and add replay protection/signature verification for webhooks. Secret provider requests belong on the server.
4. Make state recoverable. Replicate databases, accounts and files together and restore them into a clean Kernel in a repeatable check. S3 currently holds records and artifacts, so losing the Kernel volume loses application state.
5. Finish the static publishing contract. Provide head metadata, text/binary assets, explicit SPA versus document routing, correct 404s and content types. Keep this independent of the database and login choices.
6. Generate a portable action contract. Inputs, output shapes and errors should let a replacement backend run the same conformance checks. Add client configuration for deployment origin and capability adapters only when a second implementation can pass those checks.

These are proposed changes. This iteration does not open file access, introduce CORS, change signup defaults, add webhook routes or trust external identities.

## Evidence and limits

Run the representative-app replay from the repository with:

```sh
bun run examples/product-fit/smoke.ts
```

The replay starts a separate Kernel with temporary accounts and data, publishes the examples through MCP and removes its state on exit. It exercises both successful flows and known unsupported behavior. A passing result includes documented limitations and does not mean those gaps have been fixed.

The automated suites cover CLI failure cleanup, exported module completeness, exported build/deploy behavior, nullable binding parity and rejection of unsupported mutation results. Native self-host testing must compile the generated sources and use the generated deploy script, rather than a binary copied from the repository.

The representative replay passed all ten scenario groups. The repository test suite passed, the Kernel passed 51 Rust tests, and the local browser suite passed all four scenarios. The production build passed with the existing bundle-size warning. An independent review reproduced the visibility and extracted-state issues above and verified both fixes with export regression tests. A real MCP regression also changed a Deployment from public to private after inspection and verified that its exported snapshot retained private visibility.

Payments, scheduled jobs, outbound email, realtime and secret-bearing integrations have no native primitive in this release. They need an external backend or a future managed integration. This iteration does not verify Docker builds, remote S3 durability, external identity providers or production-scale workloads.
