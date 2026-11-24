#!/bin/sh
set -eu

bundle=${1:?usage: build-functions-images.sh <runner-bundle-dir> <tag-prefix>}
prefix=${2:?usage: build-functions-images.sh <runner-bundle-dir> <tag-prefix>}
runner="$bundle/konae-functions-runner-wasm"
deno_runner="$bundle/konae-functions-runner-deno"
preparer="$bundle/konae-functions-prepare"
test -x "$runner" && test -x "$deno_runner" && test -x "$preparer" || { echo "A runner or preparation helper is not executable in $bundle" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || { echo "docker no está instalado" >&2; exit 1; }
command -v file >/dev/null 2>&1 || { echo "file no está instalado" >&2; exit 1; }

description=$(file -b "$runner")
deno_description=$(file -b "$deno_runner")
case "$description" in
  *"ARM aarch64"*) platform=linux/arm64; expected='ARM aarch64' ;;
  *"x86-64"*) platform=linux/amd64; expected='x86-64' ;;
  *) echo "arquitectura del runner no reconocida: $description" >&2; exit 1 ;;
esac
case "$deno_description" in
  *"$expected"*) ;;
  *) echo "deno runner architecture does not match the Wasm runner: $deno_description" >&2; exit 1 ;;
esac
runner_sha256=$(shasum -a 256 "$runner" | awk '{print $1}')
deno_runner_sha256=$(shasum -a 256 "$deno_runner" | awk '{print $1}')
preparer_sha256=$(shasum -a 256 "$preparer" | awk '{print $1}')

docker buildx build --load --platform "$platform" --build-context "functions-runner=$bundle" \
  --build-arg "FUNCTIONS_RUNNER_SHA256=$runner_sha256" \
  --build-arg "FUNCTIONS_DENO_RUNNER_SHA256=$deno_runner_sha256" \
  --build-arg "FUNCTIONS_PREPARER_SHA256=$preparer_sha256" \
  -f Dockerfile.kernel -t "$prefix-kernel" .
docker buildx build --load --platform "$platform" --build-context "functions-runner=$bundle" \
  --build-arg "FUNCTIONS_RUNNER_SHA256=$runner_sha256" \
  --build-arg "FUNCTIONS_DENO_RUNNER_SHA256=$deno_runner_sha256" \
  --build-arg "FUNCTIONS_PREPARER_SHA256=$preparer_sha256" \
  -f Dockerfile.konae -t "$prefix-konae" .
