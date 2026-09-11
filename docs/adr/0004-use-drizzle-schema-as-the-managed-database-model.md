# Use a Drizzle schema as the Managed Project database model

Status: accepted

Every Managed Project will expose a constrained Drizzle `schema.ts` as its canonical, editable Database Model. Users and agents iterate on that model in the Littlebox editor; Littlebox derives SQL migrations, validation artifacts and database backups through one managed database-lifecycle module. Generated SQL remains reviewable and portable, but it is not a second editable source of truth.

## Decision

The Managed Capsule evolves from an editable `schema.sql` to an editable `schema.ts` containing Drizzle table, column, relation, constraint and index declarations. Littlebox pins the supported Drizzle version and imports, provides autocomplete and diagnostics, and prevents arbitrary modules, drivers, credentials or executable package configuration from entering the schema file. The same Database Model is supplied to the agent, the editor's schema view, the Validation Gate and migration generation so they cannot disagree about the intended structure.

`schema.ts` is an authoring and build input; it is never included in frontend output and does not give browser code a database connection. `actions.json` remains the Managed Capsule's set of named runtime SQL operations, checked against the schema-derived database. Browser code continues to invoke those actions through `@littlebox/sdk`, and the Kernel continues to authorize them as required by ADR-0001.

The database-lifecycle module hides Drizzle Kit, migration bookkeeping and persistence adapters behind a small Kernel interface. A successful managed schema change performs these steps as one coordinated operation:

1. Typecheck and evaluate the constrained `schema.ts`, then materialize its target SQLite structure in a disposable database.
2. Generate a candidate SQL migration from the previously accepted Drizzle snapshot to the new Database Model, preserving its SQL, hashes and migration metadata.
3. Apply the candidate to a disposable copy of the current database and verify convergence, constraints and affected Server Actions.
4. Present the migration and any destructive or ambiguous operation for the required Project Owner approval.
5. Ask the persistence adapter for a consistent backup immediately before changing the real database.
6. Apply the ordered SQL migration through Drizzle's migration machinery and record the new model revision only after it succeeds.
7. Verify the resulting schema and retain the pre-migration backup as its recovery point.

Physical snapshot, retention and restore remain the responsibility of the database persistence adapter—SQLite files, D1 or another provider—not Drizzle. Keeping them inside the same lifecycle operation prevents publication, administration and the Development Kernel from implementing different safety sequences.

Managed users and agents may inspect generated migrations and SQL schema snapshots but cannot edit them independently, run `drizzle-kit` or provide a database connection. Existing `schema.sql` Projects receive a one-time, verified Drizzle baseline derived from their materialized SQLite schema; their original SQL is retained in history but stops being the active Database Model. This decision supersedes only ADR-0003's assumption that the agent edits `schema.sql`; its scratch workspace, agent loop and Validation Gate remain in force.

Eject exports the canonical `schema.ts`, ordered SQL migrations, Drizzle snapshots and a conventional `drizzle.config.ts`, but no hosted credential. It may also include a generated `schema.sql` snapshot for inspection and portability. From that point the External Project Owner may use `drizzle-kit generate` and `drizzle-kit migrate`, or deliberately use `drizzle-kit push`, against databases they administer. Access to a hosted Littlebox database still requires an explicit, short-lived Admin Interface connection. Using `push` against hosted data bypasses the exported migration history, so resulting drift, rollback and compatibility become the External Project Owner's responsibility.

## Considered options

- **Keep `schema.sql` as the editable Managed Database Model and generate Drizzle metadata internally.** Rejected because users, agents, editor tooling and migration generation would reason about different representations, requiring a permanent SQL-to-Drizzle synchronization layer.
- **Expose both `schema.ts` and `schema.sql` as editable files.** Rejected because two writable representations create ambiguous schema authority and inevitable drift.
- **Hide the Drizzle model from Managed users.** Rejected because the model is the most useful surface for typed iteration, agent edits, autocomplete and eventual Eject portability.
- **Use `drizzle-kit push` as the managed deployment mechanism.** Rejected because applying a live diff without retaining the reviewed SQL migration breaks reproducibility and weakens recovery.
- **Expose Drizzle ORM to frontend code.** Rejected because that would collapse the runtime/admin separation established by ADR-0001 and move unrestricted data capability into an inspectable client.

## Consequences

Littlebox must add `schema.ts` to the Managed Capsule, constrain its imports and execution, pin Drizzle, provide editor types, retain accepted snapshots and migrations, and migrate existing `schema.sql` Projects to verified baselines. The database-lifecycle interface becomes the single seam used by preview validation, publication, administration and local development. This ADR deliberately does not decide whether persistent data belongs to a Project Environment or to each immutable Deployment; that lifetime decision still needs its own ADR before hosted migration execution is implemented.

Implementation is staged in [`docs/plans/adr-0004-drizzle-database-model.md`](../plans/adr-0004-drizzle-database-model.md).
