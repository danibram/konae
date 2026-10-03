# Showcase apps

Five apps built as product probes on 2026-11-06, and a sixth, starter, on 2026-11-07, each forcing one group of primitives that the [product-fit examples](../product-fit/README.md) predate: HTTP routes with signatures, API keys, the file read path, Signals and Usage, Invitation only signup, verification, Eject, copies and restore, the package export, and the Action Contract.

| App | What it is | Primitives it forces |
| --- | --- | --- |
| [inbox](inbox/README.md) | A private operations inbox fed by signed webhooks | Signed routes on a private app, API keys, Signals, invitations, suspension |
| [club](club/README.md) | A members-only book club with private and shared files | File visibility and the file read path, roles in SQL, recovery, package export and import |
| [gallery](gallery/README.md) | A public portfolio with a contact form and a JSON feed | Public files under the CSP, anonymous rate limits, an open GET route, a multi-page artifact, the contract without a credential |
| [habits](habits/README.md) | A personal tracker used from two devices | Required verification, actor-bound rows, Signals and Usage, Eject, copies and restore |
| [booking](booking/README.md) | A room board with a public view and staff sign-in | Allowlist by domain, one atomic multi-row write, concurrent double booking, the contract with a session and with a key |
| [starter](starter/README.md) | The typical app in one, and the one to copy first: public landing and API, verified open signup, member, team and admin areas | Every primitive at once: public reads, key-only and signed routes, guards for roles, the file lifecycle and quota, Signals, the contract and conform, a package import, the CLI and the managed path |

Each app has a `replay.ts` that starts its own Kernel, publishes the app, runs every flow and refusal, runs `littlebox contract` and `conform`, and removes its data on exit. Run them all:

```sh
bun run examples/showcase/replay-all.ts
```

Each README ends with a table of what the primitives refused or could not do, with the exact message. Those tables are consolidated in [the showcase findings](../../docs/research/showcase-findings.md). A passing replay with a long table is the expected result: the apps exist to find the edges.
