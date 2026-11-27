#!/bin/sh
set -eu

kind=${TEST_KIND:?set TEST_KIND to authenticated-read or sandbox-denied}
uname -srmo
root=/tmp/cgroup
data=/kernel
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
chown 10001:10001 "$root/delegated/kernel-manager" "$root/delegated/kernel-manager/cgroup.procs"
chown 10001:10001 "$root/delegated"
test -e "$root/delegated/cgroup.kill"
test -w "$root/delegated/cgroup.procs"
printf 'delegated_cgroup_kill=present\n'

mkdir -p "$data/apps"
cp -a /preserve/http_functions "$data/apps/http_functions"
chown -R 10001:10001 "$data/apps/http_functions"
export KONAE_DATA_DIR=$data KONAE_BIND=127.0.0.1 KONAE_KERNEL_PORT=8787
export KONAE_FUNCTIONS_RUNNER=/usr/local/lib/konae/functions-runner
export KONAE_FUNCTIONS_CGROUP="$root/delegated"
export KONAE_FUNCTIONS_UID=10002 KONAE_FUNCTIONS_GID=10002

/usr/local/bin/kernel > /tmp/kernel.log 2>&1 &
server=$!
cleanup() {
  kill "$server" 2>/dev/null || true
  wait "$server" 2>/dev/null || true
  cat /tmp/kernel.log
}
trap cleanup EXIT INT TERM
for _ in $(seq 1 100); do
  if curl -fsS http://127.0.0.1:8787/_konae/health >/tmp/health.json; then break; fi
  kill -0 "$server" 2>/dev/null || { cat /tmp/kernel.log; exit 1; }
  sleep 0.1
done
cat /tmp/health.json
printf 'kernel_cap_eff='
grep -E '^CapEff:' "/proc/$server/status"
grep -E '^CapEff:[[:space:]]+00000000000000c0$' "/proc/$server/status"
host=$(cat /preserve/test-host.txt)
if [ -f /preserve/test-cookie.txt ]; then
  cookie=$(cat /preserve/test-cookie.txt)
else
  code=$(curl -sS -D /tmp/login.headers -o /tmp/login.json -w '%{http_code}' \
    -H "Host: $host" -H 'Content-Type: application/json' \
    --data '{"email":"alice@example.test","password":"correct horse battery"}' \
    http://127.0.0.1:8787/_konae/auth/login)
  test "$code" = 200
  cookie=$(awk 'tolower($1) == "set-cookie:" { sub(/;$/, "", $2); print $2; exit }' /tmp/login.headers)
  test -n "$cookie"
  printf 'authenticated_login_http=%s\n' "$code"
fi
if [ "$kind" = authenticated-read ]; then
  code=$(curl -sS -o /tmp/function.json -w '%{http_code}' \
    -H "Host: $host" -H "Cookie: $cookie" http://127.0.0.1:8787/api/orders)
  printf 'authenticated_sqlite_http=%s body=' "$code"
  cat /tmp/function.json
  test "$code" = 200
  grep -F '"quantity":7' /tmp/function.json
  grep -F '"version":"A"' /tmp/function.json
elif [ "$kind" = sandbox-denied ]; then
  code=$(curl -sS -o /tmp/function.json -w '%{http_code}' -X POST \
    -H "Host: $host" -H "Cookie: $cookie" -H 'Content-Type: application/json' \
    -H 'Idempotency-Key: sandbox-floor-denied' --data '{"id":1,"quantity":99}' \
    http://127.0.0.1:8787/api/orders)
  printf 'sandbox_denied_http=%s body=' "$code"
  cat /tmp/function.json
  test "$code" = 503
  grep -F '"code":"sandbox_unavailable"' /tmp/function.json
else
  echo "unsupported TEST_KIND: $kind" >&2
  exit 2
fi
cleanup
trap - EXIT INT TERM
if [ "$kind" = sandbox-denied ]; then
  for _ in $(seq 1 20); do
    leftovers=$(find "$root/delegated" -mindepth 1 -maxdepth 1 -type d -name 'konae-worker-*')
    [ -n "$leftovers" ] || break
    sleep 0.1
  done
  if [ -n "${leftovers:-}" ]; then
    printf 'worker cgroups remain after denied request\n%s\n' "$leftovers" >&2
    exit 1
  fi
  printf 'worker_cgroups_after_denied_request=none\n'
else
  leftovers=$(find "$root/delegated" -mindepth 1 -maxdepth 1 -type d -name 'konae-worker-*')
  printf 'worker_cgroups_after_read=%s\n' "$(printf '%s\n' "$leftovers" | sed '/^$/d' | wc -l | tr -d ' ')"
fi
