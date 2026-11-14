# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

## [0.8.0] - 2026-11-14

This release carries the Kamynando rollout fixes and the security fixes for import, restore and access revocation that were not in 0.7.0, and the three fixes from the review of 0.7.0.

### Added

- `konae login --browser` signs the owner in through the browser with OAuth and PKCE on a loopback port and saves a 24-hour credential without a password. OAuth clients that redirect to `127.0.0.1` or `localhost` get 24-hour tokens and may ask for `export_data`, and the consent page says how long the token lasts.
- `konae secret set <name> [--use route|call|google]` reads the value from stdin and reads the name back. `konae secret list` prints names and uses, never values.
- `konae push` replaces the published app's data with the folder's `data/` through `project_import`. It checks the installation's file limits before sending anything, runs the import in maintenance after a backup, and refuses without `--yes`.
- `release_plan` and `konae publish` name the generated hostname before a first release fixes it. `konae hostname` shows it or moves it, and the old name answers with a 308 to the new one.

### Changed

- Owner transfers over MCP move in 4 MiB blocks, so a release takes a few `transfer_write` calls instead of dozens. The proxy in front of `/mcp` must accept bodies of about 5.4 MiB.
- A release build minifies, from the `build.mjs` new app folders get, from `konae build`'s own bundler and from the editor's Deploy alike; `konae dev` and the editor preview build readable. The new `build.mjs` also type-checks `app/src/` against `actions.d.ts`. `src/main.tsx` is an entry in every build.
- With `KONAE_MAIL_TRANSPORT=development`, invitations answer `delivery: "local-mailbox"` and their `acceptUrl` once. A Kernel serving over https with that transport warns at startup.
- The `rules` topic no longer asks for `transfer_status` after each block or for a second `release_plan`.

### Fixed

- The editor's sentence for an action says it reads or changes only the caller's own rows only when the SQL proves it. A named parameter (`:name`, `@name`, `$name`), or a filter on a name the SELECT list defines, made it claim that for a query that reads other people's rows.
- Exporting a published app, from the editor or from Projects, no longer builds the editor's draft, so a draft that does not build no longer keeps the app's accounts and rows from leaving.
- After a first deploy, inviting yourself no longer says the app opens when the Signup Policy refuses your address. The receipt names the rule, and on a development transport it hands over the link.
- The owner MCP says when a token expired and how to get a new one.
- A pending sign-in and the signup hint no longer mention an email the installation never sent, and a pending sign-in names a waiting invitation.
- The build and the Validation Gate refuse an app whose pages are all whole documents that load no script, since its entry would never run.
- A first publish holds its generated name against a concurrent move and never takes the installation's own host or the path host. A call from another app to a moved app's old name still reaches it.
- Saving a project's manifest takes up to 100 actions, as the Validation Gate and the Kernel do.
- `KONAE_URL` is documented as read by `konae login` only. Every other remote command talks to `--url` or the installation `login` saved.

### Security

- An import refuses virtual tables and a password hash column hidden under another case or as a generated column, and accepts a table row only when it reads as `CREATE TABLE` with a b-tree.
- An import replaces the incoming API key table with the Kernel's own, and revoking access matches table and column names in any case.
- A restore signs out sessions held in tables spelled in capitals.
- With `KONAE_COPY_SECRET` set, a restore from S3 without `--copy` takes the newest copy that is encrypted and signed with the secret, prints each copy it skips and why, and stops after 50 skipped copies or at the first copy it cannot read, so a copy planted in the bucket neither restores nor hides the genuine ones. A manifest over 4 MiB is refused.

## [0.7.0] - 2026-11-14

An installation that sets `KONAE_ADMIN_KEY` or a `KONAE_MAIL_LIMIT_*CREATOR*` name refuses to start on this version until it uses the new name with the same value.

### Changed

- The operator commands of `konae` that name an owner say `owner`. `owner-code-create`, `owner-code-list`, `owner-code-revoke`, `owner-account-suspend` and `owner-account-resume` were `creator-code-create`, `creator-code-list`, `creator-code-revoke`, `creator-account-suspend` and `creator-account-resume`, and `owner-suspend` was `creator-suspend`. An old name answers `creator-code-create is now owner-code-create.` and exits with an error.
- The server reads its setup key from `KONAE_SETUP_KEY`, the name `konae login` already read. `konae` refuses to start while `KONAE_ADMIN_KEY` is set and `KONAE_SETUP_KEY` is not.
- `KONAE_MAIL_LIMIT_OWNER_PER_DAY` and `KONAE_MAIL_LIMIT_NOTIFY_OWNER_PER_DAY` were `KONAE_MAIL_LIMIT_CREATOR_PER_DAY` and `KONAE_MAIL_LIMIT_NOTIFY_CREATOR_PER_DAY`. An installation that sets an old name is refused at startup with the new one.
- The messages of those commands, the owner admission refusals and the owner sign-in say "owner" where they said "creator". The docs section "Creator admission" is "Owner admission", and `docs/clerk-creator-access.md` is `docs/clerk-owner-access.md`.
- `konae dev` limits a file to 2 MiB as a Kernel does, and `KONAE_FILE_MAX_BYTES` raises it. The quota for all files stays at 1 TiB so `--reset` can import a published app's files.
- `--url` in the CLI's help is the installation URL, the editor's origin.
- The validation message for the Signup Policy in `konae.json` names `signupPolicy.mode`, as the file does.
- The reference of platform rules moves from `mcp.md` to `docs/reference/`, one page per topic. `/mcp-guide` answers with the list of those pages, so an old link lands on the list rather than on its section.
- A guide, Add a second owner, and the first-run pages name which binary each command belongs to, what the first `dev` compiles, how to get a model key, and the `konae tool` form of every owner tool the guides name.

### Added

#### Release and install

- A release attaches the `kernel` binary for macOS and Linux on arm64 and x64, with `SHA256SUMS`, and publishes `konae` and `konae-kernel` for `linux/amd64` and `linux/arm64`. `KONAE_KERNEL_BIN` runs a downloaded binary in `konae dev`, so `dev` needs no Rust.
- One `docker run` of the `konae` image starts an installation with the editor, with `KONAE_SETUP_KEY` and `KONAE_PUBLIC_ORIGIN` as its only settings. The README shows the line.

#### CLI

- `konae dev` invites `test@example.com` when the app's Signup Policy is not Open and the folder holds no account yet, and prints the invitation link ready to open.
- `konae login` asks for the email and the password on a terminal when `KONAE_EMAIL` or `KONAE_PASSWORD` is unset, with no echo for the password. Without a terminal it refuses as before.
- `konae domain` takes the Deployment's ID from the `konae.json` of `--dir`.
- `init` and `export` write `.env.example` with `KONAE_KERNEL_IMAGE` set to the Kernel version in `konae.json`, so a container needs only `cp .env.example .env` and `docker compose up -d`.
- A server command typed into the CLI, such as `owner-code-create`, answers where the server binary is instead of `Unknown command`.

#### Editor and owner page

- The Deploy dialog says when the editor holds other files than the published app runs, after edits in the editor or a `konae publish` from an exported folder, and that Deploy replaces them.
- The owner page issues a token of 1, 7 or 30 days.

#### Kernel

- A port already in use is refused with its number and how to pick another.

### Fixed

- After the first deploy of a private app with no accounts, the receipt says the owner has no account there and invites the owner's email in one click.
- An action's sentence in the editor and in Deploy states only what its SQL shows: a total returns one number, and a filter bound to the actor reads their own rows.
- Go to a refused statement leaves the caret at its end, so typing a LIMIT adds to the query.
- Back to Projects focuses the card of the app you left, and Projects exports a published app with its accounts and rows.

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
