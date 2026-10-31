#!/bin/sh
set -eu
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown konae:konae /data
  chmod 0700 /data
  export HOME=/data
  exec setpriv --reuid=konae --regid=konae --init-groups --inh-caps=-all -- /usr/local/bin/konae-kernel
fi
exec /usr/local/bin/konae-kernel
