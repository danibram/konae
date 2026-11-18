# Compare the installation copy writers

Run on an idle macOS lab host. Use Python 3.11 or later, Bun, Rust 1.91.0, the locked Cargo dependencies, MinIO and its matching mc client. Set PATH to include both MinIO executables. Each remote run below starts a separate loopback server with generated credentials and an empty bucket. The tests use disposable synthetic accounts. Keep the source fixture stopped.

## Build and seed

Run these commands from the integrated checkout. The historical build supplies the v0.10.0 control. The integrated release writes both formats. Keep the binary hashes with the measurements.

```sh
export RUSTUP_TOOLCHAIN=1.91.0 CARGO_BUILD_JOBS=2 TOKIO_WORKER_THREADS=2 GOMAXPROCS=2
export CARGO_TARGET_DIR="$PWD/kernel-rs/target"
export PYTHONDONTWRITEBYTECODE=1
umask 077
lab=$(mktemp -d)
mkdir "$lab/released" "$lab/reports"
git archive af1a897175ff93a7ce43cb0ffe29fd30a122f541 kernel-rs | tar -x -C "$lab/released"
cargo build --offline --locked --release --manifest-path "$lab/released/kernel-rs/Cargo.toml" \
  --target-dir "$lab/historical-target" --bin konae --bin kernel
cargo build --offline --locked --release --manifest-path kernel-rs/Cargo.toml --bin konae --bin kernel
old="$lab/historical-target/release"
bin="$CARGO_TARGET_DIR/release"
shasum -a 256 "$old/kernel" "$old/konae" "$bin/kernel" "$bin/konae"
six="$lab/six-apps"
bun ops/tests/seed-six.ts "$old/konae" "$six"
python3 ops/tests/fill-six.py "$six"
```

The generator publishes six apps through MCP. Each has two app users, a row and an uploaded private file. With the process stopped, the filler adds 249 private blobs and writes an external size and SHA-256 oracle. Account and uploaded-file IDs are assigned on each run. The seeded fixture can also be supplied with `--six`.

## Capture and restore

```sh
minio_run() { python3 tests/smoke/with-minio.py "$@"; }
pilot() { ops/tests/writer-pilot.sh --mc "$(command -v mc)" "$@"; }
remote() { minio_run ops/tests/writer-pilot.sh --mc "$(command -v mc)" "$@"; }

remote --binary "$bin/konae" --profile draft --store unversioned --report "$lab/reports/draft.json"
remote --binary "$bin/kernel" --profile standalone --store versioned --second-cut 12 --report "$lab/reports/kernel.json"
pilot --binary "$bin/konae" --profile six --six "$six" --store local --report "$lab/reports/local.json"
for store in unversioned versioned replay; do
  remote --binary "$bin/konae" --profile six --six "$six" --store "$store" --report "$lab/reports/$store.json"
done
remote --binary "$old/konae" --historical --legacy-s3-only --profile six --six "$six" \
  --store unversioned --report "$lab/reports/historical.json"
```

The script requires the exact set of destinations for each case. It prints skipped destinations in local and single-writer runs. A missing MinIO endpoint, failed destination, inventory discrepancy or missing access check fails the run. Use a new report path for each attempt.

Both integrated writers receive one staged image. The driver checks the point ID and cut timestamp. A local live file store can publish to both directory formats. Configuring the S3 copy bucket also selects S3 for live files. The `replay` case captures local files once and passes that directory point to the two S3 writers through an ignored Rust lab test. Replay measures the cold format writers and readers independently of the S3 live cut. It does not run the scheduled S3 writer or its pruning pass.

The driver inventories the quiescent fixture before moving the live blobs, removes the live tree and live S3 versions after capture, then checks each restore in a new directory. RTO starts just before the restore command and ends after login, row access and private downloads, including permission denials. The CLI time includes download, reconstruction, invalidation, reconciliation and activation. The lab host and keys are already available. The separate raw reader reconstructs the point again before invalidation. Its schema, user version, typed row hashes, counts, paths and file hashes must match the external inventory. WAL and journal contents enter through SQLite. The live backend marker, locks, staging, transfer scratch, copy status and cache, transient Google flows and the operator mail key have separate policies in the installation inventory.

The historical writer formats restored file indexes again. Its comparison reports how many byte hashes changed and checks every field and array position. The integrated comparison requires identical bytes.

The six-app case checks the 255 files against their external hashes, lengths and private index entries. It also checks the owner catalogue and both users in every restored app. The draft case checks three published projects, an unpublished editor project and stores that were absent. The standalone Kernel restores app accounts without an owner catalogue. The runtime's inventory command runs on each source and restored tree. Use the separate encrypted, workerless app-folder check for an operator-authorized local folder. Its result does not establish an installation cut. [Recovery drill](../../docs/recovery-drill.md)

## Measurements

```sh
python3 ops/tests/writer-latency.py --binary "$bin/konae" --six "$six" --report "$lab/reports/latency-local.json"
minio_run python3 ops/tests/writer-latency.py --binary "$bin/konae" --six "$six" --store unversioned \
  --interval 15 --mc "$(command -v mc)" --report "$lab/reports/latency-s3.json"
```

For the versioned-origin load test use `--store versioned --interval 70` and another empty MinIO. For the historical S3 comparison use the release under `$old`, `--store unversioned --interval 60` and `--modes control legacy`. Choose an interval longer than the first capture for a writer expected to publish. Each trial uses a fresh clone, six authenticated clients and 216 sequential writes. The driver checks each mutation through its query action, waits 55 ms, then moves to the next client. A timed write includes the MCP round trip. The report keeps the observations in operation order, the nearest-rank p99, its difference from control and the p99 of differences at matching positions. The latter is a paired latency estimate across runs. Each case records its UTC start and end and the host load averages at both times. The server log provides the measured barrier and drain. A refused capture keeps its latency samples and fails the publication criterion.

MinIO Prometheus deltas count requests and traffic per phase. Server trace records split shared publication traffic by prefix. The trace and Prometheus count different portions of the protocol. Directory repository size counts logical file bytes. Neither counter measures physical SSD reads. The sampler sums live SQLite WAL file lengths and staging file lengths and counts the writer's open descriptors about every 100 ms. Reported maxima are sampled lower bounds. The stage budget is 1 GiB and the reader budget is 512. Apply the operator's unit prices to the operation counts, retained byte-months and chargeable traffic. Supply an independent failure domain, custody keys, remote alerting and elapsed retention time for a production decision.

## Check that a slow GET stays outside the cut

```sh
remote --binary "$bin/kernel" --profile standalone --store versioned --legacy-s3-only --delay-file-get \
  --report "$lab/reports/one-file.json"
```

The relay holds the live GET of one 512-byte private file for 2.2 s. The test sends a write when the GET reaches the relay and requires a response within 250 ms. The cut must follow that write. Restore the point after removing the live files and check that the acknowledged row and all file bytes and permissions survived. Repeat with `--store unversioned` in an empty MinIO.

Use `--profile six --s3-only` to run v2 and v3 from one cut and restore each from S3. A new process with an empty repository starts cold. Use `--second-cut 95` to measure a second publication in the same process. Read preparation, pause and materialization separately in the report, and compare the live file GET and HEAD counts between publication phases.

On a versioned store, the process can keep one generation of verified blobs in the private staging directory. The next attempt checks the current version and length with HEAD and hashes the local bytes before reuse. A missing version, including the S3 version ID `null`, requires a full GET. Removing staging or restarting the process makes the next capture cold.
