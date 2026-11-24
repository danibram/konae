#!/bin/sh
set -eu
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown konae:konae /data
  chmod 0700 /data
  export HOME=/data
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    if [ "${KONAE_FUNCTIONS_RUNNER_KIND:-deno}" = wasm ]; then
      cache=${KONAE_FUNCTIONS_WASM_CACHE:-/data/.konae/wasm-cache}
      mkdir -p "$cache"
      chown konae:konae "$cache"
      chmod 0700 "$cache"
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
