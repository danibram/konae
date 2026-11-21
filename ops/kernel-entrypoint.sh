#!/bin/sh
set -eu
data="${KONAE_DATA_DIR:-/kernel}"
if [ "$(id -u)" = 0 ]; then
  owner="$(stat -c %u:%g "$data"/apps/*/ 2>/dev/null | head -n 1 || true)"
  case "$owner" in ""|0:*) owner=10001:10001 ;; esac
  chown "$owner" "$data" "$data/apps"
  export HOME="$data"
  if [ -n "${KONAE_FUNCTIONS_RUNNER:-}" ] && [ "${KONAE_FUNCTIONS_DEVELOPMENT:-0}" != "1" ]; then
    exec setpriv --reuid="${owner%%:*}" --regid="${owner##*:}" --clear-groups \
      --inh-caps=+kill,+setuid,+setgid --ambient-caps=+kill,+setuid,+setgid \
      -- /usr/local/lib/konae/kernel "$@"
  fi
  exec setpriv --reuid="${owner%%:*}" --regid="${owner##*:}" --clear-groups --inh-caps=-all -- /usr/local/lib/konae/kernel "$@"
fi
exec /usr/local/lib/konae/kernel "$@"
