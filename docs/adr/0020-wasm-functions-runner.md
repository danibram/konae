# Build StarlingMonkey functions on the destination

Status: Proposed.

An app leaves Konae with the same sign-in and data rules. The editor and CLI both send ESM, including the dependencies each function needs. Startup on the destination must work with the released files even when the saved draft has changed. The owner selected StarlingMonkey as the production runner path on 2026-11-23.

## Decision

Keep the published `konae-functions/1` format. The app folder carries the declared ESM files in `server/` with their SHA-256 hashes and released metadata. The Kernel prepares the version with a pinned Rust helper before release, source import or serving an exported folder. IPC/3 selects a function by name. Use per-bundle snapshots until a combined component can keep each function's JavaScript realm private. One shared realm changes the globals a function sees. The destination owns the component and native compilation cache. Cache misses never compile in the request path.

The offline Gate checks the ESM artifact. For an online function change it also calls the same preparation check the Kernel runs on publish. Both report the same refusal code and sentence. Build diagnostics do not replace the refusal contract. Failed preparation leaves the prior release available.

An operator flag selects the Wasm launch policy before spawn. IPC/3 Ready confirms the kind. Wasm workers keep separate Linux identities, rlimits, RSS polling and deadline kill and reap. Docker's default security profiles apply. The Wasm path requires no cgroup delegation or privileged mounts.

## Consequences

The destination builds the snapshot and native code once for each prepared version. This adds work before serving a moved app and keeps the export small. Carrying a portable component would add an engine snapshot to each version and require a separate way to trust its relationship to the ESM. A separate Node build in each door would also split the preparation contract.

Require 1.5× or less of deno_core with snapshot on warm GET, warm POST and the slowest repeated Hono case. Require the cold maximum to stay under 250 ms after preparation. Run on stock Linux with the default container profiles before adopting this launch policy. [Production design and measurements](../architecture/functions-wasm-production.md)
