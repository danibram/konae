# Installation store inventory

Run the server binary's `inventory` command on an installation data root to check that each store has a recovery policy.

```sh
KONAE_DATA_DIR=/mnt/volume/installation /path/to/konae inventory
/path/to/konae inventory --policy
KONAE_DATA_DIR=/mnt/volume/standalone /path/to/kernel inventory
/path/to/kernel inventory --policy
```

Use a read-only view of a stable volume for certification. The command reads directory entries and file types. It starts no workers, takes no Kernel lock and opens no SQLite, secret, app file or network connection. Standard output contains the binary name, version, catalogue version and counts by component. An unknown name, unexpected type, unreadable directory or missing required store ends the command with a nonzero status. The diagnostic identifies the path on the operator's machine. Keep those paths local.

`--policy` prints the matrix for the binary even when the volume is unavailable. Each row gives the scope, component name, writer, required or optional presence, capture rule, restore treatment and test group. A `dir:` row names the scope to inspect next. A `tree` row assigns the whole directory to one writer, such as the app's artifact or transfer scratch. The inventory does not inspect those contents. The `external` rows describe dependencies that the operator checks separately. A successful count certifies classification. The operator still verifies the manifest, object bytes, access rules and recovery of the selected point. [Version 1 policy matrix](../../kernel-rs/kernel/src/inventory.tsv)

## Which operation to run

| What the operator needs | Operation and destination | What happens to access |
| --- | --- | --- |
| Recover an installation after losing its volume | Restore a directory or S3 copy into an empty data root with the matching server binary. This brings back all app folders. A Konae copy also carries the owner accounts and project catalogue. | Restore signs out users and owners, drops agent credentials and open links, ends pending mail and applies the target's revocation record. The copy carries per-app secrets. |
| Move one published app to another installation | Export the app folder and import its sources and selected data parts. The destination runs the same rules and has its own installation metadata. | The portable identity file carries accounts. Imported session tokens and API keys grant no access. The destination supplies secrets. Import into an existing app preserves its current access under the import rules. |
| Undo a change in one app | Use the Data panel or `database_execute` with `list-backups` and `restore-backup`. The record names its database snapshot or the data and sources it holds. Restore first saves the state it replaces. | An internal restore keeps the app on its installation. Compound restores preserve current sessions and keys according to the import rules. |

A directory containing a single exported app cannot establish the presence of root databases or their owners. The standalone profile recognizes `mail-key`, which the binary generates locally. The Konae profile recognizes `owners.sqlite` and `mcp.sqlite`. Both profiles expire open Google flows and retain the app's configured Google client in `data/secrets.sqlite`. [Export and import](./export-and-import.md) · [Restore and reconcile](../operate.md#data-directory-and-copies)

## Write paths

Paths below the crate names are relative to `kernel-rs/<crate>/src/`. The same runtime writes for browser, HTTP and local operator calls. Some apparent reads also write. For example, an export opens download transfers and app page visits can enqueue Usage.

| Entry point or process | Code and stores it changes |
| --- | --- |
| Owner MCP and editor | `konae/mcp.rs` creates, claims, saves and deletes projects, updates source revisions, grants, plans, import receipts and audit in `mcp.sqlite`. Deletion parks the folder under `deleted/`, removes addresses and deletes uploaded bytes. `owners.rs`, `oauth.rs`, `admission.rs` and `clerk.rs` update owner identity, sessions, admission codes and delegated credentials. |
| Source edit, publish and start from a folder | `kernel/sources.rs`, `lib.rs::deploy_input` and `folder.rs` stage and replace sources, settings, installed record, served artifact and the migrated SQLite. The release records the applied settings even if a later draft differs. `module_set` and offline module commands update the installed settings. |
| Action, route, SQL and account administration | `kernel/lib.rs`, `runtime.rs` and `http.rs` run mutations, manage API keys and record replay claims. `auth.rs`, `accounts.rs`, `enter.rs`, `invitations.rs` and `google.rs` write identity, expirations and sign-in challenges. Account and API-key administrators also write the revocation ledger. |
| Upload, replace and delete | `kernel/files.rs`, `lib.rs`, `file_store.rs` and `transfers.rs` change the index, local or S3 bytes, upload receipts and temporary transfers. Online owners and app users reach the same file store. |
| Data import, export and inner restore | `kernel/package.rs` snapshots, sanitizes and checks export parts. `import.rs` saves the selected parts before replacing live files, identity, Usage or sources. `lib.rs::create_backup` writes a flat snapshot. The owner MCP records the result. |
| Jobs and mail | `kernel/schedule.rs` runs the mutation. `usage.rs` writes slots, run outcomes, totals, Signals and retention. `mail.rs` writes the queue and the chosen development or test mailbox. `calls.rs` leases outbox entries and runs continuations against app rows. |
| Addresses and maintenance | `kernel/domain.rs`, `hostname.rs`, `path.rs` and `maintenance.rs` commit or remove per-app markers. `recovery.rs` writes the installation hold and reconciles identities. |
| Offline or outside the server process | `kernel/operator.rs` runs SQL, changes accounts, keys, modules and maintenance, and imports into a stopped Kernel. `file_store.rs` migrates stores or reconciles orphan bytes. `copies.rs` snapshots, prunes and restores. Owner suspend/resume and admission commands update the owner stores. A folder author's build, local reset, editor save or direct filesystem edit can replace files outside the server's locks. |

## How a copy admits writers

Both servers hold a shared permit through the HTTP handler, including its blocking database work. The mail, scheduled-action and call workers hold the same permit while they commit their outcomes. A copy takes the exclusive permit, waits for operator commands and drains queued Usage writes. Its cut time is the time after this drain. Directory and S3 targets receive the same staged files.

| Writer or storage mode | What protects the cut |
| --- | --- |
| Running server | Requests and workers resume after the copy opens its readers and stages files that can change in place. A project deletion, import, internal database restore or publish that replaces a database waits for the SQLite readers to close. |
| Operator command | Commands that change a live installation take a shared file lock on its data directory. An external import also takes a shared lock on `apps/`. A copy takes both locks exclusively to fix its readers. It releases the root at admission and releases `apps/` when the SQLite pages have been copied. |
| Restore, file-store migration or reconciliation | `hold_while_stopped` takes `kernel.lock` and an exclusive lock on the data directory before inspection. Keep the directory and lock file in place throughout the command. For an editor, build, reset or direct database repair on the live tree, stop the server and hold these same locks through the last write. A process or S3 client writing independently of this protocol can change the volume during capture. |
| SQLite | A read-only connection executes `BEGIN` and reads the schema. The backup API reads pages from that connection. A rollback-journal database is staged during the pause. A WAL reader can keep its view while new transactions commit. |
| Sources, artifacts, history, file indexes and blobs | The copy stages ordinary files during the pause. It opens local blobs by inode and versioned S3 objects by GET. It stages unversioned S3 bytes before admitting writers. It checks the length and recorded digest before publishing the manifest. |

The pause has one 1,850 ms budget for draining and staging together. Health records a failed attempt if the budget expires. `KONAE_COPY_READERS` caps simultaneous pins (default 512). `KONAE_COPY_STAGE_BYTES` can lower the space budget below available disk space. Once writers resume, the copy has 60 seconds to materialize its readers. Compression, encryption and uploading a sealed point take place after admission.

An interrupted owner suspension leaves an intent in `owners.sqlite`. Konae finishes credential and ledger revocation at startup before serving. For a release, import, project change, action with declared effects, or delivery whose outcome is uncertain, the copy diagnostic names the subject, the phase and its record in `.copy-operations/`. Keep the record while investigating the app, catalogue or delivery. If the commits cannot be verified, restore the last verified installation point into an empty volume. An ordinary page read holds only the in-memory permit.

## Read-only observations

The 18 November 2026 sample used v0.10.0 at `af1a897` as its code reference. The observations below report counts. The tests create their own files with fictitious names.

| Installation | Observed | Policy comparison |
| --- | --- | --- |
| Production Konae | Six app folders each had four app SQLite stores. The data root had six SQLite files. Five appeared in the directory copy. Its manifest listed 637 entries, 255 blobs and 291,988,147 logical bytes. The service reported an encrypted S3 target every 3,600 seconds. | The five root snapshots match the Konae copy registry. The sixth database, `google.sqlite`, holds incomplete OAuth flows. The live data and directory target shared a filesystem. The sample alone cannot establish survival after volume loss. |
| Production internal history | Three records in that directory copy lacked their payload directories. The live directories held 12 files in total. | Each record needs a flat snapshot or a payload directory. A compound point can contain SQLite, a file archive and saved sources. The old writer omitted nested payloads. Issue #35 verifies the record's declared scope and byte count, including a data snapshot with no components. |
| Persistent local Konae, supplied sample | Four app folders had sources and secrets. Three had a published artifact, database, identity and Usage. None had a file index. | A draft may have only settings and sources, with optional secrets. Count it without creating `dist/`, an index or an empty database. Older drafts can also depend on `.konae/sources.json`. |
| Additional persistent root on mbpd | A metadata-only inspection counted five root SQLite files and two app folders with sources and secrets. One had an artifact, database, identity and Usage. Neither had a file index. | A separate non-Konae directory at the root lacked a policy. The inventory command refused certification. Its operator can identify the writer and decide how to recover that directory. |

The two supplied samples contain no counts for local mail keys or old source metadata. Disposable tests cover both. The production figures come from the supplied root and manifest survey. They establish neither a fresh production scan nor a successful restore from that point.

## Reproduce the checks

Run from the repository root. The storage tests use temporary directories. The aggregate fixtures recreate the observed store shapes and blob count. They do not reproduce private contents or the logical byte total.

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0
cargo test --locked --offline --manifest-path kernel-rs/Cargo.toml --workspace --test inventory
cargo test --locked --offline --manifest-path kernel-rs/Cargo.toml -p kernel a_declared_store_or_module
bun install --cwd examples/docs-site --offline --frozen-lockfile
bun test --timeout 120000 scripts/inventory-policy.test.ts scripts/docs-site.test.ts
cargo test --locked --offline --manifest-path kernel-rs/Cargo.toml --workspace
```

| Matrix test group | Evidence checked by the suites |
| --- | --- |
| `samples` | The Konae command counts the two fixture layouts. The tests compare every entry's size and modification time before and after. An orphaned record, missing settings, unknown store, wrong file type or symbolic link fails. A restored empty-data marker has its own count. |
| `profiles` | Each binary runs its own inventory command before startup. The registry test removes root, app and module policies and adds an undeclared-in-policy store. The Bun test checks declared filenames and layout paths even if a fixture has never created them. |
| `legacy` | `a_copy_restores_a_legacy_draft_without_creating_optional_stores` reads the same editor document before and after directory restore, including the old name, timestamp and directories. |
| `contracts` | `a_restore_keeps_api_keys_live_and_an_import_revokes_them`, `the_incoming_identity_keeps_the_live_sessions_and_keys` and `restoring_a_backup_first_backs_up_the_live_database` exercise the three access contracts. |
| `files` and `addresses` | `a_copy_of_an_s3_installation_carries_the_file_bytes` checks that restored bytes live in the folder. `a_copy_carries_the_custom_domain_with_the_record_and_the_maintenance_state` checks each address marker individually. |
| `recovery`, `mail` and `calls` | `a_restore_applies_the_revocation_record_or_holds_without_it`, `a_copy_carries_the_mail_queue_and_a_restore_sends_none_of_its_pending_jobs` and `test_calls_are_captured_across_restart_and_replies_run_then_once` check ledger failure, mail invalidation and worker state. The production outcome of an in-flight external call remains part of the #41 drill. |

## Verification on mbpd

On 18 November 2026, both server binaries reported v0.10.0 and catalogue version 1. `konae inventory --policy` listed 59 rows. `kernel inventory --policy` listed 60. On first startup in test mode, each disposable volume acquired `apps/`, `kernel.lock` and the test mail queue. The standalone volume also acquired `mail-key`. The Konae volume acquired `owners.sqlite`. Both answered the health request and passed inventory after shutdown.

| Issue #36 criterion | Observed result |
| --- | --- |
| Versioned matrix and write paths | Both binaries resolved all seven declared modules, all four app database names and their respective installation databases. The standalone binary refused an owner store. |
| Real installation comparison and optional data | The production survey accounted for the sixth root database and exposed three missing backup payloads. The additional local root refused certification. The fixtures retained their original entries and timestamps. A directory restore reopened the original legacy draft with zero Deployments and no app data directory. Listing keys, secrets and calls on a static app left the optional stores absent. Importing an ownerless file left the app identity database absent. |
| Refuse a missing policy and distinguish the operations | Removing any one of a registered root store, an app database or the page module policy failed validation. Registering a new account store without a snapshot also failed. The commands returned separate recovery instructions for an installation copy, portable app and internal backup. |

A separate trial on native MinIO on loopback recovered an S3-backed attachment from a directory copy onto an empty volume. The local file contained the original bytes. To repeat with a disposable S3-compatible endpoint, provision an empty bucket and its test credential as defined by `test_s3_store` in `file_store.rs`, set `TEST_PORT` and `TEST_BUCKET` to that endpoint’s port and bucket, then run the file-byte test.

```sh
KONAE_TEST_S3_ENDPOINT="http://127.0.0.1:${TEST_PORT}" KONAE_TEST_S3_BUCKET="${TEST_BUCKET}" \
  cargo test --locked --offline --manifest-path kernel-rs/Cargo.toml -p kernel --lib a_copy_of_an_s3_installation_carries_the_file_bytes
```

The Rust workspace passed 635 tests and skipped the opt-in SQLite benchmark. Both binary inventory suites and the missing-policy regression passed again after the account-store and orphan-journal checks landed. The documentation and filename-policy suites passed 12 tests.

The three incomplete historical points still need the nested-restore check in #35. The additional mbpd root needs an operator policy for its unclassified directory.
