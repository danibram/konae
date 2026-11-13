# Releasing

A release is a git tag `vX.Y.Z` whose version matches `package.json` and the Cargo workspace, with a matching section in `CHANGELOG.md`. Pushing that tag builds the install images and opens a GitHub Release.

## What a release publishes

| Image | Dockerfile | Pull |
| --- | --- | --- |
| Konae (Kernel + owners, MCP, project list, built editor) | `Dockerfile.konae` | `ghcr.io/danibram/konae:X.Y.Z` |
| Kernel alone (exported app folders) | `Dockerfile.kernel` | `ghcr.io/danibram/konae-kernel:X.Y.Z` |

Each push also tags `X.Y` and `latest`. The published architecture is `linux/amd64`. `Dockerfile.konae` runs `bun run build`, and the image serves the result on the editor host from `KONAE_EDITOR_DIR`, so one container answers the editor, the owner's MCP and the apps. `Dockerfile` builds the Vite development server that Compose runs, and no release publishes it.

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

## Release from your machine

Use this path when a deploy cannot wait for Actions, for example during a rollout when each fix has to reach the installation within minutes. The script builds the same two images with the same tags and labels as the Release workflow, so a local release and an Actions release are interchangeable.

1. Cut the release as above through step 4: Changelog section, both version fields, the commit. The script refuses a working tree with uncommitted changes, because it labels the images with the commit at `HEAD`.
2. The script needs `jq`. Log Docker in to GHCR once, with a GitHub token that has `write:packages`:

   ```sh
   docker login ghcr.io -u <github-user>
   ```

3. Check what it will run, then run it:

   ```sh
   bun run release:local -- --dry-run
   bun run release:local
   ```

The script checks that `package.json`, `kernel-rs/Cargo.toml` and a `## [X.Y.Z]` section of `CHANGELOG.md` agree, and that `docker buildx` can build `linux/amd64`, and that Docker holds a GHCR login. For the login it reads which registries have credentials, never the credentials themselves. It exports `HEAD` with `git archive` and builds from that copy, as Actions builds from a fresh checkout. Then it pushes `ghcr.io/danibram/konae` and `ghcr.io/danibram/konae-kernel` as `X.Y.Z`, `X.Y` and `latest`, and prints each pushed digest and the image line your deploy needs.

| Flag | Effect |
| --- | --- |
| `--dry-run` | Runs the local checks, then prints every build and push command without running it. A Docker check that fails is reported and the plan is still printed. |
| `--no-push` | Builds both images and loads them into local Docker. Needs no GHCR login. |
| `--overwrite` | Pushes even when `X.Y.Z` is already on GHCR. Without it the script refuses, and when only one of the two images is there it says an earlier run stopped halfway. |
| `--any-host` | Allows a host other than macOS, such as a Linux workstation. |

The build runs on your machine and only there. The server pulls the pushed image and never builds it, and never receives the repository or `kernel-rs/target`. The script runs only on macOS unless you pass `--any-host`, so it does not run on the server by mistake. `KONAE_RELEASE_REGISTRY` and `KONAE_RELEASE_OWNER` change the registry and the account it pushes to, `ghcr.io` and `danibram` by default.

Bump the version for every image you deploy. A deploy that names `ghcr.io/danibram/konae:X.Y.Z` starts a new image only when that tag is new to the server. Podman, and NixOS `oci-containers` by default, pull an image only when it is missing, and an unchanged image line does not restart the container. When you replace a version with `--overwrite`, set your deploy's image to the printed `ghcr.io/danibram/konae@sha256:...` instead. A new digest changes the image line, so the server pulls and restarts.

The script creates no git tag and no GitHub Release. Push the tag afterwards when you want the Release page, knowing that Actions then builds again and moves `X.Y.Z`, `X.Y` and `latest` to new digests. A deploy pinned by digest keeps running the local build for as long as GHCR keeps that untagged version.

### Disk for the amd64 build

On Apple Silicon, Docker Desktop builds `linux/amd64` under emulation inside its own virtual disk, not in the repository. The Rust stage compiles the dependencies and the release binary of `konae` or `kernel` there, and its layers stay in the BuildKit cache between builds. Check its size before a release and clear it when the disk is tight:

```sh
docker buildx du
docker buildx prune
```

Raise the disk limit under Docker Desktop's Settings, Resources, when a build fails with `no space left on device`. The `gha` cache the workflow uses has no local equivalent; the local cache is what makes the second build faster.

## Use a published image

An installation that runs Konae:

```sh
docker pull ghcr.io/danibram/konae:X.Y.Z
```

It answers the editor on the editor host, such as `app.<domain>`, beside the owner's MCP. An empty `KONAE_EDITOR_DIR` turns the editor off.

An exported app folder's Compose file reads `KONAE_KERNEL_IMAGE` from `.env`:

```sh
KONAE_KERNEL_IMAGE=ghcr.io/danibram/konae-kernel:X.Y.Z
```

[Operate an installation](./operate.md) and [Take the app elsewhere](./guides/take-the-app-elsewhere.md) cover the rest of the run.
