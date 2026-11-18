#!/bin/sh
# Run at the checkout root. Cargo prints the test executable selected for this toolchain.
set -eu
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/kernel-rs/target}"
artifacts=$(mktemp)
trap 'rm -f "$artifacts"' EXIT
cargo test --release --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib \
    --no-run --message-format=json > "$artifacts"
reader=$(python3 - "$artifacts" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    exe, = (r["executable"] for line in f if (r := json.loads(line)).get("executable") and r["profile"]["test"])
print(exe)
PY
)
python3 -B ops/tests/writer-pilot.py --reader "$reader" "$@"
