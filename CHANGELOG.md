# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

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
