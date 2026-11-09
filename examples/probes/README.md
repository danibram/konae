# Probes

Each probe is a small Capsule with a replay. The replay proves that one kind of app works on the Kernel where no [showcase](../showcase/README.md) replay does, or pins the refusal both doors give when it does not. Each folder holds a Capsule (`index.html`, `app.js`, `schema.sql`, `actions.json`), or one per app in its own subfolder, and a `replay.ts`. The replay publishes the Capsule on its own Kernel, asserts every answer, and removes the Kernel and its data when it exits. `_kit.ts` is the harness they share. `_dns.ts` is a fake DNS resolver on a local UDP port, which the domain replay hands to the Kernel as `KONAE_DNS_RESOLVER`.

| Probe | What it is | What the replay proves |
| --- | --- | --- |
| [dataset](dataset/replay.ts) | Public river levels with search, paging, sorting, totals and a keyed write route | A public JSON read API another origin can call, a write route only an API key reaches, search, paging, sorting and aggregates |
| [hub](hub/replay.ts) | A private hub fed by outside services' webhooks, each signed in its provider's own scheme | Webhooks signed the way each provider documents, a payment confirmed by its provider's webhook, and an API for a program |
| [shop](shop/replay.ts) | Prints with limited stock and holds that lapse, with a Checkout Session through a call to `pay/` and a Stripe webhook on `store/` | Stock that holds under concurrent orders, a hold that lapses, a payment through a call and a webhook, and what the page may load from a payment provider |
| [community](community/replay.ts) | Publishers and readers on one public app | Roles kept in the app's own table, public and private rows in one table, updates by polling, a record of who did what, and files shared with a role |
| [survey](survey/replay.ts) | An anonymous survey and a signed-in vote | A multi-answer form in one write, tallies, one vote per person, and the answer to a duplicate or an invalid answer |
| [notify](notify/replay.ts) | A print drop with a waitlist and orders | A notice to the contact email, a confirmation to the person who ordered, what is not sent, and what both doors refuse |
| [notify-rows](notify-rows/replay.ts) | Chores assigned to app users, on `konae dev` | A scheduled notice reaches each app user its rows name, with only their own rows, and nobody else |
| [platform](platform/replay.ts) | Notes with reminders | The runtime endpoint offers no mail, schedule, subscription or sign-in-elsewhere tool. Both doors refuse an action of kind email, a schedule on a mutation that takes input and a subscription, and a hostname no `domain_set` claimed does not serve the app |
| [domain](domain/replay.ts) | A public guestbook on guestbook.example.org | A custom domain proved by its TXT record through a fake resolver, the alias as the app's own origin, one holder per name, and the domain in an export |
| [schedule](schedule/replay.ts) | Reminders that a schedule surfaces when they fall due | A run with nobody asking, its Usage, a failed run retried, and notices from a run |
| [calls](calls/replay.ts) | A tasks app that escalates to a desk app, two Capsules in `tasks/` and `desk/` | Calls between apps, retries and the outbox, notices from a `then` mutation, and a page reading another app's public route |

Run them all, each on its own port from 23000, or from another base:

```sh
bun run examples/probes/replay-all.ts
KONAE_PROBE_BASE_PORT=23100 bun run examples/probes/replay-all.ts
```

Run one with `KONAE_PROBE_PORT=23000 bun run examples/probes/dataset/replay.ts`. Each replay but notify-rows compiles the `konae` binary with `cargo build --offline`, unless `KONAE_BIN` names one already built from the same sources. notify-rows runs `bun run konae -- dev`, which compiles the `kernel` binary unless `KONAE_KERNEL_BIN` names one, and it also needs the next port free for the events page. The probes do not open Chrome. The schedule and notify-rows probes start their Kernel with `KONAE_SCHEDULE_MIN_SECONDS=2`, so a schedule declared every 5 minutes runs every 2 seconds.

A probe asserts what the Kernel answers today, refusals included. When the Kernel learns to do something a probe pins as a refusal, the probe fails on purpose. Update the probe and the README row together.
