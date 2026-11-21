#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
case "$(uname -m)" in
  arm64|aarch64) arch=arm64 ;;
  x86_64|amd64) arch=amd64 ;;
  *) echo "arquitectura del host no admitida: $(uname -m)" >&2; exit 2 ;;
esac
output=${KONAE_FUNCTIONS_RUNNER_CONTEXT:-$root/.konae/functions-runner}
expected=''
case "$arch" in
  amd64) expected='x86-64' ;;
  arm64) expected='ARM aarch64' ;;
esac
runner="$output/konae-functions-runner"
valid=0
if [ -x "$runner" ] && command -v file >/dev/null 2>&1; then
  description=$(file -b "$runner")
  case "$description" in *"$expected"*) valid=1 ;; esac
fi
if [ "$valid" -ne 1 ]; then
  sh "$root/scripts/build-functions-runner.sh" "$arch" "$output"
fi
KONAE_FUNCTIONS_RUNNER_SHA256=$(shasum -a 256 "$runner" | awk '{print $1}')
export KONAE_FUNCTIONS_RUNNER_SHA256
cd "$root"
KONAE_FUNCTIONS_RUNNER_CONTEXT="$output" docker compose "$@"
