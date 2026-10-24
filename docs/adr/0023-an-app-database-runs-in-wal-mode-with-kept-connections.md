# An app Database runs in WAL mode with kept connections

Status: accepted, 2026-11-09. Amends nothing; it changes how the Kernel reaches `deployments/<id>/database.sqlite`, not what an app declares or what a package carries.

Until now a Server Action opened a new connection to its Database, installed the limits and the authorizer, prepared its statement, ran it and closed the connection. Every action of one Deployment, a query as much as a mutation, waited for one mutex, and the file was in rollback-journal mode (`journal_mode=DELETE`), with a full sync at every commit.

In WAL mode a reader sees the last commit and never waits for the writer, and a commit appends the pages it changed to the log instead of syncing a journal and the file.

## Decision

- An app Database is in WAL mode. A new one starts in it, and one written before, or arriving from a snapshot or a backup, moves into it the first time a Server Action opens it. The mode lives in the file.
- A connection runs with `synchronous=NORMAL`. A crash of the Kernel loses no commit. A host that loses power can lose the last commits before a checkpoint, and the file stays consistent. `synchronous=OFF` would be faster still, but it is safe only where another store holds the durable copy, and here the disk is the durable copy.
- A Server Action borrows a configured connection from a small pool (`kernel-rs/src/app_db.rs`): 8 per Database and 64 across the Kernel. Each connection keeps its prepared statements. Each borrow starts a fresh 2-second deadline and resets SQLite's last inserted rowid, so an UPDATE never answers the id an earlier INSERT made on the same connection.
- Queries share the Deployment's lock, so they run side by side and beside a write. Mutations also take the Deployment's write lock, so they reach SQLite one at a time instead of meeting `SQLITE_BUSY`. Work that copies, restores or replaces the whole file holds the Deployment's lock alone, as before, and drops every kept connection when it takes it.
- A kept connection is lent only while its path still names the file it opened. A file renamed over the path closes the old connections. `replace_database_file` removes the old file's `-wal` and `-shm` before the rename, so SQLite never reads the old file's pages into the new one.

## Measured

`cargo test --release sqlite_bench -- --ignored --nocapture` on an arm64 Mac, 20,000 rows, through the same lock and run path as the action route:

| Case | Before | After |
| --- | --- | --- |
| One read by id, p50 | 0.091 ms | 0.014 ms |
| One insert, p50 | 0.456 ms | 0.032 ms |
| 32 readers and 1 writer, actions per second | 5,518 | 15,754 |
| The writer's insert under that load, p50 | 5.2 ms | 0.49 ms |

The insert row has two causes: the kept connection, and `synchronous=NORMAL`, which drops the sync at each commit. WAL mode alone, with a connection opened per action, made a read slower (0.6 ms p50): the close of a Database's last connection checkpoints the log and removes it. The kept connections are what turn WAL into a gain.

## Consequences

- A package, a copy and a migration are unchanged: each reads the Database with `VACUUM INTO`, which is consistent in WAL mode and writes a file in rollback mode.
- The live `database.sqlite` carries the WAL flag. bun's SQLite (3.51) refuses to open such a file read-only once the Kernel has closed its last connection and removed the `-shm`; the Kernel's bundled SQLite opens it. A tool that reads a live Database from disk opens it read-write, or reads a copy.
- An idle Kernel holds up to 64 open Databases, each with three file descriptors and up to 2 MiB of page cache.
