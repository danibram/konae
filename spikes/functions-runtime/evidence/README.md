# Functions evidence

The code under `kernel-rs/` and `src/` is the current platform implementation. This directory records experiments, test runs and build evidence. A passing spike does not replace a check against the current code.

The IPC/2 request-body records describe macOS arm64 runs. They show the 100000000-byte boundary, chunked HTTP handling, bounded pending chunks, cancellation and compatibility with earlier idempotency receipts. They do not show IPC/2 streaming under Linux.

The Linux V8 and image hardening records predate IPC/2. They cover the earlier worker protocol, cgroup limits, Landlock, seccomp, image entrypoints and architecture checks. They do not validate the new streaming transport on Linux. Functions remain experimental and have no production deployment evidence.

This checkout retains text logs, commands, manifests and source hashes that explain the recorded runs. Git omits captured source archives, compiled binaries, screenshots, profiles, local app state, SQLite files and dependencies. Those files remain on the original machine. Text transcripts have trailing spaces and extra blank lines trimmed; the earlier evidence commits retain the original captures. Some provenance records name private temporary paths or paired binaries in another thread's storage. Those paths are local references and are not part of the portable evidence set.
