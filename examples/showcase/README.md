# Showcase apps

These six apps exercise the pieces of the Kernel that the [product-fit examples](../product-fit/README.md) do not reach: HTTP routes with signatures, API keys, the file read path, Signals and Usage, Invitation only signup, verification, copies and restore, export and import, and the Action Contract. Starter puts them all in one app and is the one to copy first.

| App | What it is | What it exercises |
| --- | --- | --- |
| [inbox](inbox/README.md) | A private operations inbox fed by signed webhooks, a code host's and a payment provider's among them | Signed routes on a private app, provider signature schemes and `fields`, API keys, Signals, invitations, suspension, a notice to the contact email on a critical event |
| [club](club/README.md) | A book club for invited readers, with private and shared files | File visibility and the file read path, roles in SQL, recovery, export and import |
| [gallery](gallery/README.md) | A public portfolio with a contact form and a JSON feed | Public files under the Content Security Policy, anonymous rate limits, an open GET route, a multi-page Build Artifact, a custom domain, the contract without a credential |
| [habits](habits/README.md) | A personal tracker used from two devices | Required verification, rows bound to the actor, a daily schedule that closes the day, Signals and Usage, a publish from a folder, copies and restore |
| [booking](booking/README.md) | A room board with a public view and staff sign-in | Allowlist by domain, one atomic multi-row write, concurrent double booking, a confirmation email to the booker, a schedule that releases bookings nobody checked in, the contract with a session and with a key |
| [starter](starter/README.md) | The typical app in one: public landing and API, verified open signup, and areas for members, a team and admins | Public reads, key-only and signed routes, guards for roles, the file lifecycle and quota, Signals, a call to the inbox, the contract and conform, an import, the CLI and the editor |

Each app has a `replay.ts` that starts its own Kernel, publishes the app, runs every flow and refusal, runs `konae contract` and `conform`, and removes its data when it exits. Run them all:

```sh
bun run examples/showcase/replay-all.ts
```

Each README ends with a table of what the Kernel refused or could not do for that app, with the exact message. A passing replay with a long table is the expected result, because the apps exist to find those edges. The [probes](../probes/README.md) cover the kinds of app these six do not.
