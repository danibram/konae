#!/bin/sh
set -eu

evidence=spikes/functions-runtime/evidence/2026-11-20-production-hardening
index=index.crates.io-1949cf8c6b5b557f
registry=/tmp/cargo/registry
target=${CARGO_TARGET_DIR:-/target}
mkdir -p "$registry/cache/$index" "$registry/src/$index" /out/bin
ln -s /registry/index "$registry/index"
find "/registry/cache/$index" -maxdepth 1 -name '*.crate' -exec ln -s -t "$registry/cache/$index" {} +
find "/registry/src/$index" -mindepth 1 -maxdepth 1 -type d -exec ln -s -t "$registry/src/$index" {} +
cp /out/*.crate "$registry/cache/$index/"
rustc -Vv
cargo -V
cargo build --release --offline --locked \
    --manifest-path /repo/spikes/functions-runtime/rust-host/Cargo.toml \
    --bin konae-functions-spike --bin oom-probe
cp "$target/release/konae-functions-spike" /out/bin/
cp "$target/release/oom-probe" /out/bin/
sha256sum /out/bin/konae-functions-spike
find "$target/release/build"/konae-functions-spike-*/out/embedded -type f -name '*.js' \
    -exec sha256sum {} + | sort -k2
sha256sum /out/bin/oom-probe
