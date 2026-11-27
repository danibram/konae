#!/bin/sh
set -eu
export CARGO_TARGET_DIR=/target

root=/tmp/cgroup
mkdir /tmp/preserve
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
chown 10001:10001 "$root/delegated" "$root/delegated/kernel-manager" \
  "$root/delegated/kernel-manager/cgroup.procs" /tmp/preserve
test -e "$root/delegated/cgroup.kill"
test -w "$root/delegated/cgroup.procs"
printf 'delegated_cgroup_kill=present\n'

cargo test --locked --manifest-path /repo/kernel-rs/Cargo.toml -p kernel --lib --no-run
binary=$(find /target/debug/deps -maxdepth 1 -type f -name 'kernel-*' -perm -111 | head -n 1)
test -n "$binary"
uname -srmo
tests=${FUNCTION_TEST_FILTER:-"real_http_stream_accepts_100mb_and_refuses_the_next_byte_without_content_length real_functions_http_auth_transaction_publish_deadline_and_usage functions_export_import_executes_the_published_bundle_with_imported_identity"}
for test in $tests; do
  setpriv --reuid=10001 --regid=10001 --clear-groups \
    --bounding-set=-all,+setuid,+setgid --inh-caps=+setuid,+setgid \
    --ambient-caps=+setuid,+setgid --no-new-privs \
    env KONAE_FUNCTIONS_UID=65534 KONAE_FUNCTIONS_GID=65534 \
    KONAE_TEST_FUNCTION_WORKER=/bundle/konae-functions-runner \
    KONAE_TEST_FUNCTION_CGROUP="$root/delegated" \
    KONAE_TEST_PRESERVE_APP_DIR=/tmp/preserve/http_functions \
    "$binary" "$test" --ignored --nocapture --test-threads=1
done
