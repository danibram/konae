# The Kernel runs a declared schedule

Status: accepted.

Some work must happen when nobody is looking: a hold that lapses, a day that closes, a digest that goes out at seven. A comparison with `kernel.now` in each query can make lapsed work look done, but a lapsed hold still blocks every count that does not repeat the comparison. An app may run no process and no code of its own on the server, so the answer is a declaration the Kernel runs.

## Decision

- A mutation may declare `"schedule": { "every": "<n>m" | "<n>h" }`, from 5 minutes to 24 hours, or `"schedule": { "daily": "HH:MM" }` in UTC. It may not have input bindings, because a run has no caller to supply them. A query and a signal action may not declare one. A `subscribe` key on any action is refused with a sentence that points at polling.
- One timer task in the Kernel runs every schedule and looks once a minute. An `every` schedule cuts time into slots counted from 1970-01-01 00:00 UTC, and a `daily` one into days that start at its time. An action runs once in a slot, at the first look after the slot opens.
- For each Deployment the timer runs the due actions one after another, in name order, and skips the Deployment while its previous look is still running, so runs never pile up.
- A run takes the data lock and runs the statement as an action call does, with the 2-second budget. It binds `kernel.now` as the time of the run, literals as written and NULL for the actor. A guard does not apply, because a run that changes no row had nothing to do. The action's Signal is recorded when the run changed a row, and its notice is sent then too (ADR-0012).
- A Deployment in maintenance is skipped and runs once when it leaves it.
- Each run is recorded in `usage.sqlite`: a row with its time, outcome, changes and any error, a count under `scheduled`, and, when it succeeded, the slot it ran for. The timer writes these itself, in one transaction, because a lost write would let the action run again after a restart. `usage_read` returns the newest runs of its range.
- A run that fails leaves its slot open, and the next look tries again. The first slot an action is seen in counts as done, so publishing never runs it.
- Only the timer runs a scheduled mutation. The runtime endpoint, the page bridge and the editor preview refuse every caller with `Action <name> runs only on its schedule.` A mutation that must also run on demand declares an HTTP route, whose callers decide who may call it, or is written as a second action over the same statement.
- `KONAE_SCHEDULE_MIN_SECONDS` makes every schedule run once every that many seconds, so a local run can watch it. The Kernel refuses it unless `KONAE_BIND` is a loopback address, and keys its slots apart from the real clock's.

## Rejected options

- **Cron expressions.** A five-field expression is easy to get wrong and hard to read for a person who does not write code. A period and a time of day cover the work apps ask for.
- **A period under 5 minutes.** Every Deployment's runs share one timer and one process, and work that must look current within minutes can compare `kernel.now` where it reads.
- **Run each missed slot after downtime.** A release or a close that runs ten times in a row does the same work ten times. One run catches up.
- **Run at publish.** A daily job set for 02:00 would run at whatever hour the owner published.
- **Let a scheduled mutation answer callers as its `access` says.** Every app with a schedule wanted the opposite and wrote a clause that only the timer satisfied. An author who forgot the clause let any member run the close on demand.
- **Keep the slot in the Deployment record or in memory.** The record does not travel in an export, and memory does not survive a restart. `usage.sqlite` travels and survives.
- **A process or a queue per app.** A new app creates no process.

## Consequences

- An app can release, close, expire and tidy without a caller. Because the slot travels in `usage.sqlite`, a restart, a copy and an export resume where the app was, and a daily job that ran today does not run again today on the new machine.
- A busy Kernel runs a schedule up to a minute late. A Deployment with many slow scheduled actions delays only its own next look.
- The slot is recorded before the notice is queued, so a restart between the two can lose that notice.
- The editor preview runs no schedule. `konae dev` and a published Deployment do.
