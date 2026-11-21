#!/bin/sh
set -eu
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown konae:konae /data
  chmod 0700 /data
  export HOME=/data
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    exec setpriv --reuid=konae --regid=konae --clear-groups \
      --inh-caps=+kill,+setuid,+setgid --ambient-caps=+kill,+setuid,+setgid \
      -- /usr/local/bin/konae
  fi
  exec setpriv --reuid=konae --regid=konae --clear-groups --inh-caps=-all -- /usr/local/bin/konae
fi
exec /usr/local/bin/konae
