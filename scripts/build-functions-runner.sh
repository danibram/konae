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

runner="$scratch/konae-functions-runner-wasm"
deno_runner="$scratch/konae-functions-runner-deno"
preparer="$scratch/konae-functions-prepare"
test -x "$runner" && test -x "$deno_runner" && test -x "$preparer" || { echo 'the build did not produce both runners and the preparation helper' >&2; exit 1; }
description=$(file -b "$runner")
case "$description" in
  *"$expected"*) ;;
  *) echo "runner $description, se esperaba $arch" >&2; exit 1 ;;
esac
deno_description=$(file -b "$deno_runner")
case "$deno_description" in
  *"$expected"*) ;;
  *) echo "deno runner $deno_description, se esperaba $arch" >&2; exit 1 ;;
esac
mkdir -p "$output"
install -m 755 "$runner" "$output/konae-functions-runner-wasm"
install -m 755 "$deno_runner" "$output/konae-functions-runner-deno"
install -m 755 "$preparer" "$output/konae-functions-prepare"
shasum -a 256 "$output/konae-functions-runner-wasm" "$output/konae-functions-runner-deno" "$output/konae-functions-prepare"
