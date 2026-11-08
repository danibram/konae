# schema.sql is the only model, and a release is all or nothing

Status: accepted.

A person who writes SQL edits `schema.sql`. A person who does not write code asks the agent, which edits the same file. Both must see the same database on every machine, and a publish that fails half way must not leave the app's actions running against rows they do not fit.

## Decision

### `schema.sql` states what the database should be

- `schema.sql` holds `CREATE TABLE` and `CREATE INDEX` statements. It is the only model of the database.
- At publish, and when the `kernel` binary starts an app folder, the Kernel runs `schema.sql` in a scratch database in memory and compares its tables, columns and indexes with the live database, reading both through SQLite itself. Only statements the Kernel writes from that comparison touch the live file.
- A new table, column or index applies. A table whose definition changed is rebuilt the way SQLite's documentation on `ALTER TABLE` describes, in one transaction.
- A dropped table, a dropped column and a column whose type changed lose something. `release_plan` names what is lost, with row and value counts, and returns a confirmation that is a digest of the statements. The change applies only with that confirmation, so a confirmation fits only the plan it was given for. A rename reads as a drop and an add.
- `actions.d.ts` is generated from the Action Contract, so `miniapp.actions.call` is typed by action name, input and returned columns.

### A release is all or nothing

A publish stages and uploads first. It then copies the database, applies the schema change to the copy, and swaps the database, the Deployment record, `dist/` and the Deployment in memory together, putting back each earlier swap if a later one fails. Server Actions wait from the copy to the swap. A publish that fails leaves the old actions answering on the old rows.

### The database runs in WAL mode with kept connections

- An app database is in WAL mode, and a file that arrives in another mode moves into it the first time a Server Action opens it.
- A connection runs with `synchronous=NORMAL`. A crash of the Kernel loses no commit. A host that loses power can lose the last commits before a checkpoint, and the file stays consistent.
- A Server Action borrows a configured connection from a pool in `kernel-rs/kernel/src/app_db.rs`: 8 per database and 64 across the Kernel. Each borrow starts a fresh 2-second deadline and resets the last inserted rowid.
- Queries share the Deployment's lock and run side by side and beside a write. Mutations also take the write lock, so they reach SQLite one at a time. Work that copies, restores or replaces the whole file holds the lock alone and drops every kept connection.
- A kept connection is lent only while its path still names the file it opened. Replacing the file removes the old `-wal` and `-shm` before the rename.

## Rejected options

- **A Drizzle `schema.ts` as the model, with generated migrations.** It needed a subprocess to evaluate the owner's code, a migration history to keep, and a differ that asked a terminal whether a change was a rename. It also gave the person who writes SQL a second file to keep in step. SQLite can compare two databases itself.
- **Expose `schema.sql` and `schema.ts` as two editable files.** Two writable models drift.
- **Guess renames.** The two readings differ in whether the owner's rows survive. The Kernel shows what is lost and asks.
- **Apply the schema change to the live database before the swap.** A failure after it would leave the new schema under the old actions.
- **`synchronous=OFF`.** It is safe only where another store holds the durable copy, and here the disk is the durable copy.
- **WAL with a connection opened per action.** The close of the last connection checkpoints the log, which made a read slower than before. Kept connections are what make WAL faster.

## Consequences

- The editor preview runs `schema.sql` on its own test rows and never changes a table it already has. When the preview lacks a table or column that `schema.sql` declares, the Gate warns and passes. Try such a change on `konae dev` or a published Deployment.
- An export and a copy read the database with `VACUUM INTO`, which is consistent in WAL mode and writes a file in rollback mode.
- A tool that reads a live database from disk opens it read-write, or reads a copy, because bun's SQLite refuses to open a WAL file read-only once its `-shm` is gone.
