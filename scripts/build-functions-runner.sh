#!/bin/sh
set -eu

arch=${1:?usage: build-functions-runner.sh <amd64|arm64> <output-directory>}
output=${2:?usage: build-functions-runner.sh <amd64|arm64> <output-directory>}
case "$arch" in
  amd64) platform=linux/amd64; expected='x86-64' ;;
  arm64) platform=linux/arm64; expected='ARM aarch64' ;;
  *) echo "arquitectura no admitida: $arch (usa amd64 o arm64)" >&2; exit 2 ;;
esac
command -v docker >/dev/null 2>&1 || { echo 'docker no está instalado' >&2; exit 1; }
command -v file >/dev/null 2>&1 || { echo 'file no está instalado' >&2; exit 1; }

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
root=$(CDPATH= cd "$script_dir/.." && pwd)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/konae-functions-runner.XXXXXX")
trap 'rm -r -- "$scratch"' EXIT HUP INT TERM
docker buildx build --platform "$platform" --target artifact \
  --output "type=local,dest=$scratch" \
  --file "$root/Dockerfile.functions-runner" "$root"

runner="$scratch/konae-functions-spike"
test -x "$runner" || { echo 'el build no produjo el runner ejecutable' >&2; exit 1; }
description=$(file -b "$runner")
case "$description" in
  *"$expected"*) ;;
  *) echo "runner $description, se esperaba $arch" >&2; exit 1 ;;
esac
mkdir -p "$output"
install -m 755 "$runner" "$output/konae-functions-spike"
shasum -a 256 "$output/konae-functions-spike"
