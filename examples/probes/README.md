# Probes

Small Capsules that decide the cells of the [capability matrix](../../docs/research/capability-matrix.md) no [showcase](../showcase/README.md) replay proves. Each folder holds a Capsule (`index.html`, `app.js`, `schema.sql`, `actions.json`), or one per app in its own subfolder, and a `replay.ts` that publishes it on its own Kernel, asserts every answer the matrix quotes, and removes the Kernel and its data when it exits. `_kit.ts` is the harness they share, and `_dns.ts` is a fake DNS resolver on a local UDP port that the custom domain replays hand to the Kernel as `KONAE_DNS_RESOLVER`.

| Probe | What it is | Cells |
| --- | --- | --- |
| [dataset](dataset/replay.ts) | Public river levels with search, paging, sorting, totals and a keyed write route | Public read API, private write API, search, pagination and sorting, aggregates |
| [hub](hub/replay.ts) | A private hub fed by outside services' webhooks, each signed in its provider's own scheme | Webhooks in, payments confirmed by a provider's webhook, the program API |
| [shop](shop/replay.ts) | Prints with limited stock and holds that lapse; Checkout Session via a call to `pay/`, Stripe webhook on `store/` | Inventory under concurrency, delayed work, payments, what the page may load |
| [community](community/replay.ts) | Publishers and readers on one public app | Roles inside the app, public and private rows, realtime by polling, audit, files by role |
| [survey](survey/replay.ts) | An anonymous survey and a signed-in vote | Multi-answer form in one write, tallies, one vote per person |
| [notify](notify/replay.ts) | A print drop with a waitlist and orders | Email on an event: a notice to the owner, a confirmation to the person who ordered, what is not sent, what both doors refuse |
| [platform](platform/replay.ts) | Notes with reminders | No mail tool or kind on the runtime door, realtime push, third-party login, an unclaimed hostname, and the schedule and subscription forms both doors refuse |
| [domain](domain/replay.ts) | A public guestbook on guestbook.example.org | Custom domain: the TXT proof through a fake resolver, the alias as the app's origin, one holder per name, the package and the maintenance page |
| [schedule](schedule/replay.ts) | Reminders that a schedule surfaces when they fall due | Scheduled work: a run with nobody asking, its Usage, a retried failure, maintenance and owner notices |
| [calls](calls/replay.ts) | A tasks app that escalates to a desk app, two Capsules in `tasks/` and `desk/` | Calls between apps, retries and the outbox, notices from a then mutation, a page reading another app's public route |

Run them all, each on its own port from 23000:

```sh
bun run examples/probes/replay-all.ts
KONAE_PROBE_BASE_PORT=23100 bun run examples/probes/replay-all.ts
```

Run one with `KONAE_PROBE_PORT=23000 bun run examples/probes/dataset/replay.ts`. Each replay compiles the Kernel with `cargo build --offline` unless `KONAE_KERNEL_BIN` names a Kernel binary already built from the same sources. The probes do not open Chrome. The schedule probe starts its Kernel with `KONAE_SCHEDULE_MIN_SECONDS=2`, which `_kit.ts` passes through, so a schedule declared every 5 minutes runs every 2 seconds.

A probe asserts what the Kernel answers today, refusals included. Where a probe pins an answer the matrix calls a gap, fixing the gap fails the probe on purpose; update the probe and the matrix cell together. The platform probe asserted until 2026-11-07 that both doors accepted and ignored a `"schedule"` and a `"subscribe"` key; it now asserts the refusals both doors give.
