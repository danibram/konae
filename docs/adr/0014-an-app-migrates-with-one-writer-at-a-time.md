# An app migrates with one writer at a time

Status: accepted, 2026-11-06. Builds on [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md), and amends [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md)'s note on API keys at restore: a key revoked after a copy no longer comes back.

A package (ADR-0013) moves an app's state to another installation, but nothing stopped the origin from taking writes after the capture, nothing kept the import from serving before the origin stopped, and a restore from an hourly copy brought back accounts suspended after the copy. The launch plan asks for a migration with an explicit write pause, an import that starts disabled, and a restore that never revives a revoked identity.

## Decision

Each Deployment has a migration state in `deployments/<id>/migration.json`, inside its folder. While the state pauses the app, the Kernel answers every request to the app's host with a maintenance page for a page request and a 503 that names the operation for anything else, refuses administrative tools that would change the app, and the mail worker holds the app's queued email. The file survives a restart and travels in scheduled copies, so neither a restart nor a restore resumes a paused or retired app. A file that exists but cannot be read pauses the app.

The origin runs plan, capture and confirm with five tools behind `manage_lifecycle`:

- `migration_begin` writes the pause first, then takes the project, data and identity locks once each, so every write that started before the pause has finished, waits a bounded time for email a worker has handed to the provider, and ends the rest of the app's queued email. It then captures a package with the export code and keeps it in the Deployment's folder. It also needs `export_data`.
- The package of a migration is version 2: version 1 plus a `migration` stamp with the operation ID, the origin and the SHA-256 of a confirmation the origin keeps. A release payload that republishes the live artifact travels next to it.
- `migration_confirm` retires the origin for good, sets a retention date seven days out, may record a validated new URL that the maintenance page shows until then, and returns the confirmation in its receipt.
- `migration_cancel` resumes the origin and discards the confirmation. It is refused after confirmation.
- `migration_status` reads the state with `read`.

An import of a version 2 package starts paused on the destination. `migration_activate` needs the confirmation whose SHA-256 the package carried. Because the origin reveals it only when it confirms, and refuses to cancel once it has, the supported procedure never has two installations taking writes: before confirmation only the origin can resume, after it only the destination can activate. Activation ends any session, email link or API key in the imported accounts and can resend pending invitations with links on the new host. A second `release_plan` with a package the project already imported returns the first plan and its receipt.

Every suspension of an app account or a creator and every API key revocation is written to each copy target as one file under `revocations/` before the tool reports success; when that write fails the tool reports the failure, and repeating it records the change. `restore` reads that record from its source and applies it before anything is installed. Only suspensions and revocations are applied; a restore never lifts a suspension. When the record cannot be read, including when the source has no `revocations/started.json`, the restored installation starts in maintenance until `littlebox-kernel reconcile` applies the record or the operator reopens it without the record. Restore also ends every open verification, recovery and invitation link.

With `LITTLEBOX_COPY_SECRET` set, copies sent to S3 are sealed file by file with AES-256-GCM under a key derived with HKDF, as `mail.rs` seals queued links. The copy ID and path are the associated data. The manifest names the cipher and a key check, so `restore` refuses an encrypted copy without the secret, or with the wrong one, before it installs anything.

## Considered options

- **Mark the destination active on import and trust the operator to stop the origin.** Rejected because a cancelled migration would leave an importable package that serves at once, and the plan names two active writers as the refusal.
- **Let the destination ask the origin whether it confirmed.** Rejected because the destination may never reach the origin, and the plan requires the destination to work with the Littlebox host unreachable. The confirmation travels with the operator instead.
- **Keep the pause in `mcp.sqlite` or in memory.** Rejected because the restore path copies Deployment folders, and the mail worker and the restore command have no access to the Kernel's state.
- **Redirect the origin to the new URL.** Rejected because a redirected POST or sign-in sends a password or a token to a host the user did not choose. The page links instead.
- **Apply resumptions from the revocation record too.** Rejected for now: an entry that fails to reach the target would then leave an account suspended or active depending on which entry got through. A suspension that stays after a restore errs on the safe side; the owner resumes it again.

## Consequences

The app host serves nothing but the maintenance page while paused, reads included, since the shell that would make them does not load. Owner read tools, exports and Data panel inspection keep working.

A Kernel that reads only version 1 refuses a migration package with its existing version message rather than serving it. The kit's `deploy.mjs` and the CLI keep producing version 1 packages, which import active as before.

The origin keeps its state and the kept package after confirmation. Deleting them after the retention date is the operator's step for now. Pending invitations, verification and recovery links do not travel; activation offers to resend invitations, and people ask again for the other links. Route signing secrets never travel, so the destination sets new ones before the import publishes routes that need them.

The record holds account and key IDs, not emails, and is not encrypted. A copy target that cannot be written makes suspensions and revocations report an error until it can.
