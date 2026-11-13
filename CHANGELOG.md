# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

## [0.5.0] - 2026-11-13

### Added

- `KONAE_S3_BUCKET` names one private bucket for the installation. File bytes go under `files/`, and with `KONAE_COPY_S3=1` copies and the revocation record go under `copies/`, all with the `KONAE_S3_*` key pair. The older `KONAE_FILES_S3_*`, `KONAE_DATA_BUCKET` and `KONAE_COPY_S3_PREFIX` settings keep working, and the Kernel refuses to start with both.
- `storage-migrate --from <settings file>` moves file bytes from the store an older settings file names to the store of the current environment, reads each one back by SHA-256, and switches `files-store.json` only when all pass. The settings file must have mode 0600.
  - `--dry-run` writes nothing and may run beside a serving Kernel. A real run needs the Kernel stopped.
  - It keeps the source, and a run that stopped or failed can run again. It removes the temporary files a killed run left in `transfers/`, and reports how many files the destination already held.
  - It refuses a destination holding files of an app this installation does not have, and a destination file with other bytes.
  - It does not move copies. When the settings file names S3 copies, the environment must send them to the same bucket, prefix and `KONAE_COPY_SECRET`, and the command opens every copy manifest and the revocation record with the new keys before it switches. When the settings file names none, it refuses a destination whose `copies/` already holds objects.
  - Run with the settings swapped, it moves the files back, including files written after the switch.
- `artifacts-recover --from <settings file>` writes into `dist/` any file of a live release that only the older artifact bucket holds. It compares every app before it writes, so a file that differs stops the run with nothing written. It skips folder markers, stops on an unreadable Deployment record, and writes nothing for an app that publishes while it runs.
- The design system gallery and its docs show `BarList`, the bar chart Usage uses.
- Usage shows each published app's current storage by database, uploaded files, app accounts, Usage history, published pages and sources, with limits where they apply. Backups appear separately.
- Usage counts daily activity per signed-in app account, including app opens and successful actions. Queued service calls count once, and anonymous visitors and sessions where an owner entered as an app user add no per-account activity. The app's export carries this history.
- Usage compares operations and storage across apps with bar charts, and shows daily active accounts and activity by account alongside tables. `usage_summary` and `usage_overview` expose the new account and storage figures over MCP.

### Changed

- `miniapp.files.upload` sends the file in one `PUT /_konae/transfers/<id>` instead of one MCP call per 64 KiB, so a 2 MiB photo takes four requests instead of about seventy. Against an older Kernel it still sends blocks.
- `miniapp.files.upload` returns the file's path, the one `files.url` gives, as `url`, instead of downloading the file again for a Blob URL.
- `miniapp.files.list` returns each file's path as `url`, the one `files.url` gives, and reads no file bytes. It used to download every listed file for a Blob URL.
- `GET /_konae/files/<id>` answers with an ETag and `Cache-Control: private, no-cache`. A browser that holds the file gets 304 after the Kernel checks the caller, with no read from the bucket.
- The Compose stack creates one bucket, `konae`, instead of `konae-apps` and `konae-data`. A volume from an earlier stack keeps its files in the data directory, so the Kernel refuses to start on it with the bucket; `docker compose down -v` starts with empty volumes.
- `konae dev` defaults to 1 TiB per uploaded file and per app's uploaded files for local testing. `KONAE_FILE_MAX_BYTES` and `KONAE_FILES_QUOTA_BYTES` can set lower limits; editor previews and published Deployments keep their own limits.
- The editor's new-project dialog selects Private notes by default and labels every template Public or Private. Choosing Shared list still creates a public app with anonymous writes.

### Removed

- The Kernel no longer copies Build Artifacts to `KONAE_ARTIFACT_BUCKET` or reads a missing `dist/` file from there, and the owner tool `storage_test` is gone. Every app is served from its `dist/`. On an installation that used the artifact bucket, run `artifacts-recover` before the first start of this version.

### Security

- When an import carries API keys forward, the Kernel uses its own table definitions and a fixed column list. The Kernel rejects triggers and views in every imported SQLite component, regardless of the letter case of their stored type.
- New deployments avoid the installation's editor and MCP hosts. Legacy deployment records, custom domain aliases and previous hostnames cannot take over those hosts or redirect owner requests to an app.
- Queries, owner SQL, table browsing and notice rows enforce the 2 MiB result budget as columns are read, including column names, JSON escapes and hexadecimal blobs. An oversized notice result rolls back its mutation.
- SQLite has a process-wide memory ceiling, set with `KONAE_SQLITE_HEAP_BYTES`. The default is half the available memory, including visible Linux cgroup limits, or 256 MiB when the memory size is unknown.
- Each Deployment runs at most four visitor action statements at once. A call that waits two seconds without a slot answers `rate-limited` with `Retry-After`. A slow SQLite function can still exceed the statement deadline within one call.
- Password verification accepts only the 600,000-iteration hashes the Kernel writes and bounds their encoded size before decoding. Imports reject unsupported hashes; malformed stored hashes run the same bounded check and return the same credential error as an unknown email.
- With `KONAE_COPY_SECRET` set, S3 copy manifests and revocation entries carry authentication signatures. Restore refuses unencrypted S3 copies and manifests with missing or invalid signatures. An unsigned or altered revocation entry keeps the restored installation in maintenance. Set `KONAE_COPY_ACCEPT_UNSIGNED=1` to restore, reconcile or migrate with older encrypted copies or entries without signatures. Entries a 0.4.2 Kernel wrote stay unsigned in the revocation record, so every later restore from that record needs the setting. Invalid signatures still fail, and directory copies remain unsigned.
- New template apps escape both quote characters in text and attributes, including row IDs, member roles and file URLs. The build agent and exported app instructions require escaping untrusted output. Existing app files keep their own code.

## [0.4.2] - 2026-11-13

### Changed

- The editor and the server share one owner page, `/_konae/owner/login`. The editor links there with the page the owner came from, and the owner page returns there after sign-in or offers "Back to your project". `/platform-access` keeps only the Clerk sign-in, and sends a signed-in owner on to the owner page.
- A project's More menu opens the owner account, so an owner no longer has to leave the project to reach it.
- `GET /_konae/owner/config` answers `defaultSetupKey` on a local installation that still runs with the development key, so the owner page fills it in as the editor did.

### Fixed

- The project's More menu was dark grey on dark under the dark theme.
- An owner who created an account from a link to another owner's project landed on "Project not found". A new account lands on the project list.

## [0.4.1] - 2026-11-13

### Fixed

- An owner's reset link opened the installation's root, where the editor reads no link and an installation without the editor answers 404. It opens `/_konae/owner/login` now, as a confirmation link already did.

## [0.4.0] - 2026-11-13

### Added

- `GET /_konae/owner/config` answers `editor` and `mailToFile`, so the owner page links to the project list only where the editor runs and says when a link was written to the mailbox file instead of sent.
- A browser that opens a consent link Konae cannot honour, such as one whose client registration expired, gets a page with the reason and a way back. Agents and scripts keep the JSON answer.

### Changed

- The owner sign-in at `/_konae/owner/login` and the OAuth consent at `/oauth/authorize` follow the design system in light and dark. The binary serves their typefaces, so an installation without the editor shows them the same way.
- The consent page names the client as unverified, says where the token goes, what each permission allows, for how long and what it cannot do, and keeps Allow and Cancel in view on a phone.
- The owner page separates sign-in from creating an account and lists the commands to connect an agent. A token can be limited to chosen projects, an uncopied token is replaced only after a confirmation, and revoking asks first and names the token it revoked.

### Fixed

- A consent request for an unknown client answered `Query returned no rows`. It now says the client must register again.
- A reset or confirmation link pasted into an open sign-in tab opens the set-password form.

## [0.3.0] - 2026-11-13

### Added

- The editor opens a published app that has no editor files, such as one published from a folder or over MCP, on its database, its app accounts and what it did. **Open data and users** is on its card under Apps without editor files, and `/app/<id>` names the page. Its files, actions and visibility still change in the folder.
- `examples/invitation` is a public app where a visitor asks for an invitation code. It refuses a repeated address and emails the owner for each request, and the home links to it at `<domain>/p/invitation/`.
- The home is in English and Spanish, says what every app has, names the installation's own domain in its animation and links the privacy policy and terms where those pages answer.
- `bun run release:local` builds and pushes the install images from your machine.

### Fixed

- Under the dark theme the data browser's cells, table list and buttons kept light surfaces with light text.

## [0.2.0] - 2026-11-13

### Added

- The `konae` image serves the editor on the editor host, such as `app.<domain>`, next to the owner's MCP and the apps. `KONAE_EDITOR_DIR` names the built editor, and an empty value turns it off.
- `hostname_set` moves a Deployment to another generated hostname under the installation's suffix, such as `kamynando2.konae.es` to `kamynando.konae.es`. The old name answers with a 308 redirect to the same path on the new one and stays reserved for the app, so email links and bookmarks keep working.

## [0.1.6] - 2026-11-12

### Fixed

- The app runtime MCP door accepts generated hostnames under `KONAE_HOST_SUFFIX` (for example `kamynando2.konae.es` when the editor is on `app.konae.es`). Without that, every published app got `Cross-origin or unrecognized-host MCP request rejected.` on `/_konae/mcp` after sign-in.
- `konae build` and `konae publish` run the folder's `npm run build` when `package.json` names one, and accept `src/main.tsx` as the entry for the built-in bundler.

## [0.1.5] - 2026-11-12

### Changed

- A first publish takes `<slug>.<host-suffix>` when that hostname is free, the same rule a bare Kernel folder already used. The random `<slug>-<code>` form remains only when another Deployment already holds the slug.

## [0.1.4] - 2026-11-12

### Fixed

- MCP OAuth consent allows Cursor's registered callback at `www.cursor.com` to redirect to `cursor.com` after approval in Chromium. The consent page omits CSP `form-action`; Chromium applies that directive to the whole post-Allow redirect chain and blocked the hop even when both origins were listed.

## [0.1.3] - 2026-11-12

### Added

- `manifest.fileAccess: "app-users"` shares every uploaded file with signed-in users of that app, including earlier uploads, without rewriting the index. Lists, `GET /_konae/files/<id>` and visibility changes follow the policy; API keys cannot read those files; the uploader still deletes. The Validation Gate, editor Users panel, preview host and SDK refuse any other visibility while the policy is on. Removing it restores each file's stored visibility.
- Path addresses treat `fileAccess: "app-users"` as an app with accounts, so `path_set` needs `acceptSharedOrigin` the same way as private apps or non-anonymous actions.
- Example apps `examples/politica-de-privacidad` and `examples/terminos-y-condiciones` for the installation's legal pages at `/p/politica-de-privacidad/` and `/p/terminos-y-condiciones/`.

## [0.1.2] - 2026-11-12

### Fixed

- OAuth consent Allow completes in Chrome/Cursor: CSP `form-action` also names the validated client `redirect_uri` origin, so the post-consent 303 is not blocked. Chrome's console still blames the authorize URL, which made the earlier public-origin-only fix look sufficient.

## [0.1.1] - 2026-11-12

### Fixed

- OAuth consent Allow works in Cursor's OAuth webview: the form posts to the public origin and CSP `form-action` names that origin, so `form-action 'self'` alone no longer blocks the POST.

## [0.1.0] - 2026-11-11

### Added

- First published release images: `ghcr.io/danibram/konae:0.1.0` and `ghcr.io/danibram/konae-kernel:0.1.0` for `linux/amd64`.
- Release workflow and Changelog process. Builds `linux/amd64` only; arm64 under QEMU was too slow for a tagged release.

## [0.0.1] - 2026-11-11

### Added

- Interim `linux/amd64` images used to deploy before this release: `ghcr.io/danibram/konae:0.0.1` and `ghcr.io/danibram/konae-kernel:0.0.1`.
