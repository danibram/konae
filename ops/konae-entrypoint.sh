#!/bin/sh
set -eu
data="${KONAE_DATA_DIR:-/data}"
kind=${KONAE_FUNCTIONS_RUNNER_KIND:-wasm}
if [ "$kind" = wasm ]; then
  export KONAE_FUNCTIONS_RUNNER="${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner-wasm}"
  export KONAE_FUNCTIONS_WASM_CACHE="${KONAE_FUNCTIONS_WASM_CACHE:-$data/.konae/wasm-cache}"
else
  export KONAE_FUNCTIONS_RUNNER="${KONAE_FUNCTIONS_RUNNER:-/usr/local/lib/konae/functions-runner-deno}"
fi
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown konae:konae /data
  chmod 0700 /data
  export HOME=/data
  if [ "$kind" = wasm ]; then
    cache=$KONAE_FUNCTIONS_WASM_CACHE
    case "$cache" in
      "$data/.konae/wasm-cache")
        install -d -m 0700 -o konae -g konae "$data/.konae" "$cache" || true ;;
      /*) install -d -m 0700 -o konae -g konae "$cache" || true ;;
    esac
  fi
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    if [ "$kind" = wasm ]; then
      exec setpriv --reuid=konae --regid=konae --clear-groups \
        --bounding-set=-all,+setuid,+setgid,+kill \
        --inh-caps=+setuid,+setgid,+kill --ambient-caps=+setuid,+setgid,+kill \
        -- /usr/local/bin/konae
    fi
    exec setpriv --reuid=konae --regid=konae --clear-groups \
      --bounding-set=-all,+setuid,+setgid,+kill \
      --inh-caps=+setuid,+setgid,+kill --ambient-caps=+setuid,+setgid,+kill \
      -- /usr/local/bin/konae
  fi
  exec setpriv --reuid=konae --regid=konae --clear-groups --inh-caps=-all -- /usr/local/bin/konae
fi
exec /usr/local/bin/konae
