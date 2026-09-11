#!/bin/sh
set -eu
if [ "$(id -u)" = 0 ]; then
  mkdir -p /data
  chown littlebox:littlebox /data
  chmod 0700 /data
  export HOME=/data
  exec setpriv --reuid=littlebox --regid=littlebox --init-groups --inh-caps=-all -- /usr/local/bin/littlebox-kernel
fi
exec /usr/local/bin/littlebox-kernel
