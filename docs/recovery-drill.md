# Rehearse loss of the installation volume

Run the primary and independent restores on a clean macOS recovery host. Use a dedicated OS account with FileVault, encrypted swap and no other sessions under that account. Keep the live volume unmounted. The recovery host needs the server binary for the installation, Python 3.9 or later, `hdiutil` and `sandbox-exec`. The permissions probe also needs AWS CLI v2.

## Prepare the independent repository

The copy beside the live data disappears with that disk. Have a custodian in another account retain the copy objects, their manifests and the whole `copies/revocations/` prefix on independent storage. Include objects referenced by older home copy IDs. Retain healthy internal backup payloads before the source prunes them. A new replica of a point with missing payloads inherits the loss.

| Who | Access |
| --- | --- |
| Installation operator | Takes copies on the primary. The primary cleaner can delete objects there. Keep its credentials and account recovery authority out of the independent account. |
| Replication writer | Reads the primary copy prefix and writes new objects or versions into the vault. Deny delete markers, version deletion, governance bypass and changes to retention, lifecycle, roles and bucket policy. Keep replica retention independent of primary prune and delete replication. |
| Recovery reader | Lists and gets the retained copy objects and revocation record. Restrict listing by prefix and object access to that prefix. Deny writes and access to live `files/`. Its issuer, MFA and account recovery must survive loss of the installation operator's account. |
| Retention custodian | Owns versioning, retention and expiry. Use a separately approved credential for deletion after retention. Keep it off the writer and recovery hosts. |
| Key custodian | Keeps each generation of `KONAE_COPY_SECRET` in a separate vault and a sealed emergency store. Record the copy range and signing key for each generation. Test retrieval from an independent account. |

Versioned objects can have a new current version. Protect a dated inventory of object versions, manifest hashes and revocation entries in the custodian's account. Confirm replication through a recorded time **after** the selected cut. Compare the independent inventory with this receipt. If current objects differ, have the custodian present the recorded versions under their original keys in a separate recovery repository. Restore reads current objects by name. A signature detects alteration of an entry but cannot detect removal of the last suspension.

A rotation needs a separate repository or prefix and a ledger that the new key can authenticate. The reader uses one key per restore. Rehearse an old point with its old key and complete ledger, and a new point with its new key. Reconcile later revocations across the rotation under identity custody before reopening an installation. Keep both keys through the last retained point that needs them. After emergency use, retire the short-lived access, record who opened the key and return the emergency material to custody. Rotate storage access separately from the copy encryption key.

## Test effective delete permissions

With approval to create and try deleting disposable canaries, run the following on the custody side. Use profiles already issued by the provider. The writer puts a random 32-byte object under each of `copies/drill-canaries/` and `copies/revocations/`. Each name ends in `.canary`. Konae ignores it when parsing revocations. The probe tries a delete marker, explicit version deletion and governance bypass. Only an S3 `AccessDenied` counts. The probe also tries to overwrite and delete the canary as the reader. After both refusals the reader retrieves the same current version and bytes.

```sh
python3 ops/recovery-permissions.py --bucket "$VAULT_BUCKET" \
  --endpoint "$VAULT_ENDPOINT" --reader "$READER_PROFILE" --writer "$WRITER_PROFILE"
```

Keep the two canaries for normal retention. Check that retained manifests, referenced objects and ledger entries have the same effective deletion rules as the canaries. Record any policy exceptions. If the daily operator has no vault access at all, also verify its identity against the issuer and its delete denial on a custodian-created canary. The probe above tests a credential with write permission. Loss of the usual account requires a separate recovery login with its own custody chain.

## Run each restore

1. Obtain the operator-approved binary and verify its SHA-256 against the trusted release or build receipt. Use the binary for the installation profile, `konae` or `kernel`. Choose an explicit copy ID. From the independent custody receipt, take the SHA-256 of the exact stored bytes of an entry that suspended an account or revoked a key after this cut. The subject must exist and lack that revocation in the chosen copy. Directory and signed S3 entry bytes have different hashes.
2. For S3, the custodian supplies a dedicated loopback HTTP relay to **one** recovery endpoint. Restrict its upstream to the TLS-verified S3 service over the approved private channel. It must preserve the SigV4 host and request, or sign as the prefix-limited reader. Bind it on `127.0.0.1` inside the recovery host. A local S3 service with the retained versions also works. Give the drill only the existing reader's `KONAE_S3_ACCESS_KEY`, `KONAE_S3_SECRET_KEY` and the matching `KONAE_COPY_SECRET` through the credential broker. Keep credentials out of command arguments and shell history. The current Kernel takes a key pair and does not send an AWS session token. Provision a compatible reader or a broker that signs upstream with the short-lived session.
3. Run the primary exercise. For a directory, mount the copy repository read-only and use the directory form below. Treat unsigned directory copies as trusted custody inputs. For encrypted S3, use the S3 form with `--scenario primary`.
4. Remove the primary mount or stop its relay. Have the custodian verify that the clean host cannot access the primary, the live file store or the usual operating account. Run the independent exercise with its own relay, reader, key and witnessed entry. The scenario label records the operator's selection. Retain the separate evidence of account and failure-domain isolation.

```sh
umask 077
python3 ops/recovery-drill.py --scenario primary --material installation \
  --binary "$BINARY" --binary-sha256 "$BINARY_SHA256" --source "$READ_ONLY_MOUNT" \
  --copy "$COPY_ID" --witness "$DIRECTORY_ENTRY_SHA256"

python3 ops/recovery-drill.py --scenario independent --material installation \
  --binary "$BINARY" --binary-sha256 "$BINARY_SHA256" --source "s3://$VAULT_BUCKET/copies/" \
  --endpoint "$READER_RELAY" --region "$REGION" --copy "$COPY_ID" --witness "$SIGNED_ENTRY_SHA256"
```

The drill creates a fresh AES-256 sparse image with a random key held in memory. The mount and scratch directory sit under an owner-only directory. Set `--size-gib` if the restored installation and temporary SQLite files need more than the default 2 GiB. The restore child receives access to scratch and the selected read-only input. For S3 it may connect only to the relay port. The first S3 attempt withholds the decryption key and must leave the destination empty with the Kernel's encrypted-copy refusal.

The second attempt runs the installation's normal `restore` with `--require-revocation`. In staging, the Kernel finds the named entry in the record, verifies that it follows the cut and checks the subject before and after reconciliation. Failure discards staging. A separate `drill-check` process gets no repository credentials or network exceptions. It checks that the OS denies a TCP listener and an outgoing connection before loading the restored state. It invokes the health route in process. No user socket, mail worker, call worker, copy worker or schedule timer starts. The loaded Deployment count must match the restore summary.

The process detaches the image, checks the device list and removes the image before reporting success. If cleanup fails, keep the host restricted. Use `hdiutil info` under the drill account, match the image under its private `konae-drill-*` directory, detach that specific device and confirm its absence before removing the ciphertext. Follow the same steps after a host crash or a forced kill. Record the interruption and cleanup separately.

## Keep the evidence

Keep the exact copy ID, witness entry, object version list, operator approvals, independent account proof and binary receipt under custody. Share the JSON reports with the point fingerprint, binary digest, version, UTC time, elapsed seconds, counts, negative key test and volume removal. Any nonzero exit fails the exercise. Limits on claims matter here. Workerless health proves Kernel loading. Fresh login, a named action, private file access, nested backup restore and time to serve users need their own functional tests. Record whether the inputs came from a lab, a local app folder or a retained installation copy.

To repeat the disposable lab on a Mac, put the MinIO server and `mc` binaries in one directory and make AWS CLI v2 available on PATH. The test creates two stores on separate loopback ports, issues restricted test identities, takes a directory copy and a sealed S3 copy, then removes its source and primary before exercising the independent store.

```sh
RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 cargo build --offline --locked --manifest-path kernel-rs/Cargo.toml
python3 ops/tests/recovery-drill.py --minio-bin-dir "$MINIO_BIN_DIR"
```

For a separately authorized **local app folder**, stop its local writers or use an offline folder. The following check copies the app's sources, build, settings, Deployment record, row, identity and usage databases, journals and assets to the encrypted scratch volume. It compares file hashes, checks SQLite integrity and checks index entries against local blob sizes. Installation accounts, the secret store and MCP credentials are outside this check. Keep the folder path under custody.

```sh
python3 ops/tests/app-folder-check.py --folder "$AUTHORIZED_FOLDER" \
  --binary "$KERNEL_BINARY" --binary-sha256 "$KERNEL_SHA256"
```

## Pruning v3 copies

Each installation copy worker prunes its v3 destination after publishing a point. It authenticates the listed manifests with the current and historical data keys. It keeps the latest 24 points, the latest point on each UTC day within 30 days of the pruning clock, and the point just published. Before deleting anything it checks the name and stored length of every block reachable from those roots. Restore authenticates the block contents. The collector removes expired manifests first. It gives unreachable blocks 24 hours from their storage timestamp before collecting them. Loss of the installation's copy cache does not change these decisions.

All processes sharing an active v3 repository must use the reservation protocol. Upgrade or stop earlier v3 clients before enabling pruning. A directory must support shared and exclusive file locks on the repository directory across all clients. Writers and readers take shared locks. The collector takes an exclusive lock. Process exit releases the lock.

An active S3 destination must support conditional create, strong read-after-write and complete, strongly consistent LIST results across all clients. Pagination must enumerate every key present throughout the scan. A writer or reader creates a unique key under `users/` and checks `collectors/` before using blocks. A collector creates its own key under `collectors/`, checks for other collectors and checks `users/`. An incompatible reservation returns an error. The next attempt registers again. The worker reports a pruning error separately from a completed publication. A reader needs permission to create and delete its reservation and list collector reservations.

A missing continuation, repeated token, inconsistent key count, or failed page stops pruning. A known publication or referenced block missing from a purportedly complete listing also stops it. A compatible service that silently omits an unknown root or another client's reservation breaks the exclusion contract. The collector cannot discover that omission from this listing alone.

S3 reservations have no timeout. After process loss or an unconfirmed manifest write or delete, fence all clients of the affected prefix and confirm that pending storage requests have finished. Inspect the remaining entries in `users/` and `collectors/` and remove only reservations known to be abandoned. Start a fresh cycle with all retained data keys. Never resume an old deletion list. Keep lifecycle rules off the active objects, manifests and reservations. In a versioned bucket a DELETE removes the current key from the listing. The storage operator accounts separately for historical object versions.

To recover from an independent read-only S3 replica, have its custodian present a stationary prefix at the verified recovery time, including revocations. Mirror that prefix with the read-only credential into a private empty directory ending in `copies-v3/<repository-id>`. Check it against the custodian's recorded inventory and restore from the local directory with the required historical keys. This path needs no write permission on the replica. Mirroring a prefix while another client prunes it does not provide a snapshot.

## Change the installation copy writer

The release operator keeps the usual directory and S3 writers enabled while qualifying the reader. Deploy the candidate binary to the isolated recovery host first. Restore a known directory point and a signed S3 point by ID. Apply the revocation record, start the matching server, log in, call an action and read a private file. Verify that another app user cannot read it. Record the build commit, binary SHA-256 and times. Keep the historical binary, matching OS dependencies, all required key generations and the reconciled revocation record available through the expiry of the last copy that needs them.

For a shadow run, retain the existing writer settings and add `KONAE_COPY_V3_DIR` and `KONAE_COPY_V3_S3`. The directory ends in `copies-v3/<repository-id>`. S3 takes a different 32-character lowercase hex ID under `copies-v3/`. Give the historical process its own writable volume and legacy destination. Restrict its object permissions to the legacy prefix and its file-store needs. Test overwrite and delete on disposable objects and manifests under the actual v3 protection rules. Exercise its cleaner on expired legacy points.

Confirm that all four targets reported the same cut. Qualify each destination against the B11 inventory and app checks. Hold promotion if the pilot fails, the common cut fails for either writer, the latest remote cut is missing or stale, a restore fails, an inventory difference lacks a classification, an integrity alert remains open or a budget is exceeded. The pilot targets are 65 minutes for remote cut age, 30 minutes to usable service on the 292 MB profile, 250 ms additional p99 write wait and two seconds for the capture barrier. Use the budgets agreed before the pilot. A verified lab restore cannot clear a failed pilot.

### Return to the historical executable

1. Stop the candidate copy worker and isolate writes on the affected installation. Preserve the repositories, key generations and revocations through the incident. Turning off the v3 settings keeps the integrated binary and its common cut. A failure in that cut can still prevent the usual writer from publishing.
2. Read the retained manifests under the legacy directory or `copies/` prefix. Order complete points by cut time, newest first. Select the newest point that the exact rollback build can verify and restore into an empty volume. Verify its internal backup history and apply later revocations. Preserve any refused point and record the reason if an earlier point is needed. A v3 point requires a v3 reader. If there is no verified, sufficiently recent compatible point on the surviving destination, hold the handoff and keep service closed.
3. Record the actual cuts. Subtract the compatible cut from the newest verified cut to measure the additional interval lost by choosing the historical reader. Subtract the compatible cut from incident time for the full potential loss. Measure RTO from the empty recovery host with access to custody through reconciliation, fresh login, an action and private file access. Confirm denied access as well. Keep the rollback process under the tested permissions after reopening.
4. While v3 is primary, keep making compatible points and delivering revocations to both generations until retirement is approved. A rehearsal of a v3-only interval measures what would be lost if compatible writes stopped.

### Record the decision

Restore a recent point each UTC day. Each week restore the oldest retained point. Alternate directory and S3 within each series. Record successes and failures, including missed runs. Alert on a missed deadline. After a prune, read each destination's retained list and recover its oldest and newest points. For retirement, repeat both restores after a prune at the end of at least 30 elapsed days of operation. Use the separate custody account and emergency key for an independent restore. A coordinated restore on active v3 needs list and get access plus put and delete on that repository's `users/` reservations. A list/get-only reader can mirror a stationary replica, including its revocation record, then restore the mirror offline.

Operations and maintenance keep a restricted JSON receipt and sign the same bytes. Take the public keys for both roles from the installation's trust store. Keep raw IDs, signatures, trusted keys and source reports under custody. Share aggregate counts, build digests, cut differences and timings. The receipt fields and time units are illustrated by `example()` in `ops/tests/writer-cutover.py`. Operators attest the complete B11 report, inventory, alerts, actual build and role, custody, permission checks, observed retained lists and drill results. A later success requires disposition of any earlier failure before a new release review.

Keep the publication journal outside the repositories. Append each destination's confirmed cut ID to it when the writer publishes. At the prune boundary, seal that journal and list the surviving roots independently on each destination. Include both lists in the signed receipt. The review keeps the newest 24 and the last cut of each UTC day whose age at pruning is at most 30 days. It compares the complete expected set with the observed set and checks gaps against the agreed RPO. For legacy points, record the format from each selected manifest. Bind each restore to its repository, copy format, reader commit and SHA-256. Keep the frozen inventory with both endpoint restore records.

Use `openssl dgst -sha256 -sign` with each role's signing mechanism. Store the detached signatures beside the receipt as `<receipt>.operations.sig` and `<receipt>.maintenance.sig`. Run the read-only review on the recovery host with a trusted UTC clock. Exit 1 holds the change. The review checks the signed record and calendar. Operations and maintenance verify its source evidence and own the release decision.

```sh
python3 ops/writer-cutover.py promote "$RECEIPT" \
  --operations-key "$OPS_PUBLIC_KEY" --maintenance-key "$MAINT_PUBLIC_KEY"
python3 ops/writer-cutover.py retire "$RECEIPT" \
  --operations-key "$OPS_PUBLIC_KEY" --maintenance-key "$MAINT_PUBLIC_KEY"
```

### Repeat the synthetic rehearsal

Use a macOS checkout, Python 3.11 or later, OpenSSL, AWS CLI v2, `sandbox-exec` and a fresh loopback MinIO for each binary. The launcher provisions an empty `konae-test` bucket, exports `KONAE_TEST_S3_*` and the matching `mc` alias `lab`, and removes the server after the command exits. The rehearsal creates a second bucket and restricted identities on that disposable server. The primary writer and the historical cleaner must each receive an S3 AccessDenied when they try to overwrite or delete objects in the second bucket. Independent account recovery and loss of the S3 provider need a separate operational exercise.

Build with the same optimized profile as the historical binaries. Set `BASELINE` to the directory of release artifacts. Each full commit directory contains `kernel`, `konae` and the trusted `binaries.sha256`. The rehearsal checks the exact v0.10.0 commit and the later af1a897 base. Use an empty restricted output directory.

```sh
set -eu
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR="$PWD/kernel-rs/target"
build_commit=$(git rev-parse HEAD)
cargo build --locked --offline --release --bins --manifest-path kernel-rs/Cargo.toml
python3 -B ops/tests/writer-cutover.py
for binary in kernel konae; do
  "$WITH_MINIO" python3 -B ops/tests/writer-rollback.py --binary "$CARGO_TARGET_DIR/release/$binary" \
    --build-commit "$build_commit" --baseline "$BASELINE" --mc "$MC" --pilot "${PILOT_RESULT:-pending}" \
    --report "$REPORTS/$binary.json"
done
```

The rehearsal advances the drill schedule through 30 synthetic days. The writers, pruning, clocks used for cuts, restore commands, access checks and permission refusals run on the host. The signed lab receipt and its signing keys expire with the fixture. The aggregate report records the refusal to retire, the cut difference, per-destination timings and S3 server traffic by phase. Each emergency RTO includes the full replica download. Run the two profiles outside another pilot's latency window.
