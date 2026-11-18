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
