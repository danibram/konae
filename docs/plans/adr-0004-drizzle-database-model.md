# ADR-0004 implementation plan: managed Drizzle model

## Intended outcome

Each Managed Project will have five editable sources, replacing `schema.sql` with a restricted Drizzle `schema.ts`. The user and the agent will edit that single Database Model. Konae will generate a reviewable SQL migration, test it against a copy of the data, create a backup, apply it and record the new revision. The browser runtime will keep accessing data only through Server Actions.

The change must be incremental. The `schema.sql` path will not be removed until existing projects can be converted and verified without losing structure or data.

## Starting point

- `ProjectFile` currently fixes `schema.sql` as one of five files in `src/lib/types.ts`.
- The editor, the agent and its prompt know those five names literally.
- `ValidationGate` runs `schema.sql` twice with `sql.js`, applies it to a copy of the local database and only detects divergences. It does not generate a migration.
- The preview runs the schema again when it opens its database.
- The Kernel stores the Capsule inside each Deployment, creates one SQLite per Deployment and runs `schema.sql` during `deploy()`.
- SQLite export exists, but automatic backups, retention and restore are not implemented yet.

## Prior decision required

Before enabling migrations on hosted data, a separate ADR must decide who owns the persistent database:

- The recommended option is that a Project Environment owns the database and file storage. Immutable Deployments reference that Environment and an expected schema revision.
- In the current model, each Deployment owns its own SQLite and blobs. In this model, publishing creates another database, so there is no clear continuity to migrate from.

ADR-0004 can progress through generation, preview, Development Kernel and project conversion without this decision. Hosted application must stay disabled until it is settled.

## Main seam

Create a deep `database-lifecycle` module with two operations for its callers:

```ts
interface DatabaseLifecycle {
  planChange(input: PlanDatabaseChange): Promise<MigrationPlan>;
  applyChange(input: ApplyDatabaseChange): Promise<AppliedSchemaRevision>;
}
```

`planChange` covers model compilation, SQL generation, migration validation and risk classification. `applyChange` checks that the plan and the source revision are still current, obtains the required approval, creates the recovery point, applies and verifies. Preview, publishing, Admin Interface and Development Kernel must not coordinate Drizzle, backups and transactions separately.

Minimum seam types:

- `DatabaseModelSource` holds the `schema.ts` content, the pinned dialect and the toolchain version.
- `SchemaRevision` holds the model hash, the Drizzle snapshot, the migrations head and the acceptance date.
- `MigrationPlan` holds an immutable identity, source/target revision, generated SQL, diagnostics, risks and affected actions.
- `AppliedSchemaRevision` holds the accepted revision, the applied migration and the associated recovery point.
- `Approval` is an explicit decision for a rename, data loss or another ambiguous operation. It is never an implicit boolean.

Internal adapters:

- `DrizzleSchemaCompiler` runs `generate`, `check` and `pull --init` for conversions and produces deterministic artifacts.
- `DatabasePersistence` clones, creates recovery points, applies SQL and restores. It will have at least `sql.js` and Bun/SQLite adapters. A future D1 would be another implementation of the same seam.
- `MigrationRepository` keeps plans, SQL, snapshots, hashes, revisions and application state.

Drizzle must stay inside this module. Calls to Drizzle Kit will not be spread across the editor, HTTP routes or the Deployment Store.

## Phases

### 0. Technical spike and supported contract

1. Pin exact compatible versions of `drizzle-orm` and `drizzle-kit`. Do not use ranges for persistent artifacts.
2. Test the full SQLite cycle in Bun: `schema.ts` → `generate` → SQL/snapshot → `check` → apply with `drizzle-orm/bun-sqlite`.
3. Test `pull --init` on a SQLite created from the current `schema.sql` and compare tables, columns, indexes, constraints and foreign keys.
4. Check how Drizzle reports interactive renames and design a non-interactive `requires-approval` output. Konae must never accept an ambiguous rename automatically.
5. Define the subset allowed in Managed: imports only from the pinned SQLite module, exports of tables and relations, and no drivers, environment variables, network, filesystem or side effects.
6. Measure Drizzle Kit cold and warm startup and choose between an ephemeral process and a worker pool.

This phase outputs an automated round-trip test and an artifact fixture. If the CLI does not provide a stable seam, wrap it as a replaceable command. Do not import undocumented internal modules across the application.

### 1. Version the Capsule and add `schema.ts`

1. Add an explicit version to the Project source format, separate from `CapsuleManifest.version`.
2. Define Capsule v2 with `schema.ts` instead of `schema.sql`. Keep a v1 reader during the transition.
3. Add a Drizzle starter equivalent to the current `tasks` table.
4. Store in the Project the accepted schema revision and the exact toolchain version that produced its artifacts.
5. Make normalization, import/export, reset and persistence keep v1 and v2 correctly.

Done when a new project saves, reloads and exports with `schema.ts`. A v1 project still opens without destructive changes.

### 2. Implement `database-lifecycle` in isolation

1. Implement `planChange` with an ephemeral workspace, configuration generated by Konae and a cache keyed by `(toolchain, previousSnapshotHash, modelHash)`.
2. Reject imports or code outside the contract before evaluating `schema.ts`.
3. Run the evaluation in an isolated process, with no network or credentials, with a timeout, CPU/memory/output limits and its own temporary directory.
4. Normalize the Drizzle result into a `MigrationPlan`. Do not leak temporary paths or internal messages to the editor.
5. Apply the plan to a clone, compare the final schema with the target model and prepare every Server Action.
6. Implement `applyChange` with per-database concurrency control, compare-and-swap on the source revision and an idempotency key.

Done when creating a table, adding a nullable column, adding an index and changing a constraint produce deterministic plans. Applying the same plan twice does not duplicate a migration, and two concurrent plans cannot both advance from the same revision.

### 3. Integrate editor, preview and agent

1. Add TypeScript/Drizzle support to CodeMirror, including pinned types, autocomplete and local diagnostics in a worker.
2. Show `schema.ts` as the default model in Code and Schema Studio.
3. Replace every agent list, instruction and heuristic that mentions `schema.sql`. `schema.ts` and `actions.json` remain the risk files.
4. Keep a single agent tool, `validate_capsule`. Internally it will call `planChange` and return diagnostics and a migration summary, with no CLI details.
5. Apply the SQL of a valid plan only to a clone of the preview SQLite. The last valid preview stays active if the new model does not compile or does not converge.
6. Add a read-only view of the generated SQL with risks, impact and source/target revision.

Done when an agent adds a column in `schema.ts`, gets typed feedback, validates, sees the generated SQL and updates the preview without editing migrations or using a shell.

### 4. Backups and application in the Development Kernel

1. Implement `DatabasePersistence.createRecoveryPoint()` correctly for WAL. Do not copy an open `.sqlite` while ignoring `-wal` and `-shm`.
2. Persist the backup, metadata, checksum, source schema and target migration before the first real change.
3. Apply the migration in a transaction when SQLite allows it, verify the result and record the revision only at the end.
4. Inject failures after the backup, mid-migration, before the journal and after the journal to test recovery and idempotency.
5. Expose list/restore through the Admin Interface, never the Runtime Interface.
6. Add size limits, retention and auditing.

Done when a valid migration keeps the data. A failed migration does not announce the new revision. The recovery point can be restored and the previous Server Actions can run again.

### 5. Convert existing `schema.sql` projects

1. Run the historical `schema.sql` in a temporary SQLite and use introspection to create `schema.ts`, a snapshot and a baseline migration.
2. Compare both schemas through normalized SQLite metadata, not through textual DDL equality.
3. Keep the original SQL as a historical artifact. Seed `INSERT` statements stay in a custom migration and are not mixed up with the Database Model.
4. Classify triggers, views and objects that cannot be represented. Keep them as a custom migration only if it can be shown that future migrations will not drop them. Otherwise block the conversion with an actionable report.
5. Prepare every Server Action against the converted database and compare data and counts before marking the baseline as accepted.
6. Keep rollback to Capsule v1 until the first v2 change applies correctly.

Done when the starter and the representative fixtures convert with no structural differences and no row loss. An unsupported case fails closed and keeps the v1 project.

### 6. Integrate hosted publishing

This phase starts after the ADR on data ownership.

1. Separate Environment persistence from the immutable Deployment files if that is the adopted decision.
2. Make the Deployment reference `schemaRevisionId` and reject publishing when actions or the artifact expect another revision.
3. Replace `db.exec(hosted.files["schema.sql"])` with `databaseLifecycle.applyChange(...)` in the administrative plane.
4. Serialize publishing and migration per Environment. The runtime can keep serving the previous revision while the plan is validated, but it must not mix new actions with the old schema.
5. Define the compatibility strategy: expand/contract migrations for incompatible changes, or an explicit maintenance window for the first simple apps.
6. Record the actor, plan, approval, backup, migration, result and associated Deployment.

Done when publishing a compatible revision migrates once and switches the Deployment only after verifying it. A failure leaves the previous code, schema and actions working.

### 7. Eject and external operation

1. Export `schema.ts`, the full `drizzle/` directory, the pinned version, `drizzle.config.ts`, the `db:generate`, `db:migrate` and `db:check` scripts and the Development Kernel configuration.
2. Do not include credentials or a persistent administrative URL.
3. Add an authenticated command to request a short-lived administrative tunnel when the owner wants to operate the hosted database.
4. Detect and show drift before accepting a new Build Artifact or external migration.
5. Document that `push` is an explicit decision of the External Project Owner and that it breaks the guarantees of the exported history.

Done when the ejected project installs dependencies, creates a local database from scratch, runs its migrations and works with the Development Kernel with no frontend changes.

## Security

- Treat `schema.ts` as untrusted code even though its interface is small.
- Compile and evaluate outside the Kernel process that holds secrets and sessions.
- Block dynamic imports, top-level await, access to dangerous globals and any module that is not allowlisted.
- Do not accept a `drizzle.config.ts` supplied by a Managed Project. Konae generates the configuration.
- Do not send a hosted database to the browser for validation. The Kernel validates against a server-side snapshot.
- Sign or hash the `MigrationPlan`. `applyChange` must refuse if the model, actions, source revision or SQL changed.
- Require a specific approval for each risk, not a reusable "accept all".

## Performance

- Warm syntax and type diagnostics in the editor target a p95 under 150 ms.
- Debounce canonical generation. Do not run Drizzle Kit on every keystroke.
- Content-addressed cache of model and snapshot, and a warm pool if the spike shows a clear gain.
- Preview keeps the last valid plan and applies the SQL to a clone. It never blocks editing while waiting for a deploy.
- For a small schema, the initial target is a warm canonical plan with a p95 under 1 s, measured before it becomes an SLO.
- `drizzle-orm` and `drizzle-kit` must not enter the mini-app runtime bundle. Only the types and the editor support can load lazily.

## Minimum test matrix

- Model tests cover a table, an autoincrement PK, an FK, unique, an index, a default and a relation.
- Evolution tests cover add nullable, add required with backfill, ambiguous rename, destructive drop and SQLite table recreation.
- Data tests cover row preservation, failed constraints, seeds and SQLite blobs.
- Action tests cover a valid action, a dropped column, an incompatible binding and rollback to previous actions.
- Concurrency tests cover two plans from the same revision, a retry after a timeout and a duplicate apply.
- Recovery tests cover a failure before and after the backup, a partial failure, a missing journal and a verified restore.
- Security tests cover a forbidden import, an infinite loop, excessive output, reading the environment, filesystem and network.
- Compatibility tests cover a clean v1 conversion, v1 with seeds and v1 with an object that cannot be represented.
- E2E tests cover a user and an agent editing `schema.ts`, approving a migration, keeping data, publishing and ejecting.

## Recommended delivery order

1. Drizzle spike and supported contract.
2. Capsule v2 and a starter with `schema.ts`.
3. Deterministic `planChange` and action validation.
4. Editor, agent and preview on generated migrations.
5. `applyChange`, backups and restore in the Development Kernel.
6. Safe conversion of v1 projects.
7. ADR for Project Environment and shared persistence.
8. Hosted application and coordinated publishing.
9. Full Drizzle eject and temporary administrative tunnel.

Each delivery must leave a working end-to-end path. A `schema.ts` editor and a Kernel that still only understands `schema.sql` will not ship together without a compatibility adapter.

## Definition of done

- `schema.ts` is the only editable schema source in every new Managed Project.
- The agent can change it and validate it without a terminal or knowledge of Drizzle Kit.
- Every schema mutation has a SQL plan, a source/target revision, an approval when needed, a backup and an audit record.
- Preview, Development Kernel and hosting exercise the same lifecycle interface with different adapters.
- The Kernel no longer reruns a declarative schema when it opens a database.
- v1 projects are converted in a verified way or stay intact with an actionable diagnostic.
- Eject produces a conventional Drizzle project that can recreate the local database and continue its history.

## Technical references

- [Drizzle Kit generate](https://orm.drizzle.team/docs/drizzle-kit-generate) generates SQL and snapshots by comparing the current model with the previous history.
- [Drizzle Kit check](https://orm.drizzle.team/docs/drizzle-kit-check) checks the consistency of the generated history.
- [Drizzle Kit pull](https://orm.drizzle.team/docs/drizzle-kit-pull) introspects an existing database, and `--init` sets a baseline.
- [Drizzle with Bun SQLite](https://orm.drizzle.team/docs/get-started/bun-sqlite-existing) was the supported adapter for the former Kernel based on `bun:sqlite`, since replaced by the Rust Kernel.
