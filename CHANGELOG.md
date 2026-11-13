# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

### Added

- `konae login --browser` signs the owner in through the browser with OAuth and PKCE on a loopback port and saves a 24-hour credential without a password. OAuth clients that redirect to `127.0.0.1` or `localhost` get 24-hour tokens and may ask for `export_data`.
- `konae secret set <name> [--use route|call|google]` reads the value from stdin and reads the name back. `konae secret list` prints names and uses, never values.
- `konae push` replaces the published app's data with the folder's `data/` through `project_import`. It checks the installation's file limits before sending anything, runs the import in maintenance after a backup, and refuses without `--yes`. [Bring existing data](./docs/guides/bring-existing-data.md) explains how to write a `data/` from another source.
- `release_plan` and `konae publish` name the generated hostname before a first release fixes it. `konae hostname` shows it or moves it, and the old name answers with a 308 to the new one.

### Changed

- Owner transfers over MCP move in 4 MiB blocks, so a release takes a few `transfer_write` calls instead of dozens. The proxy in front of `/mcp` must accept bodies of about 5.4 MiB. CLIs from 0.3.0 and earlier refuse download blocks over 64 KiB, so `export` and `pull` against an upgraded installation need this CLI.
- The `rules` topic no longer asks for `transfer_status` after each block or for a second `release_plan`, and says that "Transfer not found or expired." after `transfer_cancel` means the cancel worked. The `publishing` topic starts with `konae publish` and gives the full release payload.
- New app folders get a `build.mjs` that minifies with `NODE_ENV` set to production (`--dev` for `konae dev`) and type-checks `app/src/` against `actions.d.ts` with a pinned TypeScript. `src/main.tsx` is an entry in every build.
- With `KONAE_MAIL_TRANSPORT=development`, invitations answer `delivery: "local-mailbox"` and their `acceptUrl` once. A Kernel serving over https with that transport warns at startup, and health and `mail_status` report it.

### Fixed

- The owner MCP says when a token expired and how to get a new one, instead of the same answer as no token.
- `konae build`, `dev` and `publish` run `npm install` once in a folder fresh from `init` or `export`, whose build needs esbuild, and show npm's output only when it fails, so `publish` prints just the address.
- A pending sign-in no longer says "the link we sent" when the installation sends no email, and names a waiting invitation.
- The build and the Validation Gate refuse an app whose pages are all whole documents that load no script, since its entry would never run.
- A first publish holds its generated name against a concurrent move and never takes the installation's own host. A call from another app to a moved app's old name still reaches it. The suffix refusal from `domain_set` points at `hostname_set`.
- The docs say that published apps refuse WebAssembly and `eval`, and explain `manifest.fileAccess: "app-users"`.

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
