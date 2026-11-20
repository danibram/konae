#!/bin/sh
set -eu

evidence=spikes/functions-runtime/evidence/2026-11-20-production-hardening
registry="${CARGO_HOME:-$HOME/.cargo}/registry"
scratch=$(mktemp -d /tmp/konae-functions-package.XXXXXX)
target_volume=konae-functions-hardening-linux-target-20261120
docker volume create "$target_volume" >/dev/null
cleanup() {
    if [ "${KEEP_PACKAGE:-0}" = 1 ]; then
        printf '%s\n' "$scratch" > /tmp/konae-functions-package-path
    else
        rm -rf "$scratch"
        docker volume rm "$target_volume" >/dev/null
    fi
}
trap cleanup EXIT

curl -fsSL --max-time 120 --max-filesize 200000000 \
    https://github.com/denoland/rusty_v8/releases/download/v150.4.0/librusty_v8_simdutf_release_aarch64-unknown-linux-gnu.a.gz \
    -o "$scratch/v8.a.gz"
for package in openssl-probe-0.1.6 rustls-native-certs-0.7.3; do
    curl -fsSL --max-time 30 "https://static.crates.io/crates/${package%-*}/$package.crate" \
        -o "$scratch/$package.crate"
done
(cd "$scratch" && shasum -a 256 -c <<'HASHES'
539e283815a396a5796f32858b42e517b858ebaaeaaad05d03290ee8c864a527  v8.a.gz
d05e27ee213611ffe7d6348b942e8f942b37114c00cc03cec254295a4a17852e  openssl-probe-0.1.6.crate
e5bfb394eeed242e909609f56089eecfe5fda225042e8b171791b9c95f5931e5  rustls-native-certs-0.7.3.crate
HASHES
)

image=rust@sha256:c1e5f19e773b7878c3f7a805dd00a495e747acbdc76fb2337a4ebf0418896b33
docker image inspect --format 'build_image={{.Id}} arch={{.Architecture}} os={{.Os}}' "$image"
docker run --rm --pull never --network none --cgroupns private --cap-drop ALL \
    --read-only --security-opt no-new-privileges --memory 5g --memory-swap 5g \
    --cpus 2 --pids-limit 192 --tmpfs /tmp:rw,exec,nosuid,nodev,size=1g \
    -v "$PWD:/repo:ro" -v "$registry:/registry:ro" -v "$scratch:/out:rw" \
    -v "$target_volume:/target" \
    -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS=2 \
    -e CARGO_PROFILE_RELEASE_OPT_LEVEL=1 -e CARGO_PROFILE_RELEASE_DEBUG=0 \
    -e RUSTY_V8_ARCHIVE=/out/v8.a.gz "$image" sh "/repo/$evidence/build-clean.sh"

clean_image=python:3.12-slim
docker_image_id=$(docker image inspect --format '{{.Id}}' "$clean_image")
printf 'clean_image=%s\n' "$docker_image_id"
docker run --rm --pull never --network none --cgroupns private --cap-drop ALL \
    --read-only --security-opt no-new-privileges --memory 512m --memory-swap 512m \
    --cpus 1 --pids-limit 32 --tmpfs /tmp:rw,nosuid,nodev,noexec,size=8m \
    -v "$scratch/bin:/bundle:ro" -v "$PWD/$evidence/smoke.py:/smoke.py:ro" \
    "$docker_image_id" python /smoke.py /bundle/konae-functions-spike

docker run --rm --pull never --network none --cgroupns private \
    --cap-drop ALL --cap-add SYS_ADMIN --cap-add SETPCAP --cap-add KILL --cap-add SETUID --cap-add SETGID \
    --read-only --security-opt no-new-privileges --memory 512m --memory-swap 512m \
    --cpus 2 --pids-limit 96 --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m \
    -v "$scratch/bin:/bundle:ro" -v "$PWD/$evidence:/evidence:ro" \
    "$image" sh /evidence/strict-oom.sh
