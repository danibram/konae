# Recovery drill on mbpd, 18 November 2026

The installation exercise used disposable identities, one Deployment and two native MinIO servers. Each server had its own storage directory and administrator. The independent server received the sealed objects and later revocation through an append credential. The test deleted its live tree, stopped the primary server and removed the primary repositories before the independent restore. Both servers ran on mbpd.

| Issue criterion | Observed on mbpd | Evidence still due from the operator |
| --- | --- | --- |
| Independent custody and effective permissions | MinIO refused six writer delete requests and four reader writes across two disposable objects. The reader retrieved both original versions and bytes. A deletion-capable administrator failed the negative control. The reader could not get a known object under the live file prefix. | Verify the daily principal on the real repository, the policies on retained objects and ledger entries, account separation, retention and emergency access. Exercise key rotation across retained points. |
| Restore after loss of the volume and primary | Directory, primary S3 and independent S3 restores recovered the same point. Each verified eight files, loaded one Deployment and applied one owner suspension. The Kernel checked the subject in staging before and after reconciliation. The independent restore ran with only the replica reader and custody key. | Restore a real retained installation on a separate clean host using only the independent account. Match the full ledger to the custodian's independent receipt. |
| Isolation, missing key and disposal | The OS refused reads of an unrelated file and connections to a second listening port. Before loading the recovered Kernel, the OS denied both TCP bind and outbound connect. A due email and call stayed pending with zero attempts. The overdue schedule produced zero writes. The encrypted-copy refusal with the key withheld left the destination empty. Every trial detached the AES-256 image and removed its backing file. | Repeat under the installation's recovery host, encrypted storage and access controls. Full user-facing recovery time, fresh login and private file authorization require separate tests. |

## Timed installation trials before the inventory integration

All eight trials used point fingerprint `a61be63bad72004e263cab820ff612c53d70a6504643d6c79265ec981ba2e610` and Konae `0.10.0`, binary SHA-256 `ca1b2fb24dc58857be6534d9fe49a3375c670716dca47ff1bbf802c28b400dc9`. Durations include volume creation and removal.

| UTC start | Source | Outcome | Seconds |
| --- | --- | --- | ---: |
| 02:59:14 | primary directory | passed | 4.469 |
| 02:59:19 | primary sealed-s3 | passed | 5.475 |
| 02:59:30 | independent sealed-s3 | passed | 5.762 |
| 02:59:36 | independent sealed-s3 | custody-key-unavailable | 4.001 |
| 02:59:40 | independent sealed-s3 | wrong-custody-key | 5.183 |
| 02:59:46 | independent sealed-s3 | missing-witness | 5.447 |
| 02:59:52 | independent sealed-s3 | unsigned-input | 5.213 |
| 02:59:57 | independent sealed-s3 | record-unavailable | 5.072 |

## Local app folder

One authorized app folder already present on mbpd had a publication receipt. Its sources, build, settings, Deployment record, row, identity and usage databases and local file bytes stayed on that machine. The check compared **365 files**, ran SQLite integrity checks on **3 databases**, verified **259 indexed assets**, and loaded **1 Deployment** with network access denied. It removed the encrypted volume after **42.599 s**. The Kernel reported version `0.10.0`. Its binary SHA-256 was `49cc32d98641e358fe3b6f7ea6a5983afc3489fae04a8473243ec46741c5d90e`.

The folder had no pull receipt establishing a production cut. No production installation copy, emergency reader or independently witnessed production revocation was supplied. The app check did not contact production or exercise production recovery credentials. The integration owner has the local custody reference.

## Reproduction

Use the lab and app-folder commands in the [operator guide](../../../docs/recovery-drill.md). For the regression and documentation checks, run the following at the repository root.

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib recovery::tests
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib copies::tests -- --test-threads=2
bun install --cwd examples/docs-site --offline --frozen-lockfile
bun test --timeout 60000 scripts/docs-site.test.ts
```

All 5 recovery tests, 37 copy tests and 10 documentation tests passed. The default five-second documentation timeout expired once during concurrent builds on mbpd. The command above completed all 503 documentation assertions.

## Rerun with the installation inventory

On integration commit `719284c`, MinIO passed the permission probes and the job fixture stopped with `no such table: calls`. The installation had never queued a call. The corrected fixture checked that its first health read left the optional outbox absent, created the queue with the Kernel schema, then inserted a due call. The next health read left the call and email pending with zero attempts. The schedule made zero writes.

The rerun used binary SHA-256 `10e23519eaae4900462ccc3896379b19319e1a65193b5e47d32a9cb3678ffa5b` and point fingerprint `5b3b84acdd84cb2bc9d9e7a2f254feebb255a7a2e70825fe68dd0799603af88f`. All three restores verified seven files and applied the witnessed suspension. MinIO denied six writer deletes and four reader writes. The sandbox denied the unrelated file read and connection. All eight trials detached the encrypted volume and removed its backing file.

| UTC start | Source | Outcome | Seconds |
| --- | --- | --- | ---: |
| 03:15:16 | primary directory | passed | 5.361 |
| 03:15:22 | primary sealed-s3 | passed | 5.722 |
| 03:15:32 | independent sealed-s3 | passed | 5.677 |
| 03:15:38 | independent sealed-s3 | custody-key-unavailable | 5.061 |
| 03:15:43 | independent sealed-s3 | wrong-custody-key | 5.229 |
| 03:15:49 | independent sealed-s3 | missing-witness | 4.317 |
| 03:15:53 | independent sealed-s3 | unsigned-input | 5.234 |
| 03:15:59 | independent sealed-s3 | record-unavailable | 5.300 |

Repeat from the repository root after building the integrated binaries.

```sh
python3 -B ops/tests/recovery-drill.py --minio-bin-dir "$MINIO_BIN_DIR"
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p kernel --lib calls::tests::test_calls_are_captured_across_restart_and_replies_run_then_once
cargo test --offline --locked --manifest-path kernel-rs/Cargo.toml -p konae --test inventory
```
