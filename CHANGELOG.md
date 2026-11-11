# Changelog

All notable changes to Konae are recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

How a release is cut is in [Releasing](./docs/releasing.md).

## [Unreleased]

## [0.0.1] - 2026-11-11

### Added

- Interim `linux/amd64` images for an early deploy: `ghcr.io/danibram/konae:0.0.1` and `ghcr.io/danibram/konae-kernel:0.0.1`.
- Release workflow that builds `linux/amd64` only. Emulating arm64 under QEMU made the first tagged build too slow to wait on.

`0.1.0` is next: same images under that tag once this interim is out.
