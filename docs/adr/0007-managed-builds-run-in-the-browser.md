# Run Managed application builds in the browser

Status: accepted, 2026-10-21. Amends [ADR-0005](0005-build-artifacts-separate-managed-and-external-projects.md), which left the builder's location open.

Littlebox will produce the Build Artifact of a Managed Project in the owner's browser, not in the Kernel. ADR-0005 made the artifact the publishing contract and deliberately left the builder's location open; this fixes it. Building where the source already is keeps a new Deployment free of server compute, and keeps the process that holds every Deployment's database and identity store from executing project code or third-party packages. The Kernel's role narrows to what it can verify: it accepts, validates and serves bytes it did not produce.

The decision is scoped to the **application** build. Deriving migrations from the Database Model of ADR-0004 is a separate pipeline with a separate answer, stated below, because the risk it carries is not the same risk.

## Decision

The managed application builder runs in the browser, isolated from the editor: bundling, TypeScript, JSX and CSS are processed by a WebAssembly toolchain in a Worker, and the result is a Build Artifact with the manifest ADR-0005 defines. Deployment submits that artifact through the owner-authenticated Admin Interface alongside the capability manifest, Database Model and Server Actions. The Kernel performs no dependency resolution, no bundling and no lifecycle scripts for Managed Projects.

**Bundling is not execution, and the difference is the security boundary.** The builder parses and links source without running it, so a dependency cannot act during a build. A build step that needs the editor's DOM, its credentials or its network identity is not a build step Littlebox will run.

Dependencies stay resolved by Littlebox, as ADR-0005 requires. Authors and agents declare intent; Littlebox resolves it against pinned, browser-compatible versions and **generates** the dependency manifest and lock into the Capsule. Those files are reviewable and exportable, and they are not a second editable source of truth. ADR-0004 sets the same relationship between `schema.ts` and generated SQL. A hand-authored manifest with unrestricted dependencies is what an External Project is for.

The Validation Gate gains a build stage: a Capsule that does not compile cannot be applied or published, alongside the existing schema, migration and Server Action checks.

**The Kernel treats a submitted artifact as untrusted input.** It verifies every declared hash against the bytes received, enforces size and file-count limits, admits only an allowlist of content types, rejects paths that escape the artifact root or collide with `/_littlebox/*`, and serves the result under a Content-Security-Policy. Owner authentication says who may publish; it never says the bytes are well-formed.

### Schema migrations are a different pipeline

Deriving a migration needs two things that must not be confused: **evaluating** the owner's `schema.ts` to obtain its table declarations, and **diffing** two schema snapshots into SQL. Only the first is project code.

Neither happens in the Kernel process. Both run in a short-lived subprocess with an empty environment, closed stdin, a 20-second timeout and a 4 MB output cap. It runs as the Kernel's user without a network or filesystem sandbox, so the model guard and that process boundary are the containment. The browser alternative for the same work is rejected below. One disposable subprocess per plan request is not the per-app container ADR-0001 rejects; it is the same class of cost as any other request.

The diff moved out of the Kernel for a reason that has nothing to do with trust, and it is worth recording because the first version of this decision said the opposite. Drizzle's differ cannot tell a rename from a drop-and-create, so it resolves the ambiguity by asking a human, and it decides whether to ask by looking for a terminal. Renaming a column is an ordinary edit. A Kernel started from a terminal therefore renders an interactive prompt into an HTTP request nobody can answer, and the process, which holds every Deployment's database, never replies; this was reproduced under a pseudo-terminal, not inferred. In the subprocess, stdin is a pipe the parent has already closed, so the question cannot be posed at all.

That containment is a backstop, not the answer. Littlebox decides the policy itself, before the differ is reached: a change that removes a table, column or view while adding another in the same step is refused, because the two readings differ in whether the owner's rows survive and Littlebox does not guess about the owner's data. The owner is told to apply the addition and the removal as separate changes.

What stays in the Kernel is judgement over data it received: the plan's identity, and whether the returned statements are destructive, computed from the SQL rather than trusted from the subprocess, since that process ran the owner's code. **The subprocess turns source into SQL; the Kernel decides what that SQL means.**

## Considered options

- **Build in the Kernel.** Rejected. Package lifecycle scripts, bundler plugins and module evaluation are arbitrary code execution inside the multi-tenant process that owns every Deployment's SQLite file, and per-deploy build CPU contradicts ADR-0001's rule that a new mini-app creates no process. Containing it needs an ephemeral sandbox per build, which is the per-app cost the product exists to avoid.
- **Build in CI on the owner's infrastructure.** Rejected for Managed Projects: it makes publishing depend on tooling Littlebox does not control and pushes a self-hosted install toward a pipeline. It is exactly what External Projects already do under ADR-0005, so the option exists without being the managed default.
- **Keep serving unbundled sources.** Rejected: it caps a Capsule at what a browser can run verbatim, with no TypeScript, no imports between files and no libraries, which is the ceiling this decision exists to lift.
- **Run the builder in the editor's own context** rather than an isolated worker. Rejected: the editor holds provider API keys, and a build is the one moment a Capsule's dependencies get near them.
- **Move migration diffing into the browser too.** Rejected on evidence, not preference. Drizzle's migration API ships as the whole CLI in one file, roughly 3 MB including database drivers for engines Littlebox does not use, and does not declare browser support. Bundling it for a browser target fails on dozens of unresolvable Node builtins (`child_process`, `net`, `tls`, `http2`, `fs`); stubbing every builtin and driver does produce a bundle of about 1.5 MB minified, but the code path Littlebox actually needs then calls `require()` at runtime and hashes snapshots with a synchronous digest the platform does not offer. Reaching a working port means maintaining an impersonation of Node against a tool that publishes no browser contract, where any upgrade can reintroduce a dependency silently. An isolated subprocess achieves the same goal without that liability. The owner's code leaves the Kernel, and so does a differ that wants to ask questions.

## Consequences

Littlebox owns a browser toolchain: a pinned dependency registry, a WebAssembly bundler, TypeScript diagnostics, and artifact assembly with hashing. Build time and memory become the owner's tab rather than the server, which bounds how large a Managed Project can reasonably get. ADR-0005 already scopes Managed Projects below a general repository, so the limit is acceptable, and it is a reason to keep Eject available.

Because the artifact is produced client-side, its integrity is a Kernel responsibility rather than an assumption, and the verification above is not optional hardening: it is the price of moving the build. Offline authoring keeps working, since resolution happens against pinned versions the editor can cache.

Two pipelines with two locations is more surface than one, and the boundary has to stay legible: the browser produces artifacts, an isolated subprocess turns declarations into SQL, and the Kernel verifies bytes and classifies results. The rule that keeps it honest is that the Kernel process never evaluates a Capsule's source. The migration pipeline satisfies it today; the browser build toolchain is still to be written, so the same rule has to survive that work.

Refusing ambiguous changes is a real limitation rather than a temporary gap: renaming a column is something owners will want, and today it must be done as an addition and a removal, which does not carry the rows over. Expressing an intended rename, so that the migration can preserve data, needs the Database Model to say so, and that is a decision for another ADR.

A Capsule now carries generated dependency files, so export, import and the agent's file surface grow accordingly; the agent edits declarations of intent, never the generated lock.

## Note, 2026-11-07: what is built

The application build exists. `src/runtime/browser-build.ts` bundles the Capsule's entrypoint and its relative imports with `esbuild-wasm`, which runs in its own worker, and `src/lib/static-artifact.js` assembles the Build Artifact the Kernel verifies. The migration pipeline runs in the subprocess described above. Declared Dependencies are not built: no Capsule carries a generated manifest or lock, and the builder resolves only files the Capsule holds. The sentences above about the dependency registry and the generated dependency files describe that part of the decision, not the current system.

## Note, 2026-11-09: packages without a registry

A Capsule may import npm packages. Its `package.json` lists them under `dependencies` with exact versions, and that file is the one declaration: the agent and the owner edit it, and nothing is generated from it. The registry and the generated lock above are dropped. The build already runs on the owner's machine, so Littlebox has no reason to choose which libraries an app may use.

- The editor's build resolves a bare import against that list and fetches it from esm.sh at that version, with every other listed package pinned through esm.sh's `deps` parameter so two packages share one copy. It reads only that origin. esbuild links the text into the app's bundle; the editor never evaluates it, so a package gets no nearer the provider keys than a relative import does. The fetch is a build-time dependency on esm.sh, which sees the list of packages a Capsule names.
- `littlebox build` and the exported kit resolve the same imports from `node_modules` after `npm install`, as esbuild does by default. The kit's `start.sh` installs `app/package.json` and its deploy skips `app/node_modules`.
- The published app loads nothing from esm.sh. Its Content Security Policy is unchanged.
- The two sources give the same package versions, not byte-identical bundles: esm.sh compiles a package for `es2022` and npm ships what the author published.
