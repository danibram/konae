# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

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
