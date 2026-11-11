# Releasing

A release is a git tag `vX.Y.Z` whose version matches `package.json` and the Cargo workspace, with a matching section in `CHANGELOG.md`. Pushing that tag builds the install images and opens a GitHub Release.

## What a release publishes

| Image | Dockerfile | Pull |
| --- | --- | --- |
| Konae (Kernel + owners, MCP, project list) | `Dockerfile.konae` | `ghcr.io/danibram/konae:X.Y.Z` |
| Kernel alone (exported app folders) | `Dockerfile.kernel` | `ghcr.io/danibram/konae-kernel:X.Y.Z` |

Each push also tags `X.Y` and `latest`. The published architecture is `linux/amd64`. The editor image in `Dockerfile` stays local to Compose and is not published.

After the first release, set each package's visibility to Public under the repository's Packages settings, or GHCR keeps them private to the account.

## Track changes

1. Write commits with a [Conventional Commits](https://www.conventionalcommits.org/) type in the subject: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, `perf:`. The type decides which Changelog heading the change belongs under. Breaking changes add `!` after the type or a `BREAKING CHANGE:` footer, and those force a major bump.
2. When the change is worth naming in a release, add a bullet under `## [Unreleased]` in `CHANGELOG.md` in the same pull request. Group under `Added`, `Changed`, `Fixed`, `Deprecated`, `Removed` or `Security`. Skip noise that only the commit history needs.
3. Keep `package.json` `version` and `kernel-rs/Cargo.toml` `[workspace.package] version` equal. Bump them only when cutting a release.

## Cut a release

1. Choose the next SemVer from the Unreleased bullets: major for a breaking change, minor for a feature, patch for a fix.
2. Move the Unreleased bullets into a new section:

   ```md
   ## [X.Y.Z] - YYYY-MM-DD

   ### Added
   - ...
   ```

   Leave an empty `## [Unreleased]` above it.
3. Set both version fields to `X.Y.Z`.
4. Commit on `main`:

   ```sh
   git add CHANGELOG.md package.json kernel-rs/Cargo.toml
   git commit -m "chore: release vX.Y.Z"
   ```

5. Tag and push:

   ```sh
   git tag -a "vX.Y.Z" -m "vX.Y.Z"
   git push origin main "vX.Y.Z"
   ```

6. The [Release](../.github/workflows/release.yml) workflow checks that the tag matches both version fields and that `CHANGELOG.md` has a `## [X.Y.Z]` section, builds both images, pushes them to GHCR, and creates the GitHub Release from that section.

## Use a published image

An installation that runs Konae:

```sh
docker pull ghcr.io/danibram/konae:X.Y.Z
```

An exported app folder's Compose file reads `KONAE_KERNEL_IMAGE` from `.env`:

```sh
KONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:X.Y.Z
```

[Operate an installation](./operate.md) and [Take the app elsewhere](./guides/take-the-app-elsewhere.md) cover the rest of the run.
