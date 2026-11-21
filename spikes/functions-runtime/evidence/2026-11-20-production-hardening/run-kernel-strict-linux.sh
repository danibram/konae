#!/bin/sh
set -eux

root=/tmp/cgroup
mkdir "$root"
mount -t cgroup2 -o nosuid,nodev,noexec cgroup2 "$root"
mkdir "$root/setup"
printf '%s\n' "$$" > "$root/setup/cgroup.procs"
printf '+memory +pids\n' > "$root/cgroup.subtree_control"
delegated="$root/delegated"
mkdir "$delegated"
printf '+memory +pids\n' > "$delegated/cgroup.subtree_control"
mkdir "$delegated/kernel"
printf '+memory +pids\n' > "$delegated/kernel/cgroup.subtree_control"
mkdir "$delegated/kernel/test-runner"
printf '%s\n' "$$" > "$delegated/kernel/test-runner/cgroup.procs"
test_binary=$(find /target/debug/deps -maxdepth 1 -type f -name 'kernel-*' -perm -111 | head -n 1)
test -n "$test_binary"
sha256sum "$test_binary" /bundle/konae-functions-spike
uname -srmo
test_filter=${KONAE_TEST_FILTER:-real_functions_http_auth_transaction_publish_deadline_and_usage}
test_ignored=${KONAE_TEST_IGNORED:-1}
if [ "$test_ignored" = 1 ]; then
    exec setpriv --bounding-set=-all,+kill,+setuid,+setgid --inh-caps=-all --ambient-caps=-all --no-new-privs \
        env KONAE_FUNCTIONS_UID=65534 KONAE_FUNCTIONS_GID=65534 \
        KONAE_TEST_FUNCTION_WORKER=/bundle/konae-functions-spike \
        KONAE_TEST_FUNCTION_CGROUP="$delegated/kernel" \
        KONAE_TEST_FUNCTION_TIMINGS=1 \
        "$test_binary" "$test_filter" --ignored --nocapture --test-threads=1
fi
exec setpriv --bounding-set=-all,+kill,+setuid,+setgid --inh-caps=-all --ambient-caps=-all --no-new-privs \
    env KONAE_FUNCTIONS_UID=65534 KONAE_FUNCTIONS_GID=65534 \
    KONAE_TEST_FUNCTION_WORKER=/bundle/konae-functions-spike \
    KONAE_TEST_FUNCTION_CGROUP="$delegated/kernel" \
    "$test_binary" "$test_filter" --nocapture --test-threads=1
