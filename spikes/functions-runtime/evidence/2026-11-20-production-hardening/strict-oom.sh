#!/bin/sh
set -eux

test "$(cat /proc/self/cgroup)" = '0::/'
outer=/tmp/cgroup
mkdir "$outer"
mount -t cgroup2 -o nosuid,nodev,noexec cgroup2 "$outer"
test "$(cat "$outer/memory.max")" = 536870912
test "$(cat "$outer/pids.max")" = 96
mkdir "$outer/setup"
printf '%s\n' "$$" > "$outer/setup/cgroup.procs"
printf '+memory +pids\n' > "$outer/cgroup.subtree_control"

root="$outer/delegated"
mkdir "$root"
printf '+memory +pids\n' > "$root/cgroup.subtree_control"
mkdir "$root/foreign-sibling"
printf 536870912 > "$root/foreign-sibling/memory.max"
printf 0 > "$root/foreign-sibling/memory.swap.max"
printf 96 > "$root/foreign-sibling/pids.max"
printf 1 > "$root/foreign-sibling/memory.oom.group"
mkdir "$root/supervisor"
printf '%s\n' "$$" > "$root/supervisor/cgroup.procs"
uname -srmo >&2
sha256sum /bundle/konae-functions-spike /bundle/oom-probe >&2
setpriv --bounding-set=-all,+kill,+setuid,+setgid --inh-caps=-all --ambient-caps=-all --no-new-privs \
    env KONAE_FUNCTIONS_UID=65534 KONAE_FUNCTIONS_GID=65534 \
    /bundle/oom-probe --die-with-worker /bundle/konae-functions-spike /evidence "$root" /tmp/orphan.marker &
supervisor=$!
for _ in $(seq 1 100); do
    test -s /tmp/orphan.marker && break
    sleep .05
done
test -s /tmp/orphan.marker
read worker orphan_group < /tmp/orphan.marker
sleep .5
kill -KILL "$supervisor"
wait "$supervisor" 2>/dev/null || true
for _ in $(seq 1 100); do
    test ! -e "/proc/$worker" && break
    sleep .05
done
test ! -e "/proc/$worker"
test -d "$orphan_group"
printf 'supervisor_sigkill=worker_pdeathsig_reaped=true orphan_group_retained_for_safe_recovery=true\n'
setpriv --bounding-set=-all,+kill,+setuid,+setgid --inh-caps=-all --ambient-caps=-all --no-new-privs \
    env KONAE_FUNCTIONS_UID=65534 KONAE_FUNCTIONS_GID=65534 KONAE_TEST_SECRET=parent-secret \
    /bundle/oom-probe /bundle/konae-functions-spike /evidence "$root"
test -d "$root/foreign-sibling"
test "$(cat "$root/foreign-sibling/memory.max")" = 536870912
printf 'foreign_sibling=preserved=true\n'
