# Use Build Artifacts as the seam between Managed and External Projects

Status: accepted

Littlebox will keep Managed Projects deliberately smaller than a general Node IDE: they gain TypeScript and browser-compatible libraries while Littlebox owns dependency resolution, type tooling, validation and bundling. The stable publishing contract is an immutable static Build Artifact, not a particular compiler or project layout. Eject irreversibly transfers source, build and database-schema authority to the Project Owner while the same Project may retain Littlebox hosting, capabilities, data and deployment history.

## Decision

Managed authoring exposes a constrained capsule rather than an arbitrary repository. Users and agents edit application source, capability declarations and schema intent; they do not manage package-manager commands, lifecycle scripts, bundler configuration or server processes. Littlebox pins and resolves supported browser dependencies, validates changes and produces a canonical Build Artifact before publication. Node APIs, native addons, arbitrary build plugins, SSR and application server routes are outside the Managed Project contract.

Application code uses the typed `@littlebox/sdk` interface for actions, auth and files. Every successful build produces a validated manifest of static paths, hashes, sizes, content types and entry behavior. A Deployment references that Build Artifact plus the capability manifest, schema and Server Actions; it never requires a process per app, and `/_littlebox/*` remains owned by the Kernel.

```text
Managed source  -> Littlebox build -> Build Artifact -+
                                                       +-> Deployment
External source -> owner build     -> Build Artifact -+
```

Eject exports a conventional TypeScript project with resolved dependencies, `@littlebox/sdk`, the canonical Drizzle `schema.ts` and SQL migration history defined in ADR-0004, Server Actions and deployment metadata. It changes the existing Project from Managed to External without deleting its Deployments or administered resources. Littlebox then disables source editing, agent mutations, managed dependency resolution and hosted bundling for that Project; the owner builds locally or in CI and submits a prebuilt artifact through the owner-authenticated Admin Interface.

Schema authority follows source authority. A Managed Project exposes no general database connection that external schema tools can target; schema changes must pass through Littlebox's database lifecycle and Validation Gate. After Eject, the owner accepts responsibility for migration history, destructive changes, schema drift and compatibility between the database, Server Actions and submitted artifacts. Returning to managed authoring imports the external source as a new Project rather than recreating two sources of truth for the original one.

## Considered options

- **Embed a complete Node/Vite workspace in Managed Projects.** Rejected because terminals, arbitrary configuration, plugins and package scripts enlarge the user and agent interface and turn all managed builds into unrestricted code execution.
- **Make the compiler or local project structure the publishing contract.** Rejected because either may change without changing what a Deployment needs. The validated Build Artifact is the smaller and more stable seam.
- **Make Eject leave Littlebox completely.** Rejected because source authority, build authority and hosting authority are independent. Owners should be able to outgrow managed authoring without abandoning Kernel capabilities.

## Consequences

Littlebox must provide dependency resolution, TypeScript diagnostics, a canonical managed builder, Build Artifact storage and serving, a typed SDK, export/eject, external artifact submission and visible Project state. The Managed environment intentionally supports fewer packages and frameworks than a general repository; that constraint buys simpler agent behavior, faster feedback and a smaller security surface. External Projects gain freedom at the cost of losing Littlebox's source and schema guarantees.

## Note, 2026-11-06

The Kernel stores a Deployment's Build Artifact in `deployments/<id>/artifact/`, in the same folder as its record, databases, files and backups, instead of a separate `artifacts/<id>/` tree. That folder is the unit a scheduled copy carries and `littlebox-kernel restore` installs. When S3 is configured the Kernel still uploads the artifact to the artifact bucket, but serves from the volume and reads S3 only when the local file is missing. A Kernel with the older layout moves it into place on start.

## Note, 2026-11-06: Eject on the server

Eject runs on the Kernel in two steps, `eject_plan` and `eject_apply`, behind a new `manage_lifecycle` permission that the owner's browser session holds and a token holds only when delegated. The plan pins the saved-sources revision, the counter `project_sources_save` advances, and returns the code package as a download: the saved sources, a `littlebox.json` for an External project and a README with the CLI commands. The client checks it before it applies. Apply takes the project lock a save takes, checks the revision again, sets the source authority to External and records who ejected it, when, and at which revision. A save from an old tab therefore lands before Eject, which makes the plan stale, or is refused after it with one message.

Eject does not export a conventional project with resolved dependencies and migration history, as the decision above describes. Managed projects have no package manifest or migration files to resolve, so the package holds what the editor saved: the source tree, `schema.sql`, `actions.json` and the Drizzle `schema.ts`. The Littlebox CLI builds that tree and publishes it. After Eject the project's last saved sources stay readable, frozen, so the editor can show them read-only; the plan forbids Managed source changes, not reads.

A project created External never passes through Eject, and `project_create` asking for Managed on an External project is refused, so source authority only moves one way. The Deployment, its accounts, sessions, invitations, mail and URL are not touched.
