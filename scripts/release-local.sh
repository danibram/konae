#!/usr/bin/env bash
# Builds the two install images on this machine and pushes them to GHCR, the
# same images, tags and labels as .github/workflows/release.yml.
# See "Release from your machine" in docs/releasing.md.
#
# Runs with macOS /bin/bash 3.2: no mapfile, no associative arrays.
set -euo pipefail

REGISTRY="${KONAE_RELEASE_REGISTRY:-ghcr.io}"
OWNER="${KONAE_RELEASE_OWNER:-danibram}"
SOURCE_URL="https://github.com/danibram/konae"
PLATFORM="linux/amd64"

usage() {
  cat <<'EOF'
Usage: scripts/release-local.sh [--dry-run] [--no-push] [--overwrite] [--any-host]

Builds ghcr.io/danibram/konae and ghcr.io/danibram/konae-kernel for linux/amd64
from a clean checkout of HEAD, tags them X.Y.Z, X.Y and latest, pushes them
and prints their digests. X.Y.Z comes from package.json. The workflow on a
version tag publishes linux/amd64 and linux/arm64; a push from here replaces
the tag with an amd64-only image, so prefer the workflow for a release.

  --dry-run    Run the local checks, then print every build and push command
               without running it. Touches no registry and builds nothing.
  --no-push    Build and load both images into local Docker. No GHCR login needed.
  --overwrite  Push even when X.Y.Z is already on GHCR.
  --any-host   Allow a Linux workstation. The script refuses Linux otherwise,
               so it never builds on the server that runs Konae.
EOF
}

DRY_RUN=0
PUSH=1
OVERWRITE=0
ANY_HOST=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    --no-push) PUSH=0 ;;
    --overwrite) OVERWRITE=1 ;;
    --any-host) ANY_HOST=1 ;;
    -h|--help) usage; exit 0 ;;
    --) ;;
    *) echo "Unknown option: $arg" >&2; usage >&2; exit 2 ;;
  esac
done

fail() {
  echo "release-local: $*" >&2
  exit 1
}

# Every build and push goes through run, so --dry-run prints exactly what a real run does.
run() {
  printf '+'
  printf ' %q' "$@"
  printf '\n'
  if [ "$DRY_RUN" -eq 0 ]; then
    "$@"
  fi
}

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# --- Host -------------------------------------------------------------------

if [ "$(uname -s)" != "Darwin" ] && [ "$ANY_HOST" -eq 0 ]; then
  fail "refused on $(uname -s). This script builds on your workstation and pushes to GHCR; the server only pulls. On a Linux workstation, pass --any-host."
fi

# --- Versions (same checks as the workflow) ---------------------------------

command -v jq >/dev/null 2>&1 || fail "jq is missing. Install it (brew install jq) and run again."

version="$(jq -r '.version' package.json)"
cargo_version="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p;}' kernel-rs/Cargo.toml | head -1)"
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "package.json version must look like X.Y.Z, got '${version}'." ;;
esac
if [ "$cargo_version" != "$version" ]; then
  fail "package.json says ${version} but kernel-rs/Cargo.toml [workspace.package] says ${cargo_version}. Set both to the release version."
fi
notes="$(awk -v ver="$version" '
  $0 ~ "^## \\[" ver "\\]" { grab=1; next }
  grab && $0 ~ /^## \[/ { exit }
  grab { print }
' CHANGELOG.md)"
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
  fail "CHANGELOG.md has no ## [${version}] section, or it is empty. Move the Unreleased bullets into it first."
fi
major="${version%%.*}"
rest="${version#*.}"
minor="${rest%%.*}"
major_minor="${major}.${minor}"

# --- Git --------------------------------------------------------------------

if [ -n "$(git status --porcelain)" ]; then
  fail "the working tree has uncommitted changes. The image is built from HEAD and labelled with its commit; commit or set the changes aside first."
fi
revision="$(git rev-parse HEAD)"
tag_note=""
if ! git rev-parse -q --verify "refs/tags/v${version}" >/dev/null; then
  tag_note="No local tag v${version} yet."
elif [ "$(git rev-parse "v${version}^{commit}")" != "$revision" ]; then
  tag_note="Tag v${version} points at another commit than HEAD."
fi

# --- Docker -----------------------------------------------------------------

# A dry run reports a failed Docker check and still prints the plan; a real run stops.
BLOCKED=0
docker_fail() {
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "release-local: a real run stops here: $*" >&2
    BLOCKED=1
  else
    fail "$*"
  fi
}

# Reads only which registries have credentials, never the credentials.
ghcr_logged_in() {
  local cfg helper
  cfg="${DOCKER_CONFIG:-$HOME/.docker}/config.json"
  [ -f "$cfg" ] || return 1
  helper="$(jq -r --arg r "$REGISTRY" '(.credHelpers // {})[$r] // .credsStore // empty' "$cfg")"
  if [ -n "$helper" ]; then
    command -v "docker-credential-${helper}" >/dev/null 2>&1 || return 1
    "docker-credential-${helper}" list 2>/dev/null \
      | jq -e --arg r "$REGISTRY" 'keys | any(. == $r or . == ("https://" + $r))' >/dev/null
    return
  fi
  jq -e --arg r "$REGISTRY" '(.auths // {}) | to_entries
    | any((.key == $r or .key == ("https://" + $r)) and ((.value.auth // "") != ""))' "$cfg" >/dev/null
}

check_docker() {
  local builder_info
  command -v docker >/dev/null 2>&1 || { docker_fail "docker is missing. Install Docker Desktop and start it."; return; }
  docker buildx version >/dev/null 2>&1 || { docker_fail "docker buildx is missing. Docker Desktop ships it; with Homebrew docker, run 'brew install docker-buildx' and link it as a CLI plugin."; return; }
  if ! docker info >/dev/null 2>&1; then
    docker_fail "Docker is not running. Start Docker Desktop and run again."
  elif ! builder_info="$(docker buildx inspect 2>&1)"; then
    docker_fail "docker buildx cannot reach a builder. buildx said: ${builder_info}"
  else
    case "$builder_info" in
      *"Error:"*) docker_fail "the current buildx builder is not reachable. Start Docker Desktop, or pick a builder with 'docker buildx use'." ;;
      *"$PLATFORM"*) ;;
      *) docker_fail "the current buildx builder does not list ${PLATFORM}. Turn on amd64 emulation in Docker Desktop, or pick a builder with 'docker buildx use'." ;;
    esac
  fi
  if [ "$PUSH" -eq 1 ] && ! ghcr_logged_in; then
    docker_fail "Docker has no login for ${REGISTRY}. Run 'docker login ${REGISTRY} -u <github-user>' with a GitHub token that has write:packages, or pass --no-push to build only."
  fi
}
check_docker

# --- Plan -------------------------------------------------------------------

echo "Konae ${version} (${major_minor}, latest) from ${revision}"
[ -n "$tag_note" ] && echo "$tag_note This script does not tag; see docs/releasing.md."
if [ "$PUSH" -eq 1 ]; then
  echo "Builds ${PLATFORM} on this machine and pushes to ${REGISTRY}/${OWNER}."
else
  echo "Builds ${PLATFORM} on this machine and loads into local Docker. Nothing is pushed."
fi
[ "$DRY_RUN" -eq 1 ] && echo "Dry run: the commands below are printed, not run."
echo

if [ "$PUSH" -eq 1 ] && [ "$OVERWRITE" -eq 0 ]; then
  present=""
  for name in konae konae-kernel; do
    ref="${REGISTRY}/${OWNER}/${name}:${version}"
    if [ "$DRY_RUN" -eq 1 ]; then
      printf '+ docker buildx imagetools inspect %s  (refuse if it exists)\n' "$ref"
    elif docker buildx imagetools inspect "$ref" >/dev/null 2>&1; then
      present="${present} ${ref}"
    fi
  done
  case "$present" in
    "") ;;
    *konae:*konae-kernel:*)
      fail "${version} is already on ${REGISTRY}:${present}. A server that already pulled ${version} keeps its copy when it pins the tag, so bump the version. To replace it anyway, pass --overwrite and pin the printed digest in your deploy." ;;
    *)
      fail "only${present} is on ${REGISTRY}, so an earlier run stopped halfway. Run again with --overwrite to push both images." ;;
  esac
fi

# Build from HEAD exported by git, as Actions builds from a fresh checkout.
# The context then holds no node_modules, kernel-rs/target or local env files.
if [ "$DRY_RUN" -eq 1 ]; then
  work="/tmp/konae-release.dry-run"
else
  work="$(mktemp -d "${TMPDIR:-/tmp}/konae-release.XXXXXX")"
  trap 'rm -rf "$work"' EXIT
fi
context="${work}/src"
run mkdir -p "$context"
run git archive --format=tar -o "${work}/src.tar" "$revision"
run tar -xf "${work}/src.tar" -C "$context"

if [ "$PUSH" -eq 1 ]; then
  output="--push"
else
  output="--load"
fi

build() {
  local name="$1" dockerfile="$2" description="$3"
  local image="${REGISTRY}/${OWNER}/${name}"
  run docker buildx build \
    --platform "$PLATFORM" \
    --file "${context}/${dockerfile}" \
    --tag "${image}:${version}" \
    --tag "${image}:${major_minor}" \
    --tag "${image}:latest" \
    --label "org.opencontainers.image.title=${name}" \
    --label "org.opencontainers.image.description=${description}" \
    --label "org.opencontainers.image.source=${SOURCE_URL}" \
    --label "org.opencontainers.image.version=${version}" \
    --label "org.opencontainers.image.revision=${revision}" \
    --metadata-file "${work}/${name}.json" \
    "$output" \
    "$context"
}

build konae Dockerfile.konae "Konae: Kernel with owner accounts, MCP and the project list"
build konae-kernel Dockerfile.kernel "Kernel alone: serves app folders without Konae"

# --- Result -----------------------------------------------------------------

# Reads the digest buildx wrote to the metadata file. Prints nothing when the key is absent.
digest_from_metadata() {
  jq -r "$1 // empty" "$2"
}

digest_of() {
  local digest
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "sha256:<digest>"
    return
  fi
  if [ "$PUSH" -eq 1 ]; then
    digest="$(digest_from_metadata '."containerimage.digest"' "${work}/$1.json")"
  else
    digest="$(digest_from_metadata '."containerimage.config.digest" // ."containerimage.digest"' "${work}/$1.json")"
  fi
  case "$digest" in
    sha256:*) echo "$digest" ;;
    *) echo "" ;;
  esac
}
konae_digest="$(digest_of konae)"
kernel_digest="$(digest_of konae-kernel)"
konae_ref="${REGISTRY}/${OWNER}/konae:${version}"
kernel_ref="${REGISTRY}/${OWNER}/konae-kernel:${version}"

echo
if [ "$PUSH" -eq 0 ]; then
  echo "Loaded into local Docker (config digests, not registry digests):"
  echo "  ${konae_ref}  ${konae_digest:-digest unknown}"
  echo "  ${kernel_ref}  ${kernel_digest:-digest unknown}"
  echo "Nothing was pushed. Run again without --no-push to publish."
else
  if [ -z "$konae_digest" ] || [ -z "$kernel_digest" ]; then
    fail "pushed ${konae_ref} and ${kernel_ref}, but buildx wrote no digest to its metadata file. Read them with 'docker buildx imagetools inspect ${konae_ref}' before you change your deploy."
  fi
  konae_pin="${REGISTRY}/${OWNER}/konae@${konae_digest}"
  kernel_pin="${REGISTRY}/${OWNER}/konae-kernel@${kernel_digest}"
  if [ "$DRY_RUN" -eq 1 ]; then
    echo "Would push (each also as :${major_minor} and :latest):"
  else
    echo "Pushed (each also as :${major_minor} and :latest):"
  fi
  echo "  ${konae_ref}  ${konae_digest}"
  echo "  ${kernel_ref}  ${kernel_digest}"
  echo
  echo "Before you run your deploy:"
  if [ "$OVERWRITE" -eq 1 ]; then
    echo "  ${version} may already be on the server. A deploy that names only the tag keeps the"
    echo "  old copy, because podman and NixOS oci-containers pull only a missing image."
    echo "  Set your deploy's Konae image to the digest:"
    echo "    ${konae_pin}"
  else
    echo "  Set your deploy's Konae image to ${konae_ref}"
    echo "  (NixOS: the container's image option, wherever your host config sets it)."
    echo "  To pin these exact bytes, set it to the digest instead:"
    echo "    ${konae_pin}"
  fi
  echo "  Exported app folders take ${kernel_ref} (or ${kernel_pin}) in KONAE_KERNEL_IMAGE."
  echo
  echo "This created no git tag and no GitHub Release. Pushing tag v${version} later makes"
  echo "Actions rebuild and move ${version}, ${major_minor} and latest to new digests."
fi

if [ "$BLOCKED" -eq 1 ]; then
  echo
  echo "release-local: a real run would stop at the Docker check reported above." >&2
  exit 1
fi
