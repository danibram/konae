# Integration verification

Date: 2026-11-07. Base commit `89ba91d`, with the changes described below in the working tree.

## Fixes

The schedule runner and the outbound-call worker's `then` runner accepted `notify` on their mutations but never processed it. Both now call the existing notice recorder after a successful mutation changes rows. They use the same mail queue, creator, app and recipient budgets, and Usage outcomes as an HTTP or runtime mutation.

A notice to `owner` reaches the project owner. Neither runner has a caller, so a notice to `actor` records `no-account`. A scheduled run queues at most one notice regardless of how many rows changed. Templates still take only declared inputs and the actor ID; a schedule supplies neither row values nor a caller.

The schedule probe failed before the fix with zero notices instead of two. It now checks two changed runs, two owner notices, their Usage counts, and no extra notices from unchanged runs, failures or maintenance. The calls probe failed before the fix while waiting for notices. It now checks three owner notices from changed `then` mutations with their mapped input values, plus `no-account` for an actor notice.

The capability matrix had marked outbound retries impossible even though the call worker already retries after 30 seconds, 1, 2 and 4 minutes, up to five attempts. Its integration-hub F18 cell now records that fixed calendar as a compromise. Counts recomputed from the cells are 146 works, 94 compromises and 10 impossible. None of the 88 required cells is impossible. All ten impossible cells are third-party login.

The findings report contained a second copy of the whole report and stale conclusions about schedules, roles and parser parity. It now has one copy and the current matrix results. References left at ADR-0014 by independently developed branches now name their accepted ADRs. The SDK contract, generated agent prompt, Kernel guide and SDK documentation describe the notice behavior.

## Checks

| Check | Command | Result |
| --- | --- | --- |
| Full Bun suite after the fixes | `bun run test` | 875 passed, 0 failed |
| Capability probes after the fixes | `KONAE_PROBE_BASE_PORT=24600 bun run examples/probes/replay-all.ts` | All 10 passed, 61 checks |
| Kernel suite after the fixes | `cargo test --manifest-path kernel-rs/Cargo.toml` | 256 passed |
| Parser, Gate, conformance and generated SDK readers after the fixes | `bun test src/sdk/contract.test.ts src/kernel/actions.test.ts src/agent/gate.test.ts src/lib/action-conformance.test.ts` | 230 passed |
| Editor type check and production build after the fixes | `bun run build` | Passed; Vite retains its chunk-size warning |
| Exported kit after the fixes | `bun run tests/smoke/portable-export.ts --port 24520` | Passed, including fresh-kit compilation and package import |
| Local browser flows | `KONAE_EDITOR_URL=http://127.0.0.1:4398 KONAE_TEST_KERNEL_PORT=8898 bun run test:e2e:local` | 8 passed |
| Showcase apps | `KONAE_SHOWCASE_BASE_PORT=24100 bun run examples/showcase/replay-all.ts` | All 6 passed |
| Product-fit examples | `KONAE_PRODUCT_FIT_PORT=24310 bun run examples/product-fit/smoke.ts` | 12 scenarios passed; 175 conformance checks passed, 1 skipped guard |
| Migration and restore rehearsal | `bun run tests/smoke/migration.ts --port 24330` | Passed, including restore into an empty directory and retained revocations |

The browser, showcase, product-fit and migration runs preceded the notice-runner fixes. The full Bun suite, all ten probes, Kernel suite, parser and SDK checks, and exported-kit compilation were repeated after those changes. The two added notice checks run on real Kernels with the development mail transport.

## Limits

The schedule's SQL write, recorded slot and mail job use separate transactions. A crash after the slot is recorded but before its notice is queued can lose that notice. This change does not provide transactional delivery across those stores.

No check drives the editor's build agent, sends email to a real inbox, exercises real provider credentials, proves a real custom domain through public DNS and TLS, or deploys to a production host. This session did not repeat the Docker stack check. The matrix retains its stated compromises for row-based recipients, digests, search, polling and payment-provider checkout creation.
