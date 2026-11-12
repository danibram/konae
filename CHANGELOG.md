# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

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
