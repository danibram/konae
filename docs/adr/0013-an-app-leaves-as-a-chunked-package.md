# An app leaves as a chunked package

Status: accepted, 2026-11-06. Amends [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md).

`project_export` put an app's database, accounts and files in one JSON document as base64, capped at 16 MiB, while the quotas let a database reach 256 MiB, accounts 64 MiB and files 20 MiB. An app the host accepted could stop being exportable, and both ends held the whole document in memory. The same tool handed password hashes to any credential with the `read` scope.

## Decision

`package_export` captures one Deployment under its project, data and identity locks with `VACUUM INTO` and a file copy, the code a scheduled copy uses. It writes three components, `database`, `identity` and `files`, and offers each as a download transfer. A manifest with a format name and version lists every component with its size and SHA-256 and names the published revision. The identity component leaves without sessions or email links; API keys travel revoked, as the note of 2026-11-06 on keys says.

An import uploads each component as a transfer started with its component name, which raises that transfer's size cap to the component's quota, and names the transfers in the release payload as `initialPackage`. `release_plan` checks the manifest, the checksums, SQLite integrity, the files against the accounts they name, and the free space before any Deployment exists. `release_apply` copies the components into the new Deployment's folder and checks each SHA-256 again as it copies. A project with a Deployment refuses a package. Repeating an applied plan returns its receipt, so an import cannot create a second app or second accounts.

Downloading state needs a new permission, `export_data`. The owner's browser session holds it; `read`, `write` and `publish` do not include it; a delegated credential can carry it for chosen projects. Transfers created by an export need `export_data` to read, because every credential of one owner shares the transfer principal. `project_export` keeps its format for readers of the small package and needs `export_data` too, as does `database_export`, which hands out the raw database file.

## Considered options

- **Raise the JSON package's cap.** Rejected because base64 in one JSON value grows the document by a third and both ends still hold it whole.
- **Export the Deployment folder as a Copy.** Rejected because a Copy carries sessions, backups and Usage, and restores an installation, not one app into a project that already exists on the target.
- **One transfer for the whole package.** Rejected because each component has its own quota and checks, and a retry of one component should not resend the others.
- **A table of imports with its own receipts.** Rejected because release plans already carry a receipt and an applied plan already answers a repeat.

## Consequences

The kit carries the package under `littlebox/package/`, and its `deploy.mjs` uploads it in 64 KiB blocks, resuming from the Kernel's offset. A 256 MiB database at its write cap and 20 MiB of files exported and imported with each process at or under 120 MiB of resident memory (`tests/smoke/package-quota.ts`). Moving it takes about 4,400 MCP calls each way.

`transfer_complete` hashes uploads in 64 KiB reads instead of reading the file whole. Package transfers write one audit row at their first block rather than one per block.

The editor's own export still builds the small format in the browser. The OAuth consent screen does not offer `export_data`, so an OAuth client cannot download state; a delegated token or the owner session can.

## Note, 2026-11-06: migration packages

[ADR-0014](0014-an-app-migrates-with-one-writer-at-a-time.md) adds a version 2 manifest: version 1 plus a `migration` stamp that `migration_begin` writes. The Kernel and `src/lib/portable-package.ts` accept both versions, require the stamp on version 2 and refuse it on version 1. A version 2 import starts paused until `migration_activate`. A `release_plan` with a package the project already imported now returns the first plan and its receipt instead of the refusal for a project that has a Deployment; any other package for that project is still refused.

## Note, 2026-11-06: API keys travel revoked

The identity component used to delete the `api_keys` table's rows with the sessions and links. The small format had carried keys and revoked them at import since ADR-0011's update, and CONTEXT.md and `docs/mcp.md` said both formats did, so a package import left the destination's owner with `{ keys: [] }` and no way to tell which programs had access at the origin (the club showcase app found it).

A package now carries each key's name, actions, file visibilities, creation time and hash. The capture marks every live key revoked with `revoked_reason = 'import'` before the component is hashed, so the package opens nothing even on a Kernel that would not revoke it, and the install and `migration_activate` mark any live key again. Keys the owner revoked keep their time and no reason. `api_keys_list` returns `revokedReason`, and the editor lists those keys under "Revoked at import" with what each could call.

We considered leaving the hash behind, since a revoked key never matches. We kept it: the small format already carries it, it is the SHA-256 of a 256-bit random token, and the row stays the same shape in both formats. A restore of a Copy is unchanged and keeps keys live. `identity_copies_lose_sessions_and_links_and_carry_keys_revoked` in `package.rs`, `tests/package.integration.test.ts` and the club replay check it.
