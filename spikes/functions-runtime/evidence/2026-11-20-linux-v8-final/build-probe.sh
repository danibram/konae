#!/bin/sh
set -eux

mkdir /tmp/cargo
ln -s /registry /tmp/cargo/registry
cp -R /repo/kernel-rs/functions-executor /tmp/executor
printf '\n[workspace]\n' >> /tmp/executor/Cargo.toml
cp /repo/spikes/functions-runtime/rust-host/Cargo.lock /tmp/executor/Cargo.lock
cargo build --release --offline --manifest-path /tmp/executor/Cargo.toml
rustc --edition=2024 -C strip=debuginfo -L dependency=/tmp/target/release/deps \
    --extern konae_functions_executor="$(find /tmp/target/release/deps -name 'libkonae_functions_executor-*.rlib')" \
    --extern serde_json="$(find /tmp/target/release/deps -name 'libserde_json-*.rlib')" \
    /repo/spikes/functions-runtime/evidence/2026-11-20-linux-v8-final/probe.rs -o /out/probe
sha256sum /out/probe
