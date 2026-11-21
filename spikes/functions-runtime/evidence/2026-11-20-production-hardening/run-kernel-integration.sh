#!/bin/sh
set -eu

package_dir=${1:?pass the temporary package-clean output directory}
image=rust@sha256:c1e5f19e773b7878c3f7a805dd00a495e747acbdc76fb2337a4ebf0418896b33
set -- --rm --pull never --network none --cgroupns private \
    --cap-drop ALL --cap-add SYS_ADMIN --cap-add SETPCAP --cap-add KILL \
    --cap-add SETUID --cap-add SETGID --read-only --security-opt no-new-privileges \
    --memory 5g --memory-swap 5g --cpus 2 --pids-limit 512 \
    --tmpfs /tmp:rw,exec,nosuid,nodev,size=2g \
    -v "$PWD:/repo:ro" -v "$package_dir/bin:/bundle:ro" \
    -v konae-functions-hardening-linux-target-20261120:/target:rw \
    -e KONAE_TEST_FILTER -e KONAE_TEST_IGNORED \
    -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/target
if [ -n "${KONAE_TEST_PRESERVE_HOST:-}" ]; then
    set -- "$@" -v "$KONAE_TEST_PRESERVE_HOST:/preserve:rw" \
        -e KONAE_TEST_PRESERVE_APP_DIR=/preserve/http_functions
fi
docker run "$@" "$image" sh \
    /repo/spikes/functions-runtime/evidence/2026-11-20-production-hardening/run-kernel-strict-linux.sh
