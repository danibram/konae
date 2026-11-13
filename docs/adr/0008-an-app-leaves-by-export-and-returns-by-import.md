# An app leaves by export and returns by import

Status: accepted.

Job 4 asks that either person can take the app to another machine with its accounts, its data and its rules. One export and one import serve every case: leaving Konae, moving between installations, pulling a published app's data to a laptop and putting a rehearsed version back. A scheduled copy and a restore are a separate thing, which recovers a whole installation.

## Decision

### Export hands over the app folder

`project_export` hands over the app folder of ADR-0001 as transfers in 64 KiB blocks, so no part of it sits whole in memory at either end. The `folder` download holds `konae.json`, `app/` and `dist/`. Each part of `data/` is one component: database, identity, files and usage. The Kernel captures them under the app's locks, as a scheduled copy does. A manifest lists every component with its size and SHA-256 and names the revisions that produced the app.

- The export carries everything of the app and nothing of Konae: no owner account, no credential, no project record, no secret and none of the installation's `.konae/`.
- The identity component leaves without sessions, email links or entry links. Each API key travels with its name, actions, file visibilities and hash, marked revoked with the reason `import`, so the export opens nothing even on a Kernel that would not revoke it. The editor lists such keys under "Revoked at import" with what each could call.
- Downloading accounts and data needs `export_data`. OAuth does not offer it (ADR-0002).
- `konae pull` is an export of the published app's data, and `konae dev --reset` imports it into the local folder. The data is what the installation held at the time of the pull.
- Local work never reaches the published app on its own. `konae dev` serves the folder's own `data/` on the `kernel` binary, `pull` runs only when asked, and `konae sql` and `konae events` read the published app only with `--prod`.

### Import takes a folder

- Into a new app, the components travel as transfers named in the release payload as `initialPackage`. `release_plan` checks the manifest, the checksums, SQLite integrity, the files against the accounts they name and the free space before any Deployment exists. Konae assigns the app to the owner who imports it.
- Into an existing app, `project_import` or the `kernel` binary's `import` replaces the sources, the data or both, as the owner chooses, after a backup the Data panel lists and `restore-backup` brings back. It never merges rows. The database takes the schema the app will run. Sessions and API keys of the live app survive, as after a restore, and sessions of accounts the new data does not hold end.
- An import that replaces data is the way local rows reach a published app. The owner decides that, and the backup is how they undo it.

### Maintenance pauses an app

An owner puts an app in maintenance with `maintenance_set` or the switch in App settings, and an operator with the `kernel` binary's `maintenance`. The state lives in the app's `.konae/maintenance.json`, so a restart, a copy and a restore keep it, and the Kernel reads it on every check. While it pauses an app, the app's host serves a maintenance page for the shell and refuses every other request, tools that would change the app are refused except `maintenance_set` and `project_import`, the mail worker holds the app's email, and its calls and schedule wait.

Moving an app with no lost write is maintenance on the origin, export, import on the destination. Nothing enforces that the origin stays paused. The owner who moves the app decides when the old copy stops.

### A copy and a restore recover an installation

- The Kernel copies its data directory on a schedule, every app folder and every installation store, to a directory, an S3 prefix or both, with a manifest of sizes and SHA-256. With `KONAE_COPY_SECRET` set, each file sent to S3 is sealed with AES-256-GCM, and `restore` needs the same secret. The secret also signs each manifest and each revocation entry sent to S3, so `restore` refuses a copy it cannot authenticate; `KONAE_COPY_ACCEPT_UNSIGNED=1` accepts copies and entries an older Kernel wrote without a signature.
- `restore` installs one copy into an empty data directory. It signs out every session and agent credential and ends every open email link, because the people can sign in again in a minute. It keeps API keys live, because they sit in programs the owner may not control and the app is still the same app.
- Every suspension of an account and every revocation of an API key is written to each copy target, as one file under `revocations/`, before the tool reports success. `restore` applies that record before the Kernel serves, so nothing suspended or revoked after the copy comes back. It applies only suspensions and revocations, never a resumption. When the record cannot be read, the restored installation starts in maintenance until `reconcile` applies it or the operator reopens without it.

## Rejected options

- **Several export formats.** A small JSON export, a database download, a self-host kit, an Eject package and a chunked package each left out something the others carried. One folder carries one set of things.
- **A live migration that kept one writer through a confirmation handed between installations.** It cost five tools and a second manifest version to prevent something the owner who moves the app can prevent by pausing it.
- **Refuse any push of local data to a published app.** It left the person who rehearses a change locally with no way to put the result back except retyping it. An import replaces data only after a backup and never merges rows.
- **Let `dev` pull on its own when a credential is present.** Every run of the local loop would read production, and every developer would need the permission to download accounts.
- **Delete API keys from an export.** The destination's owner could then not tell which programs had access at the origin.
- **Revoke API keys on a restore.** A restore brings back the same app for the same owner, and its keys sit in programs the owner may not control.
- **Apply resumptions from the revocation record.** An entry that failed to reach a target would leave an account suspended or active depending on which entry got through. A suspension that stays errs on the safe side.

## Consequences

- An app imported into a new installation needs its secrets set again (ADR-0013), its custom domain proved again and its path name claimed again if another app holds it there (ADR-0014).
- Pending email and queued calls stay on the installation that queued them, so an imported app sends none of its origin's.
- A key revoked at import shows to the new owner, who issues new keys.
- A copy target that cannot be written makes suspensions and revocations report an error until it can.
