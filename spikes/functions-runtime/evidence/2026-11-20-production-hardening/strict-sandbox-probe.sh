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
mkdir "$root/supervisor"
printf '%s\n' "$$" > "$root/supervisor/cgroup.procs"
uname -srmo >&2
exec setpriv --bounding-set=-all,+kill,+setuid,+setgid --inh-caps=-all --ambient-caps=-all --no-new-privs \
    sh -c 'exec 3</dev/null; exec /bundle/oom-probe --sandbox-attack /tmp/cgroup/delegated 3'
