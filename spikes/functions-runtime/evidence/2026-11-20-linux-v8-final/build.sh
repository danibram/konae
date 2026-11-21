#!/bin/sh
set -eu

reg=/tmp/cargo/registry
index=index.crates.io-1949cf8c6b5b557f
mkdir -p "$reg/cache/$index" "$reg/src/$index"
ln -s /registry/index "$reg/index"
find "/registry/cache/$index" -maxdepth 1 -name '*.crate' -exec ln -s -t "$reg/cache/$index" {} +
find "/registry/src/$index" -mindepth 1 -maxdepth 1 -type d -exec ln -s -t "$reg/src/$index" {} +
cp /out/*.crate "$reg/cache/$index/"
rustc -Vv
cargo -V
cargo build --release --offline --locked --manifest-path /repo/spikes/functions-runtime/rust-host/Cargo.toml --bin konae-functions-spike
cp /tmp/target/release/konae-functions-spike /out/bin/
sha256sum /out/bin/konae-functions-spike
df -h /tmp
