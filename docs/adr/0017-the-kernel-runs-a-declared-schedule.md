# The Kernel runs a declared schedule

Status: accepted, 2026-11-07. Builds on [ADR-0010](0010-kernel-owns-usage-and-signals.md): a run is recorded in the Usage store, and the store carries the state that keeps a run from repeating.

The capability matrix found that nothing in Littlebox runs without a caller (feature F18 in `docs/research/capability-matrix.md`). Work that only has to look done when someone asks can compare a stored time with `kernel.now`, but a hold that lapsed still blocks the hour in every count that does not repeat the comparison, and a personal tracker cannot close a day nobody opened. A `"schedule"` key passed both doors and ran nothing, so an agent could write one and see it accepted.

## Decision

A mutation may declare `"schedule": { "every": "<n>m" | "<n>h" }`, from 5 minutes to 24 hours, or `"schedule": { "daily": "HH:MM" }` in UTC. It may not have input bindings, because a run has no caller to supply them; actor, kernel and literal bindings stay. A query and a signal action may not declare one. Both doors parse it from `kernel-rs/src/schedule-cases.json` with the same messages, after the guard. A `subscribe` key on any action is refused with a message that points at polling, so the other half of the old false acceptance is closed too.

The Kernel runs every schedule from one timer task in its one process, as `copies.rs` runs copies. It looks once a minute. Time is cut into slots: an `every` period counted from 00:00 UTC, or a day that starts at the `daily` time. An action runs once in a slot, at the first look after the slot opens. For each Deployment the timer runs the due actions one after another, in name order, and skips the Deployment while its previous look is still running, so runs never overlap or queue. A run:

- is skipped while the Deployment is in maintenance, and happens once when it leaves maintenance;
- takes the Deployment's data lock and runs the statement through the same runner as an action call, with the 2-second statement budget;
- binds `kernel.now` as the time of the run, literals as written and NULL for `actor`;
- ignores the guard, because a run that changes no row had nothing to do, and records the action's Signal with `actor` null when it changed a row;
- is recorded in the Deployment's `usage.sqlite`: a row in `scheduled_runs` with its time, outcome, changes and, for a failure, SQLite's error; a count under the Usage kind `scheduled` by action name; and, when it succeeded, the slot in `schedule_state`. The timer writes these itself, in one transaction, instead of through the Usage queue, because a lost write would let the action run again after a restart.

A run that fails leaves its slot open, so the next look tries again. The first slot an action is seen in counts as done, so publishing never runs it, and a republish that changes the declaration starts again from the next slot. Because the slot travels in `usage.sqlite`, a restart, a scheduled copy, a package and a migration resume where the app was: a daily job that ran today does not run again today on the new machine. The owner reads runs with `usage_read`, which now returns the newest 500 runs in its range next to the totals. The store keeps the newest 10,000 runs.

The editor preview runs no schedule. The Development Kernel is the same binary and runs them. `LITTLEBOX_SCHEDULE_MIN_SECONDS`, from 1 to 300, makes every schedule run once every that many seconds so a replay can watch a run; the Kernel refuses to start with it unless `LITTLEBOX_BIND` is a loopback address, and it keys the store's slots apart from the real clock's, so a Kernel started later without it starts each schedule afresh.

## Considered options

- **Cron expressions.** Rejected because a five-field expression is easy to get wrong and hard to read for a person who does not write code, and two forms cover the work the matrix lists: a period and a time of day.
- **A period shorter than 5 minutes.** Rejected because every Deployment's runs share one timer and one process, and work that must look current within minutes can still compare `kernel.now` in the query that reads it.
- **Run each missed slot after downtime.** Rejected because a release or a close that runs ten times in a row does the same work ten times. One run catches up.
- **Run at publish.** Rejected because a daily job declared for 02:00 would run at whatever hour the owner published.
- **Pass input from the declaration.** Rejected for now. Literal bindings already carry fixed values, and an input with no caller would need a second place to write values the action's bindings already hold.
- **Keep the slot in the Deployment record or in memory.** Rejected because the record does not travel in a package and memory does not survive a restart. `usage.sqlite` travels in both and in copies.
- **A process or a queue per app.** Rejected by the product rule in `docs/architecture.md`: a new app creates no process.

## Consequences

An app can release, close, expire and tidy without a caller. Outbound calls retry through their own worker under ADR-0019. A changed scheduled mutation processes its declared notice through ADR-0016's queue and budgets. It sends one notice per changed run, rather than per changed row. A notice to the owner works without a caller; a notice to actor records no-account. The templates have no row values, so a reminder can tell the owner that work is due and link to the app. A busy Kernel runs a schedule up to a minute late, and a Deployment with many slow scheduled actions delays only its own next look. `project_export`, the small portable format, does not carry the Usage store, so an app imported from it starts its schedules at the next slot: it may skip one run, and never repeats one.


## Integration check, 2026-11-07

The schedule runner accepted `notify` but did not process it. It now calls the same notice recorder as a caller-driven mutation after a successful run changes rows. The schedule probe checks two changed runs, two owner notices and the Usage counts, and verifies that an unchanged run sends nothing. The slot is recorded before the notice, so a restart after recording the slot but before queueing its notice can lose that notice; the SQL write, Usage store and mail store are separate transactions.
