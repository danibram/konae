# Use a Drizzle schema as the Managed Project database model

Status: accepted, 2026-10-21. Amends [ADR-0003](0003-in-browser-agent-on-pi-agent-core.md): the agent edits `schema.ts`, not `schema.sql`.

Every Managed Project will expose a constrained Drizzle `schema.ts` as its canonical, editable Database Model. Users and agents iterate on that model in the Konae editor; Konae derives SQL migrations, validation artifacts and database backups through one managed database-lifecycle module. Generated SQL remains reviewable and portable, but it is not a second editable source of truth.

## Decision

The Managed Capsule evolves from an editable `schema.sql` to an editable `schema.ts` containing Drizzle table, column, relation, constraint and index declarations. Konae pins the supported Drizzle version and imports, provides autocomplete and diagnostics, and prevents arbitrary modules, drivers, credentials or executable package configuration from entering the schema file. The same Database Model is supplied to the agent, the editor's schema view, the Validation Gate and migration generation so they cannot disagree about the intended structure.

`schema.ts` is an authoring and build input; it is never included in frontend output and does not give browser code a database connection. `actions.json` remains the Managed Capsule's set of named runtime SQL operations, checked against the schema-derived database. Browser code continues to invoke those actions through `@konae/sdk`, and the Kernel continues to authorize them as required by ADR-0001.

The database-lifecycle module hides Drizzle Kit, migration bookkeeping and persistence adapters behind a small Kernel interface. A successful managed schema change performs these steps as one coordinated operation:

1. Typecheck and evaluate the constrained `schema.ts`, then materialize its target SQLite structure in a disposable database.
2. Generate a candidate SQL migration from the previously accepted Drizzle snapshot to the new Database Model, preserving its SQL, hashes and migration metadata.
3. Apply the candidate to a disposable copy of the current database and verify convergence, constraints and affected Server Actions.
4. Present the migration and any destructive or ambiguous operation for the required Project Owner approval.
5. Ask the persistence adapter for a consistent backup immediately before changing the real database.
6. Apply the ordered SQL migration through Drizzle's migration machinery and record the new model revision only after it succeeds.
7. Verify the resulting schema and retain the pre-migration backup as its recovery point.

Physical snapshot, retention and restore remain the responsibility of the database persistence adapter, such as SQLite files or D1, not of Drizzle. Keeping them inside the same lifecycle operation prevents publication, administration and the Development Kernel from implementing different safety sequences.

Managed users and agents may inspect generated migrations and SQL schema snapshots but cannot edit them independently, run `drizzle-kit` or provide a database connection. Existing `schema.sql` Projects receive a one-time, verified Drizzle baseline derived from their materialized SQLite schema; their original SQL is retained in history but stops being the active Database Model. This decision supersedes only ADR-0003's assumption that the agent edits `schema.sql`; its scratch workspace, agent loop and Validation Gate remain in force.

Eject exports the canonical `schema.ts`, ordered SQL migrations, Drizzle snapshots and a conventional `drizzle.config.ts`, but no hosted credential. It may also include a generated `schema.sql` snapshot for inspection and portability. From that point the External Project Owner may use `drizzle-kit generate` and `drizzle-kit migrate`, or deliberately use `drizzle-kit push`, against databases they administer. Access to a hosted Konae database still requires an explicit, short-lived Admin Interface connection. Using `push` against hosted data bypasses the exported migration history, so resulting drift, rollback and compatibility become the External Project Owner's responsibility.

## Considered options

- **Keep `schema.sql` as the editable Managed Database Model and generate Drizzle metadata internally.** Rejected because users, agents, editor tooling and migration generation would reason about different representations, requiring a permanent SQL-to-Drizzle synchronization layer.
- **Expose both `schema.ts` and `schema.sql` as editable files.** Rejected because two writable representations create ambiguous schema authority and inevitable drift.
- **Hide the Drizzle model from Managed users.** Rejected because the model is the most useful surface for typed iteration, agent edits, autocomplete and eventual Eject portability.
- **Use `drizzle-kit push` as the managed deployment mechanism.** Rejected because applying a live diff without retaining the reviewed SQL migration breaks reproducibility and weakens recovery.
- **Expose Drizzle ORM to frontend code.** Rejected because that would collapse the runtime/admin separation established by ADR-0001 and move unrestricted data capability into an inspectable client.

## Consequences

Konae must add `schema.ts` to the Managed Capsule, constrain its imports and execution, pin Drizzle, provide editor types, retain accepted snapshots and migrations, and migrate existing `schema.sql` Projects to verified baselines. The database-lifecycle interface becomes the single seam used by preview validation, publication, administration and local development. This ADR deliberately does not decide whether persistent data belongs to a Project Environment or to each immutable Deployment; that lifetime decision still needs its own ADR before hosted migration execution is implemented.

Implementation is staged in [`docs/plans/adr-0004-drizzle-database-model.md`](../plans/adr-0004-drizzle-database-model.md).

## Note, 2026-11-08: a rebuilt table keeps the CHECKs schema.sql added

A Database Model cannot state a CHECK (the model guard refuses the `sql` tag Drizzle's `check()` needs), but schema.sql can add one, and since the second smooth-authoring wave the release check leaves CHECK groups out of the model comparison. Drizzle rebuilds a table from the model alone (`CREATE TABLE __new_x`, copy, drop, rename) when a column's type, nullability or default changes, so that rebuild dropped every CHECK of the table without a word. On the Kernel the same rebuild never ran, because its SQLite authorizer refused Drizzle's `PRAGMA foreign_keys` lines with "not authorized".

The migration now carries each CHECK over from the live table's SQL, as written, and appends it to Drizzle's `CREATE TABLE __new_x`. A column's own CHECK leaves with the column, as SQLite's `ALTER TABLE DROP COLUMN` does. Any other CHECK that names a column the change removes, in a rebuild or in `ALTER TABLE DROP COLUMN`, refuses the migration before the backup with "Table habits has a CHECK that names column note, which this migration removes. Rebuild habits without that CHECK in the SQL console, or keep note, then apply the migration again." Konae does not rewrite the owner's rule to fit the new columns. The migration leaves out Drizzle's `PRAGMA foreign_keys` lines, which SQLite ignores inside the migration's transaction.

The plan stays a function of the two models. `schema_plan` returns Drizzle's statements, and the plan id covers only those. The carry runs where a database is at hand, in `apply_migration` in kernel-rs/src/main.rs through kernel-rs/src/checks.rs, and in the editor preview's `applyDatabaseMigration` and the Database Studio review through src/lib/carry-checks.ts. kernel-rs/src/check-carry-cases.json holds the cases both suites run. Step 4 above shows the owner the statements as they will run, with the carried CHECKs, and a refusal shows in place of the plan and its Apply button.

A package and a migration to another machine move the database file, so its CHECKs travel unchanged; `tests/check-carry.integration.test.ts` proves both. An ejected project runs `drizzle-kit` itself, and a rebuild there drops the CHECKs the same way. That is the External Project Owner's to handle.
