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
