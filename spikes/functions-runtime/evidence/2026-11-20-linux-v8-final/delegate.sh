#!/bin/sh
set -eux

test "$(cat /proc/self/cgroup)" = '0::/'
outer=/tmp/cgroup
mkdir "$outer"
mount -t cgroup2 -o nosuid,nodev,noexec cgroup2 "$outer"
test "$(cat "$outer/memory.max")" = 536870912
test "$(cat "$outer/pids.max")" = 96
awk '$5 == "/tmp/cgroup"' /proc/self/mountinfo >&2
mkdir "$outer/setup"
printf '%s\n' "$$" > "$outer/setup/cgroup.procs"
printf '+memory +pids\n' > "$outer/cgroup.subtree_control"

root="$outer/delegated"
mkdir "$root"
printf '+memory +pids\n' > "$root/cgroup.subtree_control"
mkdir "$root/supervisor"
printf '%s\n' "$$" > "$root/supervisor/cgroup.procs"
cat "$root/cgroup.subtree_control" >&2
uname -srmo >&2
sha256sum /bin-probe/probe /bin-probe/konae-functions-spike >&2
exec setpriv --bounding-set=-all --inh-caps=-all --ambient-caps=-all --no-new-privs \
    /bin-probe/probe /bin-probe/konae-functions-spike /evidence "$root" "$outer"
