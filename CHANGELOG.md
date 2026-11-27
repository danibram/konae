# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

### Added

- Exported apps with server functions can start on a matching Kernel image under Docker's default security profile. The app folder carries the published bundles, accounts, data, and rules. The destination prepares the bundles in its own cache. [Server functions](./docs/functions.md)
- The `konae:actions` helper types each function's named Server Actions from its allowlist and Action Contract. Both runners support request-scoped timers and Web Crypto, and the builders include a documented subset of Node.js modules.
- Function timing includes individual Server Actions. `konae events` shows local function traces, and operators can send sampled traces to an OTLP/HTTP collector.
- App exports carry pending Calls and notice intents alongside their database rows. The destination can resume them after import. [Export and import](./docs/reference/export-and-import.md)

### Changed

- StarlingMonkey now serves server functions by default. Installations can select deno_core with `KONAE_FUNCTIONS_RUNNER_KIND=deno`.
- Server Actions and HTTP routes check app sessions on worker threads with read-only SQLite connections. HTTP routes use the same approach for API keys. Requests without a session token skip the session lookup.
- Usage groups counters for up to 2 ms before writing them in the background. A mutation that declares a Signal still waits for confirmation that the Signal was written.
- Mutations commit notice intentions with their row changes. Operators moving legacy Calls can rehearse the offline migration and rollback before upgrading the installation. [Queue migration](./docs/architecture.md#migrating-the-legacy-queues)

### Fixed

- Imports wait for runtime credential checks and Usage operations before replacing their SQLite files.
- The Kernel bundles SQLite 3.51.3, which fixes the WAL reset race.
- Static responses keep the body and ETag on the same revision while publishing or rolling back. Package import checks the shared blob inventory and app-wide file quota before installing the files.

## [0.12.1] - 2026-11-19

This release brings the Konae site into the repository: the home, the docs, `llms.txt` and the legal pages as one app.

### Changed

- The home, the docs, `llms.txt` and the privacy and terms pages are one app, `examples/site`, which answers konae.es as its custom domain: the docs at `/docs/`, `llms.txt` at the root and the legal pages at `/politica-de-privacidad` and `/terminos-y-condiciones`. `examples/home`, `examples/docs-site` and the two legal apps are gone. The operator docs describe an installation whose apps live on a domain of their own, as konae.app does beside konae.es.

## [0.12.0] - 2026-11-19

This release lets the build agent test its preview and edit JSON in one checked step, and gives the pages the Kernel answers on an app's address the design system.

### Added

- The build agent can run `preview_test` against its current scratch files. It fills fields, clicks buttons, checks text and calls named actions as a preview actor on copied test rows. Results identify the source revision, action inputs and outputs, handled failures and viewport overflow. The test discards its writes and does not replace checks on a Kernel.
- The agent's `edit_json` tool changes JSON Pointer paths together. It requires matching prior values for replacements and removals, and writes nothing when a change fails.

### Changed

- The pages the Kernel answers on an app's address (sign-in, password recovery, invitation, Google sign-in, entry, maintenance and not found) use the design system: its colors in light and dark, its two fonts and its card, field, button and alert styles. The sign-in page names the app above the form instead of saying "Sign in to this app". Its Google button carries Google's logo and sits above the form, apart from it. The Kernel serves the fonts at `/_konae/fonts/` on every app's address.

## [0.11.0] - 2026-11-18

This release adds per-app modules, shareable pages backed by app data, and a browser-local editor. It also adds staged Pi Durable conversations and a new optional installation backup format.

### Added

- App settings and the owner MCP can turn modules on and configure them per app. The available switches are search listing, app accounts, file storage, data and actions, HTTP routes and outbound calls.
- Public apps can opt in to indexing with the `seo` module. Konae generates canonical URLs, robots rules, sitemaps and social previews from the pages the app serves. The home and docs include page titles and share images.
- A page action can serve a URL such as `/posts/:slug` and use a row for its title and description. Apps can supply a bounded query that lists their published URLs for the sitemap.
- Owners can read and change the contact email per app. Settings writes can check the value the owner last read, so an older form cannot overwrite a later edit.
- Open `/local` to describe, check, preview and apply an app with a provider key saved in the browser. Signed-in owners can attach a copy of a local conversation to a saved Project.
- Builds with Pi Durable staging enabled let an owner read or download conversation history without a provider key, continue from another browser with its own key, and restore a checkpoint as a proposal. Apply runs a fresh Validation Gate, Build and Preview. History stays owner-only and outside app exports. The release image uses the legacy agent by default.
- Operators can inventory installation storage and rehearse restores in isolation. Copies capture a consistent point across SQLite databases, drafts, uploaded files and queued work, including the app's internal backups.
- Optional v3 copy destinations store encrypted, authenticated blocks for SQLite and files in a directory or S3. Existing destinations continue to write the compatible formats. Operators can rehearse a rollback while keeping both formats.
- `bun run dev:local` signs in a generated development owner and opens Projects on loopback. Set `KONAE_DEV_AUTO_LOGIN=0` to exercise manual sign-in.

### Changed

- On first startup after an upgrade, Konae records module settings for existing apps from their published release and existing app state. Private apps retain sign-in, and apps with uploaded files retain file storage and their published file access policy. Search listing starts off.
- With data and actions off, static apps can publish without `actions.json`, `schema.sql` or a database. An export with no database requires a Kernel that supports optional databases to import it.
- Owner MCP clients use `modules_read` and `module_set` with `module: "auth"` to manage signup policy and sign-in methods. This replaces `signup_policy_set`, `sign_in_methods_read` and `sign_in_methods_set`.
- HTTP routes and outbound calls require data and actions. Files API keys require file storage, and route API keys require HTTP routes and data and actions.
- Konae adds `noindex` to app pages by default and keeps sign-in and other system pages out of search.

### Fixed

- Installation copies verify indexed blobs and all referenced destination bytes before publishing. The writer checks the 4 MiB limit after signing the manifest. Health preserves the last complete point per target across restarts and reports its age and the last verified restore.
- Upgrades retain owner projects and app sessions from older database layouts.
- App settings keeps the selected tab when the dialog remounts.
- Concurrent initialization of the Usage database no longer interrupts a scheduled run with a SQLite lock error.

## [0.10.0] - 2026-11-17

### Changed

- Installation copies are incremental. A directory copy hard-links each file that did not change since the previous copy, so every copy folder stays complete while unchanged bytes take disk space once. An S3 copy uploads only changed files, and its version 2 manifest names the earlier copy that holds each unchanged object. Pruning keeps every object a retained manifest names. A copy skips reading a file blob from the file store when its size and modification time match the previous copy. Restore reads version 1 and version 2 copies. An installation that holds version 2 copies needs 0.10.0 or later to restore and prune them.
- The installation home is redesigned. The hero shows the editor building an example app, then the page covers how an app is made, what every app has, the move to another server, the example apps, the developer paths and the questions people ask before asking for an invitation. Layouts name their columns at each width, and the Spanish and English texts are written together.
- The agent loads SDK references by module instead of carrying the full reference in every prompt. Its workflow checks optional inputs through the form, action bindings and SQL, and asks for interaction tests before reporting a flow as verified. A schema transformation reference documents how to preserve values through a rename.
- App administrator changes resolve the exact email through `app_users_list`, use the stable account ID and read the role back after confirmation. Editable display names do not identify accounts, and the owner can request the change in chat without writing SQL.

### Fixed

- `preview_errors` builds and loads the current scratch files instead of reporting errors from the previously applied preview. Failed candidate checks reach the model and chat as failed tool results with their evidence.
- The Validation Gate and Kernel accept totals wrapped in `COALESCE` or `IFNULL` with constant fallbacks while continuing to require a LIMIT for row queries. They report multiple invalid action names together.
- Older identity databases gain the missing `app_sessions.entered_by` column without losing accounts or sessions. Older Konae stores gain the missing project source metadata so the editor can open saved projects.

## [0.9.0] - 2026-11-16

### Added

- `konae dev` runs in Kernel test mode on loopback. It captures outbound calls, keeps email in a separate local mailbox, uses local files, pauses schedules, and refuses Google sign-in and external browser connections.
- `konae test status`, `calls`, `reply`, `schedule` and `scenario` inspect and control local test runs. The events page exposes those controls through an owner token. Captured calls retain retry timing and response numbers, and interrupted continuations do not replay.
- `konae pull --data-only` refreshes the published data snapshot while keeping local sources. `dev --reset` verifies the snapshot components, backs up local data and queues, and restores them if the reset fails. Reset and test controls cannot mutate local state at the same time.
- `konae rehearse` builds the current sources and checks them against an isolated copy of the verified pull snapshot. `konae publish --plan` reports the production schema plan without applying it or creating a remote project.
- Optional `app/schema-transforms.json` records an append-only chain of bounded SQLite transitions. Planning reports affected rows; applying checks fingerprints, actions and constraints in one transaction. The journal travels with the database through export, import and restore. `konae schema fingerprint` reads the source schema or local database without starting an app.
- Guides for [local rehearsal](./docs/guides/rehearse-locally.md) and [schema transformations](./docs/guides/schema-transformations.md) document the development and publication workflow.

### Fixed

- Schema confirmations include reported loss counts and transformation definitions and row counts, so changed production counts require a new confirmation.
- Test-mode restarts apply schema and transformation edits even when the built browser files have not changed. Publishing preserves transformed rows and their journal when it swaps the staged database.
- Local shutdown waits for an active reload before releasing its lifecycle lock and stops the final Kernel process.

### Changed

- The editor and folder workflow use the same editable `app/` files, including binary assets. Sources and Build Artifacts travel in separate uploads and use separate limits. Existing folder apps open in the editor without source metadata sidecars.
- `konae pull` downloads sources, `dist/`, settings and published data. It stages incoming sources when local edits differ, scopes observed revisions to one installation and project, and blocks `push` until `dev --reset` imports pulled data.
- Publishing checks the observed source revision at planning and apply. A source save after planning makes the release stale.
- The project list loads metadata without source payloads. Opening a project loads its files, and source edits send deltas when possible. A full-document departure cancels known reads, with unknown transfers expiring after one hour if the browser cannot complete cancellation.
- The editor preview shows proposals before Apply and isolates candidate rows, files and actor state. Static saved public assets work in Preview; fetches of local saved assets and runtime-created local asset URLs require `konae dev` or a Deployment. Fetches to other origins still work in the CSP-free preview.
- The compiled `konae` CLI uses the shared local builder and downloads a matching Kernel after checking its checksum. Only macOS arm64 has been verified; its CLI binary requires macOS 13.0.
- Owner MCP requests for Deployment operations now require `projectId` instead of `deploymentId`. The `signup_policy_read` and `usage_read` tools are removed; `project_inspect` returns Signup Policy, and `usage_summary` returns daily Usage under its `usage` field. Successful `release_apply` consumes its release uploads and retries receipt cleanup after a fault. Runtime tools and the SDK remain unchanged.


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
