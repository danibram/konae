#!/bin/sh
set -eu

index=index.crates.io-1949cf8c6b5b557f
registry=/tmp/cargo/registry
mkdir -p "$registry/cache/$index" "$registry/src/$index"
ln -s /registry/index "$registry/index"
find "/registry/cache/$index" -maxdepth 1 -name '*.crate' -exec ln -s -t "$registry/cache/$index" {} +
find "/registry/src/$index" -mindepth 1 -maxdepth 1 -type d -exec ln -s -t "$registry/src/$index" {} +
cargo test --manifest-path /repo/kernel-rs/Cargo.toml -p kernel --lib --no-run --locked --offline
