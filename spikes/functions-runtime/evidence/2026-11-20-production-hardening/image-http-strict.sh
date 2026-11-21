#!/bin/sh
set -eu

test_kind=${TEST_KIND:?set TEST_KIND to sandbox-denied or konae-auth-sql}
fixture=/preserve/http_functions
root=/tmp/cgroup
data=${KONAE_DATA_DIR:?}

mkdir -p "$root"
mount -t cgroup2 -o nosuid,nodev,noexec cgroup2 "$root"
mkdir "$root/setup"
printf '%s\n' "$$" > "$root/setup/cgroup.procs"
printf '%s\n' '+memory +pids' > "$root/cgroup.subtree_control"
mkdir "$root/delegated"
printf '%s\n' '+memory +pids' > "$root/delegated/cgroup.subtree_control"
chown 10001:10001 "$root/delegated/cgroup.procs"
mkdir "$root/delegated/kernel-manager"
printf '%s\n' "$$" > "$root/delegated/kernel-manager/cgroup.procs"
chown 10001:10001 "$root/delegated/kernel-manager" "$root/delegated/kernel-manager/cgroup.procs"
chown 10001:10001 "$root/delegated"

mkdir -p "$data/apps"
cp -a "$fixture" "$data/apps/http_functions"
chown -R 10001:10001 "$data/apps/http_functions"

if [ "${IMAGE_KIND:?}" = kernel ]; then
    entrypoint=/usr/local/bin/kernel
else
    export KONAE_SETUP_KEY=fixture-only-key
    entrypoint=/usr/local/bin/konae-entrypoint
fi
export KONAE_BIND=127.0.0.1 KONAE_KERNEL_PORT=8787
export KONAE_FUNCTIONS_RUNNER=/usr/local/lib/konae/functions-runner
export KONAE_FUNCTIONS_CGROUP="$root/delegated"
export KONAE_FUNCTIONS_UID=10002 KONAE_FUNCTIONS_GID=10002

setpriv --bounding-set=-all,+kill,+setuid,+setgid,+chown,+fowner \
    --inh-caps=+kill,+setuid,+setgid --ambient-caps=+kill,+setuid,+setgid \
    --no-new-privs "$entrypoint" > /tmp/server.log 2>&1 &
server=$!
cleanup() {
    kill "$server" 2>/dev/null || true
    wait "$server" 2>/dev/null || true
    cat /tmp/server.log 2>/dev/null || true
}
trap cleanup EXIT INT TERM

ready=0
for _ in $(seq 1 100); do
    if curl -fsS http://127.0.0.1:8787/_konae/health > /tmp/health.json; then
        ready=1
        break
    fi
    kill -0 "$server" 2>/dev/null || { cat /tmp/server.log; exit 1; }
    sleep 0.1
done
test "$ready" = 1
grep -E '^CapEff:[[:space:]]+00000000000000e0$' "/proc/$server/status"

host=$(cat /preserve/test-host.txt)
cookie=$(cat /preserve/test-cookie.txt)
if [ "$test_kind" = sandbox-denied ]; then
    code=$(curl -sS -o /tmp/function.json -w '%{http_code}' -X POST \
        -H "Host: $host" -H "Cookie: $cookie" -H 'Content-Type: application/json' \
        -H 'Idempotency-Key: sandbox-denial-r1' \
        --data '{"id":1,"quantity":99}' http://127.0.0.1:8787/api/orders)
    printf 'sandbox_denied_http=%s body=' "$code"
    cat /tmp/function.json
    test "$code" = 503
    grep -F '"code":"sandbox_unavailable"' /tmp/function.json
elif [ "$test_kind" = konae-auth-sql ]; then
    code=$(curl -sS -o /tmp/function.json -w '%{http_code}' \
        -H "Host: $host" -H "Cookie: $cookie" \
        http://127.0.0.1:8787/api/orders)
    printf 'konae_auth_sql_http=%s body=' "$code"
    cat /tmp/function.json
    test "$code" = 200
    grep -F '"quantity":6' /tmp/function.json
    grep -F '"version":"A"' /tmp/function.json
else
    echo "unsupported TEST_KIND: $test_kind" >&2
    exit 2
fi

cleanup
trap - EXIT INT TERM
echo "image_http_contract=passed kind=$test_kind image=$IMAGE_KIND"
