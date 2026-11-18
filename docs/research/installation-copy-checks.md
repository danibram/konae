# Installation copy checks

On 18 November 2026, mbpd ran the copy writer and reader against temporary directories and a MinIO bucket on loopback. The fixtures generated the accounts, files, keys and rows. The tests used Rust 1.91.0 and the locked dependencies.

| Check | Observed result |
| --- | --- |
| Indexed blob missing, truncated or replaced with bytes of the same length | The copy failed with the file ID. The preceding complete point still restored the original bytes. With an older index and no local copy cache, the preceding directory manifest detected the substitution. |
| Entry deleted before capture | The next complete copy contained an empty index and omitted the leftover blob. A legitimate import that reused an ID recorded a new digest and restored the new bytes. |
| Damage at a destination | Removing, shortening or replacing a reused directory file blocked publication. Damage to a reused encrypted S3 object left the directory target free to advance. Plain S3 also detected a replacement of the same length. Repairing the bytes allowed the next copy to complete. |
| Final manifest size | For directory v1, S3 v2 and signed S3 v2, the encoder and reader accepted 4,194,303 and 4,194,304 bytes. Both refused 4,194,305. The publication paths rejected the oversized manifests before exposing a new point. In the signed case the unsigned representation still fit. |
| Directory file types and manifest paths | Reuse refused a file link and a directory link pointing to ordinary files inside the fixture. The last good point still restored. The writers and restore gave the same reason for invalid paths and object references. |
| Hourly copies with a stopped worker | At 3,900,000 ms from the start of capture, health still reported a fresh copy. At 3,900,001 ms the target alerted without another attempt. Cancelling the worker during capture and reopening its status retained the interrupted attempt and the last complete point. |
| Restore and process restart | A CLI restore recovered the original bytes. The running Kernel reported its point and completion time over HTTP. After a forced process restart a damaged source failed the next attempt. The last success and restore receipt survived. A second restart with the original bytes produced a newer point and cleared the alert. |

## Reproduce from the repository root

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib copies::tests
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib package::tests
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib upload_refuses_to_overwrite_an_unreadable_file_index
cargo build --offline --locked --manifest-path kernel-rs/Cargo.toml --bin kernel
python3 tests/smoke/copy-health.py kernel-rs/target/debug/kernel
```

The tests under `copies::tests::integrity` use an in-process S3 server by default. To repeat them on MinIO, supply the endpoint, bucket and credentials of a disposable server through `KONAE_TEST_S3_ENDPOINT`, `KONAE_TEST_S3_BUCKET`, `KONAE_TEST_S3_ACCESS_KEY` and `KONAE_TEST_S3_SECRET_KEY`. Each run uses a fresh prefix. The mbpd lab's `with-minio` command creates the bucket, supplies these variables and removes the server and its storage when the command exits.

```sh
with-minio cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib copies::tests::integrity
with-minio bun test tests/copy-restore.integration.test.ts
```

The historical limit is the first trusted hash. If an old index has only a size and neither the cache nor a target manifest has a hash, a first capture can detect a missing or truncated blob. A same-length replacement made before that capture needs an independent reference. The copy checks read each indexed blob and each destination file. This run did not measure the cost at installation scale. The RPO calculation uses the beginning of capture as the conservative cut time across the installation.
