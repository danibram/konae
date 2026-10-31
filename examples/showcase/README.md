# Showcase apps

Five apps built as product probes on 2026-11-06, and a sixth, starter, on 2026-11-07, each forcing one group of primitives that the [product-fit examples](../product-fit/README.md) predate: HTTP routes with signatures, API keys, the file read path, Signals and Usage, Invitation only signup, verification, Eject, copies and restore, the package export, and the Action Contract.

| App | What it is | Primitives it forces |
| --- | --- | --- |
| [inbox](inbox/README.md) | A private operations inbox fed by signed webhooks, a code host's and a payment provider's among them | Signed routes on a private app, provider signature schemes and `fields`, API keys, Signals, invitations, suspension, an email to the owner on a critical event |
| [club](club/README.md) | A members-only book club with private and shared files | File visibility and the file read path, roles in SQL, recovery, package export and import |
| [gallery](gallery/README.md) | A public portfolio with a contact form and a JSON feed | Public files under the CSP, anonymous rate limits, an open GET route, a multi-page artifact, a custom domain, the contract without a credential |
| [habits](habits/README.md) | A personal tracker used from two devices | Required verification, actor-bound rows, a daily schedule that closes the day, Signals and Usage, Eject, copies and restore |
| [booking](booking/README.md) | A room board with a public view and staff sign-in | Allowlist by domain, one atomic multi-row write, concurrent double booking, a confirmation email to the booker, a schedule that releases bookings nobody checked in, the contract with a session and with a key |
| [starter](starter/README.md) | The typical app in one, and the one to copy first: public landing and API, verified open signup, member, team and admin areas | Every primitive at once: public reads, key-only and signed routes, guards for roles, the file lifecycle and quota, Signals, the contract and conform, a package import, the CLI and the managed path |

Each app has a `replay.ts` that starts its own Kernel, publishes the app, runs every flow and refusal, runs `konae contract` and `conform`, and removes its data on exit. Run them all:

```sh
bun run examples/showcase/replay-all.ts
```

Each README ends with a table of what the primitives refused or could not do, with the exact message. The first five tables are consolidated in [the showcase findings](../../docs/research/showcase-findings.md); starter's stands in its own README. A passing replay with a long table is the expected result: the apps exist to find the edges.

[The capability matrix](../../docs/research/capability-matrix.md) judges ten common app shapes against 24 features, citing these replays and the [probes](../probes/README.md) that cover what they do not.
