# Probes

Small Capsules that decide the cells of the [capability matrix](../../docs/research/capability-matrix.md) no [showcase](../showcase/README.md) replay proves. Each folder holds a Capsule (`index.html`, `app.js`, `schema.sql`, `actions.json`) and a `replay.ts` that publishes it on its own Kernel, asserts every answer the matrix quotes, and removes the Kernel and its data when it exits. `_kit.ts` is the harness they share, and `_dns.ts` is a fake DNS resolver on a local UDP port that the custom domain replays hand to the Kernel as `LITTLEBOX_DNS_RESOLVER`.

| Probe | What it is | Cells |
| --- | --- | --- |
| [dataset](dataset/replay.ts) | Public river levels with search, paging, sorting, totals and a keyed write route | Public read API, private write API, search, pagination and sorting, aggregates |
| [hub](hub/replay.ts) | A private hub fed by outside services' webhooks | Webhooks in, a payment provider's signature, the program API |
| [shop](shop/replay.ts) | Prints with limited stock and holds that lapse | Inventory under concurrency, delayed work, payments, what the page may load |
| [community](community/replay.ts) | Publishers and readers on one public app | Roles inside the app, public and private rows, realtime by polling, audit, files by role |
| [survey](survey/replay.ts) | An anonymous survey and a signed-in vote | Multi-answer form in one write, tallies, one vote per person |
| [platform](platform/replay.ts) | Notes with reminders | Email on an event, scheduled work, realtime push, third-party login, an unclaimed hostname |
| [domain](domain/replay.ts) | A public guestbook on guestbook.example.org | Custom domain: the TXT proof through a fake resolver, the alias as the app's origin, one holder per name, the package and the maintenance page |

Run them all, each on its own port from 23000:

```sh
bun run examples/probes/replay-all.ts
LITTLEBOX_PROBE_BASE_PORT=23100 bun run examples/probes/replay-all.ts
```

Run one with `LITTLEBOX_PROBE_PORT=23000 bun run examples/probes/dataset/replay.ts`. Each replay compiles the Kernel with `cargo build --offline` unless `LITTLEBOX_KERNEL_BIN` names a Kernel binary already built from the same sources. The probes do not open Chrome.

A probe asserts what the Kernel answers today, refusals included. Where a probe pins an answer the matrix calls a gap, such as a `"schedule"` key that both doors accept and ignore, fixing the gap fails the probe on purpose; update the probe and the matrix cell together.
