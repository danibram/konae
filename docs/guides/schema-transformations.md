# Carry data through schema changes

`app/schema.sql` declares the final schema the app will run. An optional `app/schema-transforms.json` gives the Kernel explicit SQL steps for moving existing rows from an older schema to that final schema. The Kernel checks the steps against a staged copy of the real database before it replaces live data.

## Fingerprint each schema

Use the CLI to read a schema definition or the structure of an existing database. The command starts no app and creates no `.konae/` state.

```sh
bun run konae -- schema fingerprint --dir /path/to/prior-app
bun run konae -- schema fingerprint --dir /path/to/current-app --data
bun run konae -- schema fingerprint --dir /path/to/candidate-app
```

Replace each example path with a folder that contains the named source or database. Without `--data`, the CLI fingerprints `app/schema.sql`. With `--data`, it reads `data/database.sqlite` read-only. `--json` returns the fingerprint and its source. To fingerprint an intermediate schema, put that version's `schema.sql` in a separate source folder and run the same command.

Use fingerprints printed by these commands. Do not type or edit a digest by hand. For the first transition, `before` must match the schema the existing database has now. Fingerprint that database with `--data`, or use a prior source folder when you know its `schema.sql` matches the database. Each following transition's `before` must equal the previous transition's `after`. The last `after` must match the current `app/schema.sql`.

## Add an explicit transition

Suppose the old schema has a `todos` table and the final schema calls it `tasks`:

```sql
-- Prior app/schema.sql
CREATE TABLE IF NOT EXISTS todos (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
```

```sql
-- Current app/schema.sql
CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL);
```

The transition states that rename directly, so SQLite keeps the rows and their values:

```json
{
  "version": 1,
  "transitions": [
    {
      "id": "todos-to-tasks",
      "predecessor": null,
      "before": "<fingerprint of the prior schema or observed database>",
      "after": "<fingerprint of the final schema>",
      "maxRows": 1,
      "statements": ["ALTER TABLE todos RENAME TO tasks"]
    }
  ]
}
```

The angle-bracket values are placeholders, not fingerprints. Replace them with the exact outputs from `schema fingerprint`. `maxRows` is required even when a transition contains only DDL. DDL counts zero changed rows, so this example uses the minimum value.

The v1 file is a linear chain. Each `id` is unique and each `predecessor` names the immediately previous transition, with `null` on the first one. Once a chain starts, append every schema version in order, including versions that only add a table or column. An existing database moves through every intermediate transition in order. Keep each applied transition in place and leave its ID, fingerprints, row limit and statements unchanged. The database journal records the applied definitions alongside the rows. A changed definition or a missing file stops the app until you restore the matching source history. To add a chain for a legacy database with no journal, use its observed starting fingerprint.

The Kernel does not guess that a dropped table and a new table are a rename. Write an explicit `ALTER TABLE ... RENAME TO ...` or `ALTER TABLE ... RENAME COLUMN ...` step when existing rows must keep their meaning.

## SQL and data limits

The supported statements are `CREATE TABLE`, `CREATE INDEX`, `ALTER TABLE ... RENAME TO`, `ALTER TABLE ... RENAME COLUMN`, `ALTER TABLE ... ADD COLUMN`, `INSERT` and `UPDATE`. The v1 validator refuses `CREATE TABLE AS SELECT`, `DROP`, `DELETE`, `REPLACE`, `INSERT OR ROLLBACK`, `UPDATE OR ROLLBACK`, `ON CONFLICT ROLLBACK`, triggers, views, `PRAGMA`, attachments, transaction control, external functions and access to SQLite or Konae's reserved metadata tables. The transformer is not an OS sandbox.

Set `maxRows` from 1 through 100000 on each transition. The Kernel measures it with SQLite `total_changes`, including foreign-key cascade writes. DDL counts as zero. SQLite's progress handler cancels work after 30 seconds. The app database keeps its 256 MiB quota.

Any transition with `INSERT` or `UPDATE` needs explicit confirmation. The plan binds the confirmation to the transition definitions and the observed change counts. The Kernel rolls back the whole database transaction if a step fails, a row limit is exceeded, the final schema differs from `schema.sql`, an app action no longer prepares, or foreign-key checks fail.

The editor Gate checks the document shape and supported SQL before Apply. The Kernel repeats those checks against the real database and validates the resulting actions and foreign keys on the staged copy. Review the plan for the target database before confirming it.

## Apply against the right database

Run `rehearse` to check the current sources against a fresh pulled-data snapshot. A rehearsal confirmation belongs to that snapshot. `publish --plan` plans against the current Deployment database and its journal. Publish applies the transitions to a staged copy of the live database, then swaps it in with the release only after checks pass. The production confirmation may differ from rehearsal, so use the value from the production plan when publishing. An import into an existing app checks the incoming database and transforms a staged copy before replacing its live data.

```sh
bun run konae -- rehearse --dir ./my-app [--confirm-schema <snapshot confirmation>]
bun run konae -- publish --dir ./my-app --plan
bun run konae -- publish --dir ./my-app --confirm-schema <production confirmation>
```

The angle-bracket values are placeholders. Copy the confirmation from the matching rehearsal or production plan. See the [local rehearsal guide](./rehearse-locally.md) for the pull and reset steps.

The Kernel keeps the transition journal inside `data/database.sqlite`. Export, copy and restore carry it with the rows. A fresh database installs `schema.sql` directly and records each listed transition as `baselineSkipped`; it does not replay those steps against an empty database. A local test run records history only in its own database. Publish reads the production database's journal, so test-mode history does not make production skip a step. `push` explicitly replaces production data and its journal together, after a backup.

Fingerprints hash sorted SQLite schema definitions with conservative token normalization. The tokenizer ignores whitespace and comments and normalizes recognized identifiers using the existing SQL token rules. It preserves string literal case and ambiguous double-quoted values. A fingerprint does not claim general SQL semantic equivalence.
