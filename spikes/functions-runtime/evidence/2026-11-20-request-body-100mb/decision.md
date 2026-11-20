# Request body decision and evidence

The Kernel counts and hashes HTTP body bytes as Axum yields them. It sends 64 KiB binary IPC chunks only when Hono pulls from `Request.body`. A two-item channel and an 8 MiB global pending-byte semaphore bound retained chunks. The path does not spool a complete request to disk or allocate a 100 MB `Vec`.

The producer test passed for exactly 100000000 bytes and rejected 100000001. The raw TCP HTTP/1.1 test sent chunked requests without Content-Length. Hono counted and checksummed exactly 100000000 bytes; the 100000001-byte request returned 413 with `The function request exceeds 100000000 bytes.` A direct Kernel request also returned 413 when its false Content-Length claimed one byte.

The same HTTP test held three independent chunked uploads open with 64 KiB pending in each, then completed a small request and another small request after cancelling the uploads. The debug-runner sample showed 38224 KiB for the Kernel test process and 158960 KiB summed across four worker processes. The final rerun used the release runner and showed 33136 KiB for the Kernel test process and 133664 KiB summed across workers. macOS reports RSS in KiB; each sum can count shared pages once per process. These are samples, not peak RSS or a memory ceiling. The debug-runner boundary and concurrent test took 31.81 seconds; the release-runner run took 11.78 seconds on this macOS arm64 host.

Both HTTP measurements ran the in-process Kernel test harness from Cargo's debug test target. The final boundary run used the release runner binary. The separately built release Kernel binary was inspected and hashed, but this local HTTP test did not launch it. Kamynando will run its HTTP test with the paired release binaries.

The worker kept its 64 MiB heap and 256 MiB RSS limits. `request.json()` parses inside that heap and may fail for bodies that the transport accepts. Handlers that must process larger bodies should consume the Web stream incrementally. Action operations have a separate serialized JSON cap of 1 MiB; 64 operations, SQL rules, row/result limits, response bytes and file limits remain independent.

New receipts use the versioned v3 method/path/headers plus body length and streaming body digest. The v2 fingerprint code remains unchanged. A v2 prelookup replays an existing receipt without starting a worker and retains at most the original 16 KiB ingress body while computing its original fingerprint. Larger requests cannot match an old receipt, so the Kernel counts them without retaining bytes and returns a conflict after EOF. A race after prelookup is rechecked transactionally before a new mutation.

The producer test also covered three stalled upload streams, a small producer completing while they were pending, cancellation cleanup, deadline failure and v2 retention boundaries at 16 KiB. Each stalled stream retained at most three 64 KiB chunks at the producer and the pending-byte semaphore returned to its full 8 MiB after cleanup.

The real HTTP action integration passed a 70 KB JSON body through `request.json()`. Large v3 receipt replay returned the saved response, a concurrent same-key replay produced one database write, a changed body conflicted, and a simulated v2 receipt replayed with its original fingerprint. The early-action and caught-early-action cases both failed before any database mutation. The frozen IPC v1 runner was rejected, reaped and latched without another spawn; the v2 runner rejected an IPC v1 Init before evaluating bundle source.

No request payloads are recorded here. No app migration, deploy, commit, push, or frozen binary changes were made.
