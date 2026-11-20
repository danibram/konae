#!/bin/sh
set -eu

evidence=spikes/functions-runtime/evidence/2026-11-20-linux-v8-final
registry="${CARGO_HOME:-$HOME/.cargo}/registry"
scratch=$(mktemp -d /tmp/konae-linux-v8-final.XXXXXX)
trap 'rm -rf "$scratch"' EXIT

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

index=index.crates.io-1949cf8c6b5b557f
mkdir "$scratch/extensions" "$scratch/bin"
for crate in deno_fetch-0.284.0 deno_net-0.252.0 deno_web-0.291.0 deno_webidl-0.260.0; do
    archive="$registry/cache/$index/$crate.crate"
    tar -tzf "$archive" | awk -F/ -v crate="$crate" \
        '$1 == crate && NF == 2 && $2 ~ /^[a-zA-Z0-9_.-]+\.js$/ {print}' > "$scratch/extension-files"
    test -s "$scratch/extension-files"
    tar -xzf "$archive" -C "$scratch/extensions" -T "$scratch/extension-files"
done

image=rust@sha256:c1e5f19e773b7878c3f7a805dd00a495e747acbdc76fb2337a4ebf0418896b33
docker run --rm --pull never --network none --cgroupns private --cap-drop ALL \
    --read-only --security-opt no-new-privileges --memory 5g --memory-swap 5g \
    --cpus 2 --pids-limit 192 --tmpfs /tmp:rw,exec,nosuid,nodev,size=3g \
    -v "$PWD:/repo:ro" -v "$registry:/registry:ro" -v "$scratch:/out:rw" \
    -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/tmp/target -e CARGO_BUILD_JOBS=2 \
    -e CARGO_PROFILE_RELEASE_OPT_LEVEL=1 -e CARGO_PROFILE_RELEASE_DEBUG=0 \
    -e RUSTY_V8_ARCHIVE=/out/v8.a.gz "$image" sh "/repo/$evidence/build.sh" >&2

docker run --rm --pull never --network none --cgroupns private --cap-drop ALL \
    --read-only --security-opt no-new-privileges --memory 1g --memory-swap 1g \
    --cpus 2 --pids-limit 128 --tmpfs /tmp:rw,exec,nosuid,nodev,size=350m \
    -v "$PWD:/repo:ro" -v "$registry:/registry:ro" -v "$scratch/bin:/out:rw" \
    -e CARGO_HOME=/tmp/cargo -e CARGO_TARGET_DIR=/tmp/target -e CARGO_BUILD_JOBS=2 \
    -e CARGO_PROFILE_RELEASE_OPT_LEVEL=1 -e CARGO_PROFILE_RELEASE_DEBUG=0 \
    "$image" sh "/repo/$evidence/build-probe.sh" >&2

docker run --rm --pull never --network none --cgroupns private --cap-drop ALL \
    --cap-add SYS_ADMIN --cap-add SETPCAP --read-only --security-opt no-new-privileges \
    --memory 512m --memory-swap 512m --cpus 2 --pids-limit 96 \
    --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m \
    -v "$scratch/bin:/bin-probe:ro" -v "$PWD/$evidence:/evidence:ro" \
    -v "$scratch/extensions:/tmp/cargo/registry/src/$index:ro" \
    "$image" sh /evidence/delegate.sh
