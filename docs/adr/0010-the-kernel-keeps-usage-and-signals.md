# The Kernel keeps Usage and Signals

Status: accepted.

An owner needs to see whether an app is used, and an app needs a way to name a business occurrence, such as an order placed, without turning the browser into an analytics client. The Kernel already authorizes every sign-in, action and upload, so recording those facts belongs with it. The app's own rows stay the app's.

## Decision

### Usage and Signals live in their own store

Each Deployment keeps `usage.sqlite` in its data, next to the database and the identity file. They do not go in an audit log or in the app's tables. The owner reads them with `usage_read`, `usage_summary`, `usage_overview` and `signals_read`, which need `read`. A Runtime Actor never reads them. There is no `miniapp.track`.

### Usage counts what the Kernel allowed and what succeeded

Usage is always on and keeps daily totals by kind and name: `session`, `action`, `route`, `upload`, `file_read`, `signup`, `login`, `refused`, `notify`, `call`, `scheduled` and `page_view`. A refused call, an input the bindings reject, a statement SQLite refuses and a wrong password count nothing.

- A `page_view` is a browser navigation (`Sec-Fetch-Dest: document`) that the Kernel answers with one of the artifact's `.html` files and 200. Prefetches, assets, HEAD requests, requests without the header and 404s count nothing. The name is the file served, never the address asked for, so a visitor cannot grow the store by inventing paths.
- A session that an owner entered as one of the app's users (ADR-0001) is not counted as that user.
- A handler puts each count on a queue of 16,384 entries and returns. One writer thread drains it in one transaction per batch. When the queue is full the entry is dropped and counted, so traffic never waits on this store. The reads wait up to five seconds for the queue to drain first.
- `usage_summary` adds daily outcomes per action and feature, by origin (runtime, HTTP route, schedule, call), and Signal totals per day. A page view is a load, not a person. The Kernel keeps no per-visitor key and counts no unique visitors.
- The store keeps 400 days and bounded row counts, dropping the oldest first.

### A Signal is declared on a Server Action

- An action declares `"signal": { "name": "order.placed", "properties": ["id"] }`. A property is the key of one of the action's input bindings, or `actor`, which records the caller's ID: the app user's, the API key's, or null. A Signal has at most 8 properties.
- An action of `"kind": "signal"` records a Signal and runs no SQL. It answers `{ "recorded": true }`.
- A Signal declared on a mutation is recorded only when the statement changed at least one row. A query may not declare one.

### A mutation may declare a guard

A mutation may declare `"guard": "<message>"`. When its statement changes no row, the Kernel answers that message as a refusal (a tool error on the runtime endpoint, 403 on a route), records no Signal, and counts the call under `refused`. Without a guard a mutation that changes nothing answers `{ changes: 0 }`. The Action Contract lists the guard among the action's errors, and `konae conform` probes it.

## Rejected options

- **Append to an audit log.** Runtime traffic would share the owner's log, and a visitor could fill it.
- **Insert into the app's database.** The database is the app's, and who may read a row is the app's decision.
- **A browser `track` call with a free payload.** The bundle is inspectable and replayable, the same reason SQL stays off the client (ADR-0002).
- **A second statement that decides a refusal.** It would run outside the first statement's view of the rows and could disagree with it. The guard keeps the rule in one statement.
- **Count unique visitors with a cookie or a hashed address.** The Kernel would keep a per-visitor key it has no other use for.

## Consequences

- An export carries `usage.sqlite`, so Usage, Signals and the schedule's slots (ADR-0011) move with the app.
- Counts are approximate under pressure. The summary reports dropped entries and write errors since restart for the whole installation.
- A queued call is not a delivered call, and an accepted email is not a delivered one. The summary does not add actions, Signals and notices into one total.
- The editor preview records nothing. A signal action there answers `{ "recorded": true }` without a store.
- Server Actions cannot query the store, so in-app funnels stay in SQL.
