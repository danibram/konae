# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

## [0.6.0] - 2026-11-13

The first release. It publishes two images for `linux/amd64`, `ghcr.io/danibram/konae:0.6.0` and `ghcr.io/danibram/konae-kernel:0.6.0`.

### Added

#### Release and install

- A version tag builds both images and opens the GitHub Release from this file. `bun run release:local` builds and pushes the same images from your machine.
- The `konae` image serves the editor on the editor host, such as `app.<domain>`, next to the owner's MCP and the apps. `KONAE_EDITOR_DIR` names the built editor, and an empty value turns it off.
- `KONAE_S3_BUCKET` names one private bucket for the installation, with `KONAE_S3_ENDPOINT` and the `KONAE_S3_*` key pair. File bytes go under `files/`, and with `KONAE_COPY_S3=1` copies and the revocation record go under `copies/`. Without a bucket, file bytes stay in the data directory. The Compose stack creates that bucket as `konae`.
- `files-migrate --to s3|local` moves file bytes between the data directory and the bucket, reads each one back by SHA-256 and switches the store only when all pass.

#### Addresses

- A first publish takes `<slug>.<host-suffix>` when that hostname is free. It takes `<slug>-<code>` when another Deployment holds the slug or the name is one of the installation's own hosts.
- `hostname_set` moves a Deployment to another generated hostname under the installation's suffix. The old name answers with a 308 redirect to the same path on the new one and stays reserved for the app, so email links and bookmarks keep working.

#### Owner sign-in and agents

- The editor and the server share one owner page, `/_konae/owner/login`, drawn with the design system in light and dark. It separates sign-in from creating an account, lists the commands that connect an agent, limits a token to chosen projects, and asks before it replaces or revokes one. A reset or confirmation link opens there.
- The OAuth consent page at `/oauth/authorize` names the client as unverified and says where the token goes, what each permission allows, for how long and what it cannot do. A browser that opens a consent link Konae cannot honour gets a page with the reason and a way back, and agents and scripts get JSON. Consent completes in Cursor and in Chrome.
- A project's More menu opens the owner account.
- `GET /_konae/owner/config` answers `editor`, `mailToFile` and, on a local installation that runs with the development key, `defaultSetupKey`.

#### Editor

- The editor opens a published app that has no editor files, such as one published from a folder or over MCP, on its database, its app accounts and what it did. **Open data and users** is on its card under Apps without editor files.
- The new-project dialog selects Private notes and labels every template Public or Private.
- The design system gallery and its docs show `BarList`, the bar chart Usage uses.

#### Files

- `manifest.fileAccess: "app-users"` shares every uploaded file with the signed-in users of that app. API keys cannot read those files, and the uploader still deletes. A path address treats such an app as an app with accounts.
- `miniapp.files.upload` sends the file in one `PUT /_konae/transfers/<id>` and returns the file's path as `url`. A refused upload fails with the Kernel's words and cancels its transfer. `miniapp.files.list` returns each file's path as `url` and reads no file bytes.
- `GET /_konae/files/<id>` answers with an ETag and `Cache-Control: private, no-cache`. A browser that holds the file gets 304 after the Kernel checks the caller, with no read from the store.
- `konae dev` allows 1 TiB per uploaded file and per app's uploaded files. `KONAE_FILE_MAX_BYTES` and `KONAE_FILES_QUOTA_BYTES` set lower limits.

#### Usage

- Usage shows each published app's storage by database, uploaded files, app accounts, Usage history, published pages and sources, with limits where they apply. Backups appear apart.
- Usage counts daily activity per signed-in app account, app opens and successful actions included. Anonymous visitors, and sessions where an owner entered as an app user, add no per-account activity. An export carries this history.
- Usage compares operations and storage across apps with bar charts. `usage_summary` and `usage_overview` give the account and storage figures over MCP.

#### CLI

- A folder fresh from `konae init` builds, runs under `konae dev` and publishes with no `npm install`. Once the folder has `node_modules`, `build`, `dev` and `publish` run its `npm run build`. The built-in bundler accepts `src/main.tsx` as the entry.
- `konae init`, the editor's Export and the Kernel write the same Kernel version into `konae.json`, the one in `package.json` and the Cargo workspace.

#### Examples

- `examples/home` is the page an installation answers on its bare domain, in English and Spanish.
- `examples/invitation` is a public app where a visitor asks for an invitation code. It refuses a repeated address and emails the owner for each request.
- `examples/politica-de-privacidad` and `examples/terminos-y-condiciones` are the installation's legal pages.
- `examples/docs-site` publishes the docs and draws their diagrams.

### Security

- An import builds the API keys table from the Kernel's own definition and a fixed column list. The Kernel rejects triggers and views in every imported SQLite component.
- A Deployment never takes the installation's editor or MCP host, by a first publish, a Deployment record, a custom domain or a hostname it held before.
- Queries, owner SQL, table browsing and notice rows stop at the 2 MiB result budget as the Kernel reads each column. An oversized notice result rolls back its mutation.
- SQLite has one memory ceiling for the process, `KONAE_SQLITE_HEAP_BYTES`. It defaults to half the available memory, Linux cgroup limits included, or 256 MiB when the memory size is unknown.
- Each Deployment runs at most four visitor action statements at once. A call that waits two seconds without a slot answers `rate-limited` with `Retry-After`.
- Password verification accepts only the 600,000-iteration hashes the Kernel writes and bounds their size before decoding. An import rejects any other hash.
- With `KONAE_COPY_SECRET` set, S3 copy manifests and revocation entries carry signatures. Restore refuses an unencrypted S3 copy and a manifest whose signature is missing or wrong, and an unsigned or altered revocation entry keeps the restored installation in maintenance.
- Template apps escape both quote characters in text and attributes. The build agent's instructions and an app folder's `AGENTS.md` require escaping untrusted output.
