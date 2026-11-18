# Installation copy fixtures

Both archives contain synthetic rows and assets. Tar preserves the inode links between the two directory copies. The S3 files are the object bodies written by the released binaries. The test key in `backup/compatibility.py` opens both S3 fixtures.

| Archive | Writer | Contents |
| --- | --- | --- |
| `v0.9.0.tar.gz` | `e34b0bc` | Encrypted S3 v1 manifest and its objects |
| `af1a897.tar.gz` | `af1a897` | Two directory v1 copies with nine shared files, and an encrypted S3 v2 point with objects from three earlier copies |

Run these commands from `kernel-rs`. Set `BASELINE` to the directory containing the historical `kernel` and `konae` executables. Set `LAB_BIN` to the directory containing the disposable `with-minio` runner and `bin/mc`. Replay needs Python 3.12 or later and a filesystem with hardlinks.

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/target"
cargo build --offline --locked -p konae-backup --example catalog
cargo build --offline --locked --bins
"$LAB_BIN/with-minio" python3 backup/compatibility.py \
  --bin-dir "$CARGO_TARGET_DIR/debug" --baseline-bin-dir "$BASELINE" \
  --mc "$LAB_BIN/bin/mc" --require-s3
```

The script starts a new process for each read. It loads S3 into a fresh loopback MinIO, omitting the three older v2 manifests. For new copies it removes the live tree, including the writer cache, before opening either repository. Each binary reads points made by both versions of its writer. The test checks stdout, stderr, exit status, the SQLite row and the asset. It also checks the usage line for the revocation witness added after `af1a897`.

Without `--mc`, the script prints `SKIP S3 destination` and runs the directory cases. With `--require-s3` an omitted S3 destination fails the run.

## Installation history and crash campaign

Run from the checkout root with Rust 1.91.0, Python 3.11 or later, a filesystem with hardlinks, and the MinIO server and client (`minio` and `mc`) on PATH. The workflow pins the Linux binaries and their SHA-256 digests. Each invocation of the launcher creates credentials, starts MinIO on loopback and removes its bucket on exit.

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/kernel-rs/target"
export RUST_TEST_THREADS=2 TOKIO_WORKER_THREADS=2 PYTHONDONTWRITEBYTECODE=1
manifest=kernel-rs/Cargo.toml
cargo build --locked --release --manifest-path "$manifest" --bins
for binary in kernel konae; do
  python3 tests/smoke/with-minio.py python3 tests/smoke/recovery-year.py \
    --binary "$CARGO_TARGET_DIR/release/$binary" --s3 --year --seed 44 --report "$binary-year.json"
done
cargo test --locked --manifest-path "$manifest" -p konae-backup --features s3 --lib publication_after_a_short_write_or_failed_sync
cargo test --locked --manifest-path "$manifest" -p kernel --lib copies::tests::activation
cargo test --locked --manifest-path "$manifest" -p kernel --lib common_cut_timeout_and_abort_reopen_admission_in_drain_and_staging
```

The app advances one logical day per insert. Every third day a member deletes a row. Each restore starts the binary on a new local tree. Two members log in, query their own rows and request the same file IDs. The model records who wrote each row, the identities returned at signup, and the owners, visibility and bytes supplied to each import. One private ID changes owner and bytes. A public ID disappears and returns. The runner restores an earlier point after each replacement and after suspending a member. The final sweep removes the live tree and its MinIO file objects before checking every surviving point.

| Run | Clock | Publication and restore checks |
| --- | --- | --- |
| Short | 12 logical days | Each binary publishes five cuts to each destination. Every cut gets a restore, including the older cuts after import and suspension. |
| Year | 365 logical days | Each binary publishes on day 1, then every seven days through day 365. The copy IDs use the host clock. The runner checks both repositories against the 24 recent points and newest point of each UTC day. |
| Retention worker with `--year` | 365 days in the repository timestamps | The backup crate publishes 1,080 points per destination. The last 31 days have hourly cuts. Separate processes hold readers, lose writers and interrupt collection. A proxy withholds a point PUT until the SDK times out, refuses a PUT, drops a continuation token and refuses the second list page. |

Build the worker with the command in the V3 retention section and run the annual trial in a fresh bucket.

```sh
python3 tests/smoke/with-minio.py python3 tests/smoke/block-retention.py \
  --worker "$worker" --mc "$(command -v mc)" --require-s3 --year --report retention.json
```

Omit `--year` for the short app history. Omit `--s3` to run the directory alone. The runner prints the skipped destination and includes it in the report. With `--s3`, missing settings fail the run. Replay a failure with the reported seed, binary and length. The report names the logical day, cut day, operation, destination and first failed comparison.

For real ENOSPC, run `python3 tests/smoke/recovery-capacity.py --binary "$CARGO_TARGET_DIR/release/kernel"` inside a launcher that sets `LAB_SMALL_VOLUME` to a private mounted filesystem of at most 144 MiB. Repeat with `konae`. The runner fills that volume, checks the last advertised point, returns space and restores both generations.

Pull requests run the short history. The Sunday 03:17 UTC job also runs the annual retention trial. GitHub Actions keeps the aggregate JSON for 90 days. The laboratory exercises process death and real filesystem and MinIO operations. Power loss, elapsed retention time and access to a separately operated custody copy require an operator drill.

## V3 retention

From the worktree root, set `LAB_BIN` to the local lab tools and `ORACLE` to `retention-oracle.py`. The runner uses Python 3, MinIO, `mc` and a fresh bucket. Cargo needs the workspace dependencies in its offline cache.

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/kernel-rs/target"
set -o pipefail
worker=$(cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p konae-backup \
  --features s3 --lib --no-run --message-format=json | python3 -c \
  'import json,sys; print(next(x["executable"] for line in sys.stdin if (x:=json.loads(line)).get("executable")))')
"$LAB_BIN/with-minio" python3 tests/smoke/block-retention.py --worker "$worker" \
  --mc "$LAB_BIN/bin/mc" --measure "$LAB_BIN/minio-measure" --oracle "$ORACLE" --require-s3
"$LAB_BIN/with-small-volume" python3 tests/smoke/block-retention.py --worker "$worker" --capacity-only
```

Each destination receives 1,080 synthetic hourly points across two key epochs. The runner compares the listed survivors against the UTC calendar and checks exact bytes in a new process for each restore. The MinIO run measures creation, pruning, both restores, stable publication and the next recovery separately. It also exercises real pagination and injects a truncated response, a failed second page and a refused manifest PUT through a loopback proxy. The capacity run fills a disposable encrypted macOS volume until the directory writer hits ENOSPC. Running without `--mc` reports the skipped S3 destination. With `--require-s3` that omission fails the run.

To exercise the historical collectors, set `BASELINE` to the verified `af1a897` binary directory. The runner starts the historical Kernel on loopback, expires two old manifests per destination and restores the v3 point on each backend.

```sh
"$LAB_BIN/with-minio" python3 tests/smoke/block-retention.py --worker "$worker" \
  --historical-kernel "$BASELINE/kernel" --mc "$LAB_BIN/bin/mc" --require-s3
```
