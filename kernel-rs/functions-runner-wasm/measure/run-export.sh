#!/bin/sh
set -eu
if [ "$#" -ne 3 ]; then
    printf '%s\n' 'Usage: run-export.sh KERNEL RUNNER EXPORTED_APP_FOLDER' >&2
    exit 2
fi
root=$(mktemp -d)
pid=
cleanup() {
    if [ -n "$pid" ]; then
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    fi
    rm -rf "$root"
}
trap cleanup EXIT
id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["id"])' "$3/konae.json")
mkdir "$root/apps"
cp -R "$3" "$root/apps/$id"
port=${PORT:-18796}
env KONAE_DATA_DIR="$root" KONAE_BIND=127.0.0.1 KONAE_KERNEL_PORT="$port" \
    KONAE_HOST_SUFFIX=localhost KONAE_PUBLIC_ORIGIN="http://localhost:$port" \
    KONAE_EXECUTION_MODE=test KONAE_MAIL_TRANSPORT=development \
    KONAE_FUNCTIONS_DEVELOPMENT=1 KONAE_FUNCTIONS_RUNNER="$2" "$1" >"$root/kernel.log" 2>&1 &
pid=$!
if ! python3 - "$port" "$pid" <<'PY'
import os
import sys
import time
from urllib.request import urlopen

port, pid = map(int, sys.argv[1:])
for _ in range(100):
    os.kill(pid, 0)
    try:
        with urlopen(f'http://127.0.0.1:{port}/_konae/health', timeout=1):
            break
    except OSError:
        time.sleep(0.1)
else:
    sys.exit('Kernel did not start')
PY
then
    cat "$root/kernel.log" >&2
    exit 1
fi
if ! python3 "$(dirname "$0")/http-bench.py" --port "$port" --pid "$pid"; then
    cat "$root/kernel.log" >&2
    exit 1
fi
