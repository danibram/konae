# Export and import

Export the app folder to take one app's accounts, data and rules to another machine, and import it into a new app or over an existing one.

## Export and import an app

`project_export { projectId }` exports the published app and needs `export_data`. It hands over the app folder: `folder`, one download of a ustar archive with `konae.json`, `app/` and `dist/`, and one download per part of `data/`, captured under the app's locks so no write lands halfway. `sourcesAhead` compares `app/` with the release's `sourceDigest`. Older releases without a digest compare `app/actions.json` with the actions the release runs. Secrets, owners, credentials, sessions and `.konae/` never leave.

```json
{
  "manifest": {
    "format": "konae-package", "version": 1, "packageId": "…", "createdAt": 1762430000000,
    "deploymentId": "…", "projectId": "…", "visibility": "private",
    "signupPolicy": { "mode": "closed", "allowlist": [] },
    "publishedRevision": 4, "artifactVersion": "v…", "sourcesRevision": 9,
    "contents": [
      { "name": "database", "size": 268435456, "sha256": "…" },
      { "name": "identity", "size": 110592, "sha256": "…" },
      { "name": "files", "size": 20987904, "sha256": "…" },
      { "name": "usage", "size": 61440, "sha256": "…" }
    ],
    "excluded": ["app sessions", "verification, recovery and invitation links", "…"]
  },
  "downloads": { "database": { "transferId": "…", "size": 268435456, "sha256": "…", "chunkBytes": 4194304, "expiresAt": 0 } },
  "folder": { "transferId": "…", "size": 40960, "sha256": "…", "chunkBytes": 4194304, "expiresAt": 0 },
  "sourcesAhead": false
}
```

| Component | What it is | Limit |
| --- | --- | --- |
| `database` | The app database, including pending Calls and portable notice intents, copied with `VACUUM INTO` | 256 MiB |
| `identity` | `identity.sqlite` without sessions or email links, with every API key revoked at import | 64 MiB |
| `files` | A ustar archive with the Files index and the bytes referenced by registered modules | The destination's per-file limit and app-wide file quota |
| `usage` | `usage.sqlite`, present when the app recorded Usage or a Signal | 128 MiB |

`publishedRevision` counts releases. `sourcesRevision` advances when Konae accepts sources through project creation, source saves, release, source import or source-backup restore. `konae.json` and `app/` in an export can carry a draft ahead of the release. The `released` snapshot in `konae.json` (`konae.json.released`) and the Kernel's Installed record retain the effective released settings, actions, schema and artifact identity. On restart, an unchanged `dist/` keeps that applied snapshot; a changed artifact applies the current sources and matching settings through the normal schema confirmation path. Legacy folders recover applied state from their Installed record and live database rather than treating a saved draft as released. Read each download with `transfer_read` until `nextOffset` equals `size`, check its SHA-256, and cancel it. The downloads expire after an hour and need `export_data`. The audit log gets one `project_export` row and one `package_download` row per download.

When moving a live app, put the source in maintenance before the final export. Resume delivery on one installation. Export refuses while a Call attempt is in flight or a notice awaits a terminal result in the installation mail queue. Pending notice intents that the queue has yet to accept can resume after import.

To import into a new app on another installation, create the project there with `project_create` and the same `projectId`, and set every signing secret with `secret_set`. Upload each component with `transfer_begin { size, sha256, projectId, component }`, so the upload may reach that component's limit; `McpClient.upload(bytes, projectId, component)` in `src/mcp/client.ts` does the three calls. Then send the release payload, the one `release_plan` always takes, with `initialPackage`:

```json
{ "project": {}, "visibility": "private", "artifact": {},
  "initialPackage": { "manifest": {}, "components": { "database": "transferId", "identity": "transferId", "files": "transferId" } } }
```

`release_plan` checks, before any Deployment exists:

- the manifest's format and version, and every component's limit;
- each upload is complete, belongs to the caller and the project, was started for that component and matches the manifest's size and SHA-256;
- every SQLite file passes `integrity_check`, and the accounts file has `app_users`;
- every file has its bytes at the indexed size, within the quotas, and every file owner is an account in the export;
- the volume has room for the components, a copy of the database for the schema check and 64 MiB to spare;
- the release's `schema.sql` and actions run against the imported database.

`release_apply` copies each component into the new app folder's `data/`, checking its SHA-256, deletes sessions and email links, revokes live API keys and unpacks the files. Pending invitations arrive without their links, so each needs `app_invitation_resend` on the destination. A failed apply can be retried with a new plan, and repeating an applied plan returns its receipt. The Signup Policy comes from the manifest unless the payload sets `initialSignupPolicy`. [The walkthrough](../mcp-walkthrough.md#14-import-on-a-second-kernel) shows the calls.

| Refusal | Message |
| --- | --- |
| Export without `export_data` | `Unknown tool or insufficient permission. project_export needs a credential with export_data. Run konae login with --export-data, or use a token from token_create that includes it.` |
| Reading an export's download without `export_data` | `Permission denied for export_data.` |
| `initialPackage` for a project that has a Deployment | `Portable state can only be imported on the first deployment.` |
| Upload bytes that differ from the declared SHA-256 | `Upload size or SHA-256 mismatch.` |
| An upload that differs from the manifest | `The database component does not match the size and SHA-256 in the manifest.` |
| Another manifest version | `Package version 2 is not supported. This Kernel reads version 1.` |
| A custom domain another Deployment on the destination serves | `Package import failed: <name> already serves another Deployment on this installation. …` |
| A component over its limit | `Transfer size is outside the limit of 67108864 bytes for the identity component.` |
| Not enough disk at `transfer_begin`, `project_export` or `release_plan` | `Not enough free space on the Kernel volume to import this app: 340 MiB needed, 120 MiB free.` |
| A database that holds a trigger or a view | `The database holds CREATE TRIGGER <name>, and Konae runs no triggers or views. Drop it before importing.` |
| An accounts or usage component that holds a trigger or a view | `The accounts component holds CREATE TRIGGER <name>, and Konae runs no triggers or views. Drop it before importing.` |
| An accounts component with a password hash this Kernel did not write | `The accounts component holds a password hash this Kernel did not write.` |

### Import into an existing app

`project_import` replaces a published app with an exported folder and needs `publish`. Every request needs `projectId` and `replace`. A `sources` or `both` import also requires `transferId`, the completed upload containing the release JSON. It may carry the source files and artifact itself. `sourcesTransferId` and `artifactTransferId` are optional uploads for those parts when they travel separately. A `data` or `both` import requires `package { manifest, components }`:

- `sources` requires `transferId`; the release JSON describes the project and artifact. Optional `sourcesTransferId` carries files under `app/`, and optional `artifactTransferId` carries `dist/`. It publishes those inputs and makes the source files the editor opens.
- `data` takes `package { manifest, components }`, uploaded as for a new app, and replaces the database, the accounts, the files and Usage. The database takes the live schema.
- `both` requires `transferId` and the data package. The release JSON can include source and artifact files; optional `sourcesTransferId` and `artifactTransferId` split those uploads. The database takes the release's schema.

It never merges rows. It first writes a backup of what it replaces, which `database_execute` operation `list-backups` lists as `Before import`, with `holds` naming `data`, `sources` or both, and `restore-backup` brings back. The app keeps its Signup Policy, sign-in methods, addresses, secrets, sessions and API keys. A schema change that loses data answers `{ imported: false, replace, schema }` and changes nothing; call again with `schema.confirmation` as `confirmSchema` once the owner agrees. Otherwise the answer is `{ imported: true, replace, backup, schema }`. `project_import` works while the app is in maintenance. In the editor, App settings has an Import section that reads the `.tar` that Export downloads.

`konae export --dir <folder> --out <empty folder> --with-data` writes the local app folder and rebuilds `dist/`. `konae pull` downloads the published app folder, data, sources and `dist/` into `.konae/pull/`, and records the observed source revision and digest for that installation and project. The exported `app/` can contain draft sources ahead of the release; `konae.json` carries the applied rules in its `released` snapshot (`konae.json.released`). A clean pull installs incoming `app/` and `konae.json`; it leaves `dist/` staged under `.konae/pull/dist/` and does not replace the local build. If local source or settings digests differ, pull preserves them and stages the incoming `app/`, `dist/` and `konae.json`.

To accept staged sources, first preserve local work outside the app folder, including both `app/` and `konae.json`. From the app folder, use a new sibling directory for the backup:

```sh
mkdir ../my-app-before-pull
mv app ../my-app-before-pull/app
cp konae.json ../my-app-before-pull/konae.json
cp -R .konae/pull/app ./app
cp .konae/pull/konae.json ./konae.json
konae pull --dir .
```

Keep the backup until you have checked the incoming files. The second pull records the observed source revision. To carry local edits forward, merge them from the backup and run `konae publish`. This resolves the source revision conflict; it does not import the staged data. Run `konae dev --reset` to back up local `data/` and import the pulled data. `konae push` refuses while that data import is pending, and becomes available after `dev --reset` records the imported package. On a `kernel` binary without Konae, `kernel import <folder>` replaces the app's data after a backup.

## Read next

- [Take the app elsewhere](../guides/take-the-app-elsewhere.md)
- [Invite people](../guides/invite-people.md)
- [The CLI](../cli.md#pull)
- [The walkthrough](../mcp-walkthrough.md#14-import-on-a-second-kernel)
