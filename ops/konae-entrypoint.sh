#!/bin/sh
set -eu
data="${KONAE_DATA_DIR:-/data}"
export KONAE_FUNCTIONS_RUNNER_KIND="${KONAE_FUNCTIONS_RUNNER_KIND-wasm}"
case "$KONAE_FUNCTIONS_RUNNER_KIND" in
  wasm)
    export KONAE_FUNCTIONS_RUNNER="${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner-wasm}"
    export KONAE_FUNCTIONS_WASM_CACHE="${KONAE_FUNCTIONS_WASM_CACHE:-$data/.konae/wasm-cache}"
    export KONAE_FUNCTIONS_PREPARER="${KONAE_FUNCTIONS_PREPARER:-/usr/local/lib/konae/functions-prepare}"
    ;;
  deno)
    export KONAE_FUNCTIONS_RUNNER="${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner-deno}"
    ;;
esac
if [ "$(id -u)" = 0 ]; then
  mkdir -p "$data"
  chown konae:konae "$data"
  chmod 0700 "$data"
  export HOME="$data"
  if [ "$KONAE_FUNCTIONS_RUNNER_KIND" = wasm ]; then
    cache=$KONAE_FUNCTIONS_WASM_CACHE
    mkdir -p "$cache"
    if [ "$cache" = "$data/.konae/wasm-cache" ]; then
      chown konae:konae "$data/.konae"
    fi
    chown konae:konae "$cache"
    chmod 0700 "$cache"
  fi
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    exec setpriv --reuid=konae --regid=konae --clear-groups \
      --bounding-set=-all,+setuid,+setgid,+kill \
      --inh-caps=+setuid,+setgid,+kill --ambient-caps=+setuid,+setgid,+kill \
      -- /usr/local/bin/konae
  fi
  exec setpriv --reuid=konae --regid=konae --clear-groups --inh-caps=-all -- /usr/local/bin/konae
fi
exec /usr/local/bin/konae
