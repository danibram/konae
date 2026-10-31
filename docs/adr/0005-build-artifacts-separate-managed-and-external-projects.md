# Use Build Artifacts as the seam between Managed and External Projects

Status: accepted, 2026-10-21. Builds on ADR-0004. [ADR-0007](0007-managed-builds-run-in-the-browser.md) decides where the managed builder runs, and the notes below record the data folder, Eject on the server and the static publishing contract.

Konae will keep Managed Projects deliberately smaller than a general Node IDE: they gain TypeScript and browser-compatible libraries while Konae owns dependency resolution, type tooling, validation and bundling. The stable publishing contract is an immutable static Build Artifact, not a particular compiler or project layout. Eject irreversibly transfers source, build and database-schema authority to the Project Owner while the same Project may retain Konae hosting, capabilities, data and deployment history.

## Decision

Managed authoring exposes a constrained capsule rather than an arbitrary repository. Users and agents edit application source, capability declarations and schema intent; they do not manage package-manager commands, lifecycle scripts, bundler configuration or server processes. Konae pins and resolves supported browser dependencies, validates changes and produces a canonical Build Artifact before publication. Node APIs, native addons, arbitrary build plugins, SSR and application server routes are outside the Managed Project contract.

Application code uses the typed `@konae/sdk` interface for actions, auth and files. Every successful build produces a validated manifest of static paths, hashes, sizes, content types and entry behavior. A Deployment references that Build Artifact plus the capability manifest, schema and Server Actions; it never requires a process per app, and `/_konae/*` remains owned by the Kernel.

```text
Managed source  -> Konae build -> Build Artifact -+
                                                       +-> Deployment
External source -> owner build     -> Build Artifact -+
```

Eject exports a conventional TypeScript project with resolved dependencies, `@konae/sdk`, the canonical Drizzle `schema.ts` and SQL migration history defined in ADR-0004, Server Actions and deployment metadata. It changes the existing Project from Managed to External without deleting its Deployments or administered resources. Konae then disables source editing, agent mutations, managed dependency resolution and hosted bundling for that Project; the owner builds locally or in CI and submits a prebuilt artifact through the owner-authenticated Admin Interface.

Schema authority follows source authority. A Managed Project exposes no general database connection that external schema tools can target; schema changes must pass through Konae's database lifecycle and Validation Gate. After Eject, the owner accepts responsibility for migration history, destructive changes, schema drift and compatibility between the database, Server Actions and submitted artifacts. Returning to managed authoring imports the external source as a new Project rather than recreating two sources of truth for the original one.

## Considered options

- **Embed a complete Node/Vite workspace in Managed Projects.** Rejected because terminals, arbitrary configuration, plugins and package scripts enlarge the user and agent interface and turn all managed builds into unrestricted code execution.
- **Make the compiler or local project structure the publishing contract.** Rejected because either may change without changing what a Deployment needs. The validated Build Artifact is the smaller and more stable seam.
- **Make Eject leave Konae completely.** Rejected because source authority, build authority and hosting authority are independent. Owners should be able to outgrow managed authoring without abandoning Kernel capabilities.

## Consequences

Konae must provide dependency resolution, TypeScript diagnostics, a canonical managed builder, Build Artifact storage and serving, a typed SDK, export/eject, external artifact submission and visible Project state. The Managed environment intentionally supports fewer packages and frameworks than a general repository; that constraint buys simpler agent behavior, faster feedback and a smaller security surface. External Projects gain freedom at the cost of losing Konae's source and schema guarantees.

## Note, 2026-11-06

The Kernel stores a Deployment's Build Artifact in `deployments/<id>/artifact/`, in the same folder as its record, databases, files and backups, instead of a separate `artifacts/<id>/` tree. That folder is the unit a scheduled copy carries and `konae-kernel restore` installs. When S3 is configured the Kernel still uploads the artifact to the artifact bucket, but serves from the volume and reads S3 only when the local file is missing. A Kernel with the older layout moves it into place on start.

## Note, 2026-11-06: Eject on the server

Eject runs on the Kernel in two steps, `eject_plan` and `eject_apply`, behind a new `manage_lifecycle` permission that the owner's browser session holds and a token holds only when delegated. The plan pins the saved-sources revision, the counter `project_sources_save` advances, and returns the code package as a download: the saved sources, a `konae.json` for an External project and a README with the CLI commands. The client checks it before it applies. Apply takes the project lock a save takes, checks the revision again, sets the source authority to External and records who ejected it, when, and at which revision. A save from an old tab therefore lands before Eject, which makes the plan stale, or is refused after it with one message.

Eject does not export a conventional project with resolved dependencies and migration history, as the decision above describes. Managed projects have no package manifest or migration files to resolve, so the package holds what the editor saved: the source tree, `schema.sql`, `actions.json` and the Drizzle `schema.ts`. The Konae CLI builds that tree and publishes it. After Eject the project's last saved sources stay readable, frozen, so the editor can show them read-only; the plan forbids Managed source changes, not reads.

A project created External never passes through Eject, and `project_create` asking for Managed on an External project is refused, so source authority only moves one way. The Deployment, its accounts, sessions, invitations, mail and URL are not touched.

## Note, 2026-11-06: the static publishing contract

The Build Artifact was one `index.html` from the CLI and the editor, and the Kernel served it for every unknown path. A two-page app lost its second page without a word, a mistyped link got the home page with 200, and `404.html` was never read (docs/research/showcase-findings.md, rows on `konae build` and unknown paths).

The contract is now a file both sides of the seam can read. `src/lib/static-artifact.js` decides which Capsule files become the artifact: every `.html` at the root and under `pages/` (served without the prefix), everything under `public/` as it is, and every other file a page or its stylesheets load. A whole-document page keeps its head and loads the SDK from `/runtime.js`; a fragment page runs in the Capsule document as before. A reference to a file the Capsule does not hold refuses the build with the path. The CLI, the editor and the exported kit's `scripts/build.mjs` run that one file, so the kit rebuilds a multi-page app the way it was published. It is plain JavaScript with a type declaration for this reason.

The artifact gained `encodings`, which names the files that travel as base64, so images and fonts publish from the CLI; version stays 1 and an artifact without it is all text, as before. Migration sends a live binary file the same way instead of refusing it. The manifest gained `routing`: `"pages"` by default, where an unknown path answers 404 with `404.html`, and `"spa"`, which keeps the old fallback to `index.html`. The Kernel refuses another routing value at `release_plan` with the builders' words. Making `pages` the default changes what an existing Deployment answers on its next request for an unknown path; an app that routes in the browser sets `"spa"`.

## Note, 2026-11-07: a reproducible build, and a fragment's title and icon

The starter app found two gaps in the static publishing contract (examples/showcase/starter/README.md). esbuild writes a comment naming each module above its code, relative to the folder the build ran in, so the published `app.js` began with `// ../../../../private/var/folders/…/app.js`, and one Capsule built from two folders gave two artifacts. `static-artifact.js` now drops each such line that names a Capsule file, with the editor builder's `konae:` prefix, and the CLI and the kit bundle from the Capsule root. One Capsule gives byte-identical files from any folder and from each door.

A fragment page had no head, so a `<title>` or `<link rel="icon">` in it landed in the body and Chrome logged a 404 for `/favicon.ico`. The `<title>` and icon links a fragment opens with now move into the head of the Capsule document, in the build and in the preview, since both render it with `renderCapsuleDocument`. The managed path holds text only, so the icon that works from every door is SVG, as a file or a `data:` URL, which the Content Security Policy already allows. Carrying binary `public/` files through the managed path as base64 was the other option; it would change the saved-sources format and the Kernel's checks on it for one small file, so it was not taken.
