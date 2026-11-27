#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
: "${KONAE_STARLING_WASMTIME:?Set KONAE_STARLING_WASMTIME to the Wasmtime 38.0.4 CLI}"
KONAE_STARLING_NODE_DIR=$(dirname "${KONAE_STARLING_NODE:-node}")
PATH="${KONAE_STARLING_TOOLS:?Set KONAE_STARLING_TOOLS to the component toolchain}/node_modules/.bin:$KONAE_STARLING_NODE_DIR:/opt/homebrew/bin:${PATH:-/usr/bin:/bin}"
export PATH
if [ ! -s "$2" ]; then
    "${KONAE_STARLING_NODE:-node}" "$(dirname "$0")/starling-component.mjs" "$1" "$2" "$4"
fi
case "$("$KONAE_STARLING_WASMTIME" --version)" in
    'wasmtime 38.0.4'|'wasmtime 38.0.4 '*) ;;
    *) printf '%s\n' 'Expected Wasmtime 38.0.4' >&2; exit 1 ;;
esac
exec "$KONAE_STARLING_WASMTIME" compile --target "$5" \
    -W gc-support=n -W threads=n -W stack-switching=n -W epoch-interruption=y \
    -O pooling-allocator=y -o "$3" "$2"
