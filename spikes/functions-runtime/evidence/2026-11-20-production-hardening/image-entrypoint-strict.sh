#!/bin/sh
set -eu

root=/tmp/cgroup
mkdir "$root"
mount -t cgroup2 -o nosuid,nodev,noexec cgroup2 "$root"
mkdir "$root/setup"
printf '%s\n' "$$" > "$root/setup/cgroup.procs"
printf '%s\n' '+memory +pids' > "$root/cgroup.subtree_control"
mkdir "$root/delegated"
printf '%s\n' '+memory +pids' > "$root/delegated/cgroup.subtree_control"
chown 10001:10001 "$root/delegated/cgroup.procs"
mkdir "$root/delegated/kernel-manager"
printf '%s\n' "$$" > "$root/delegated/kernel-manager/cgroup.procs"
chown 10001:10001 "$root/delegated/kernel-manager"
chown 10001:10001 "$root/delegated/kernel-manager/cgroup.procs"
chown 10001:10001 "$root/delegated"
test "$(stat -c %u "$root/delegated/cgroup.procs")" = 10001
test "$(stat -c %u "$root/delegated/cgroup.subtree_control")" = 0
stat -c '%u:%g %a %n' "$root/delegated/cgroup.procs" \
    "$root/delegated/cgroup.subtree_control" "$root/delegated/memory.max"
setpriv --reuid=10001 --regid=10001 --clear-groups test -w \
    "$root/delegated/cgroup.procs"
if setpriv --reuid=10001 --regid=10001 --clear-groups test -w \
    "$root/delegated/cgroup.subtree_control"; then
    echo 'Kernel identity can change delegated cgroup controls' >&2
    exit 1
fi
echo cgroup_policy=procs-writable controls-operator-owned

if [ "${IMAGE_KIND:?}" = kernel ]; then
    chown -R 10001:10001 /kernel/apps/http_functions
    export KONAE_DATA_DIR=/kernel KONAE_BIND=127.0.0.1 KONAE_KERNEL_PORT=8787
    entrypoint=/usr/local/bin/kernel
else
    export KONAE_DATA_DIR=/data KONAE_BIND=127.0.0.1 KONAE_KERNEL_PORT=8787
    export KONAE_SETUP_KEY=fixture-only-key
    entrypoint=/usr/local/bin/konae-entrypoint
fi
export KONAE_FUNCTIONS_RUNNER=/usr/local/lib/konae/functions-runner
export KONAE_FUNCTIONS_CGROUP="$root/delegated"
export KONAE_FUNCTIONS_UID=10002 KONAE_FUNCTIONS_GID=10002

start_server() {
    setpriv --bounding-set=-all,+kill,+setuid,+setgid,+chown,+fowner \
        --inh-caps=+kill,+setuid,+setgid --ambient-caps=+kill,+setuid,+setgid \
        --no-new-privs "$entrypoint" &
    server=$!
    ready=0
    for _ in $(seq 1 100); do
        if curl -fsS http://127.0.0.1:8787/_konae/health >/tmp/health.json; then
            ready=1
            break
        fi
        kill -0 "$server" 2>/dev/null || { cat /tmp/server.log 2>/dev/null || true; exit 1; }
        sleep 0.1
    done
    test "$ready" = 1
    printf 'health='; cat /tmp/health.json
    grep -E '^CapEff:[[:space:]]+00000000000000e0$' "/proc/$server/status"
}
cleanup() {
    kill "$server" 2>/dev/null || true
    wait "$server" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
start_server

if [ "$IMAGE_KIND" = kernel ]; then
    host=$(cat /preserve/test-host.txt)
    cookie=$(cat /preserve/test-cookie.txt)
    code=$(curl -sS -o /tmp/function.json -w '%{http_code}' \
        -H "Host: $host" -H "Cookie: $cookie" \
        http://127.0.0.1:8787/api/orders)
    printf 'function_http=%s body=' "$code"; cat /tmp/function.json
    test "$code" = 200
    grep -F '"quantity":6' /tmp/function.json
    grep -F '"version":"A"' /tmp/function.json
fi

cleanup
start_server
cleanup
trap - EXIT INT TERM
for _ in $(seq 1 100); do
    remaining=$(find "$root/delegated" -mindepth 1 -maxdepth 1 -type d ! -name kernel-manager -print -quit)
    [ -z "$remaining" ] && break
    sleep 0.05
done
test -z "$remaining"
echo strict_entrypoint=ready function=${IMAGE_KIND}
