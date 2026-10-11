# The Kernel owns Usage and Signals

Status: accepted, 2026-11-06. Builds on [ADR-0001](0001-separate-runtime-and-admin-data-access.md): the Kernel that authorizes a runtime operation records it.

An owner needs to see whether a Deployment is being used, and a Capsule needs a way to name a business occurrence without turning the browser into an analytics client. The Kernel already authorizes every login, action and upload. Recording those facts belongs with that authority.

## Decision

The Kernel keeps Usage and Signals in a per-Deployment store of its own, next to the app database and the identity file. They do not go in `audit`, and they do not go in the owner's tables. Usage is always on. The Kernel keeps daily totals for authorized Runtime Interface operations. A Signal is recorded only when a Server Action declared it and that action succeeds. Properties come from the action's bindings. The Project Owner reads both through the Admin Interface. A Runtime Actor does not. There is no `miniapp.track`. A Signal action is still `miniapp.actions.call`.

`audit` in `mcp.sqlite` stays the owner-tool log. App users already cannot grow it at will. Hanging Usage there would reopen that hole on a shared file. Writing Signals into `schema.ts` tables would make the host decide who may read a row, and would mix Kernel facts with the Database Model. A public dashboard still uses a Server Action and SQL, as the poll example does.

## Considered options

- **Append to `audit`.** Rejected because runtime traffic would share the owner log and a visitor could fill it.
- **Insert into the app database.** Rejected because the Database Model is the owner's, and who may read a row is the app's decision.
- **A browser `track` call with a free payload.** Rejected because the bundle is inspectable and replayable, the same reason ADR-0001 keeps SQL off the client.
- **Let Server Actions query the Kernel store.** v1 leaves that out, so in-app funnels stay in SQL.

## Consequences

Export with live state takes the Usage store along with accounts, the database and files. The Validation Gate and the Kernel both parse `signal` on an action and `kind: "signal"`. A Signal counts toward the action cap. The store has its own size bound and drops the oldest rows. The Development Kernel records the same way so an unpublished app still shows Usage.

## Note, 2026-11-06: implemented

`kernel-rs/src/usage.rs` holds the store, the declaration parser and the owner tools. Each Deployment's store is `deployments/<id>/usage.sqlite`, next to `database.sqlite` and `identity.sqlite`. It has two tables. `usage_daily` keeps one count per UTC day, kind and name. `signals` keeps one row per Signal with its name, time and properties.

Usage has seven kinds, each counted only after the Kernel allowed the operation and it succeeded:

| Kind | Name | Counted when |
| --- | --- | --- |
| `session` | | `auth_session` answers, which the SDK calls when an app opens |
| `action` | the action | `actions_invoke` runs a Server Action |
| `route` | `POST /hooks/orders` | an HTTP route runs its action. It is not counted again under `action` |
| `upload` | | `files_upload` stores a file |
| `file_read` | | `files_read` or `GET /_littlebox/files/<id>` returns a file |
| `signup` | | an account becomes active: a signup in local mode, an answered verification link or an accepted invitation |
| `login` | | a password sign-in or an answered recovery link opens a session |

A refused call, an input the bindings reject, a statement SQLite refuses and a wrong password count nothing and record no Signal.

A handler puts a message on a queue of 16,384 entries and returns. One writer thread drains the queue and writes each Deployment's file in one transaction per batch. When the queue is full the entry is dropped and the Kernel logs it, so a burst of traffic never waits on this store. `usage_read`, `signals_read` and `package_export` wait for the queue to drain first, for at most five seconds, so they include everything recorded before the call.

The store keeps 400 days and at most 100,000 rows of totals, dropping the oldest days first, and the newest 50,000 Signals. A Signal has at most 8 properties and a string property keeps its first 128 bytes, so a full store stays well under the 128 MiB a package accepts for it.

An action declares `"signal": { "name": "order.placed", "properties": ["id"] }`. The name follows the action name rule: dot-separated lowercase words, at least two, up to 64 characters. A property is the key of one of the action's input bindings, and the value recorded is the input the action ran with. Actor, kernel and literal bindings have no key, so a Signal cannot carry the account that caused it. A query may not declare a Signal. An action of `"kind": "signal"` has access, bindings and a Signal and no statement, runs no SQL and returns `{ "recorded": true }`. It may not declare `http`. The Gate refuses the same declarations as the Kernel, with the same messages, from `kernel-rs/src/signal-cases.json`, which both test suites run. A signal action counts toward the action cap. That cap was already 50 in the Gate and 100 in the Kernel before this change; this note does not change either number.

The owner reads with `usage_read { deploymentId, from, to }`, at most 400 days, and `signals_read { deploymentId, name?, cursor? }`, newest first, 100 a page. Both need `read` and the project grant. The runtime endpoint does not list them.

The scheduled copy already took `usage.sqlite` with `VACUUM INTO`, and restore checks its integrity. `package_export` carries it as an optional fourth component, `usage`, when the Deployment has one; an import installs it. The small `project_export` format does not carry it. The Development Kernel that `bun run littlebox -- dev` starts is the same binary, so it records into `.littlebox/dev-data/deployments/<id>/usage.sqlite`. The editor preview records nothing; a signal action there returns `{ "recorded": true }` without a store.

## Update, 2026-11-06: a refused mutation is visible, and a Signal can name the actor

The showcase apps counted 12 Signals for 8 check-ins, 61 `booking.created` for 24 bookings, and an `event.received` for a webhook retry that stored nothing, because a Signal on a mutation was recorded whenever the statement ran without error. A role check written into a `WHERE` answered `{ changes: 0 }` like a missing row, Usage counted it as a success and the Action Contract had no refusal for it, so `conform` would pass a backend that dropped the check.

A Signal declared on a mutation is now recorded only when the statement changed at least one row. A signal action is unchanged: it records whenever it is allowed to run.

A mutation may declare `"guard": "<message>"`, 1 to 200 characters on one line. When its statement changes no row the Kernel answers that message as a refusal, a tool error on the runtime endpoint and 403 on the route, records no Signal, and counts the call in Usage under a new kind, `refused`, named by the action. This is the one refusal Usage counts, because it is the app's rule rather than the platform's, and the owner wants to see it next to the calls that ran. Without a guard a mutation answers `{ changes: 0 }` and counts as before. We considered a declared `refuse` clause with its own SQL, and kept the rule in the one statement: a second statement would run outside the first's view of the rows and could disagree with it. The Gate and the Kernel parse guards from `kernel-rs/src/guard-cases.json`; the editor preview refuses the same way; the Action Contract lists the guard among the action's errors, and `conform` probes it by sending the valid input twice.

A Signal may list `actor` among its properties. It records the caller's ID: the app user's, the API key's, or null for a visitor or a signed request. The note above said a Signal cannot carry the account that caused it; that is no longer true. An action that lists `actor` may not also have an input named `actor`. The cases are in `kernel-rs/src/signal-cases.json`.

## Note, 2026-11-07: scheduled runs

[ADR-0017](0017-the-kernel-runs-a-declared-schedule.md) adds an eighth kind, `scheduled`, counted by action name for each run of a declared schedule, and two tables in `usage.sqlite`: `scheduled_runs`, the newest 10,000 runs with their outcome and changes, and `schedule_state`, the slot each scheduled action last ran for. The schedule's timer writes them directly rather than through the queue, because a lost write would let a run repeat after a restart. `usage_read` returns the newest 500 runs of its range under `scheduled`.
