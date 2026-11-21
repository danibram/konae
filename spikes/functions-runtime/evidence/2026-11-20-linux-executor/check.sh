#!/bin/sh
set -eu
mkdir -p /tmp/cargo /tmp/check
ln -s /registry /tmp/cargo/registry
cp -R /source/. /tmp/check/
printf '\n[workspace]\n' >> /tmp/check/Cargo.toml
cp /evidence/kernel-input.lock /tmp/check/Cargo.lock
cd /tmp/check
cargo test --offline --lib -- --test-threads=1
cp Cargo.lock /evidence/Cargo.lock
cat /proc/self/cgroup
cat /sys/fs/cgroup/memory.max
cat /sys/fs/cgroup/pids.max
cat /sys/fs/cgroup/cgroup.subtree_control
