#!/bin/sh
set -eu

bundle=${1:?usage: build-functions-images.sh <runner-bundle-dir> <tag-prefix>}
prefix=${2:?usage: build-functions-images.sh <runner-bundle-dir> <tag-prefix>}
runner="$bundle/konae-functions-spike"
test -x "$runner" || { echo "runner no ejecutable: $runner" >&2; exit 1; }
command -v docker >/dev/null 2>&1 || { echo "docker no está instalado" >&2; exit 1; }
command -v file >/dev/null 2>&1 || { echo "file no está instalado" >&2; exit 1; }

description=$(file -b "$runner")
case "$description" in
  *"ARM aarch64"*) platform=linux/arm64 ;;
  *"x86-64"*) platform=linux/amd64 ;;
  *) echo "arquitectura del runner no reconocida: $description" >&2; exit 1 ;;
esac
runner_sha256=$(shasum -a 256 "$runner" | awk '{print $1}')

docker buildx build --load --platform "$platform" --build-context "functions-runner=$bundle" \
  --build-arg "FUNCTIONS_RUNNER_SHA256=$runner_sha256" \
  -f Dockerfile.kernel -t "$prefix-kernel" .
docker buildx build --load --platform "$platform" --build-context "functions-runner=$bundle" \
  --build-arg "FUNCTIONS_RUNNER_SHA256=$runner_sha256" \
  -f Dockerfile.konae -t "$prefix-konae" .
