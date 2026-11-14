# Usage, Signals, schedules and notices

The Kernel counts what an app does and records the Signals its actions declare, refuses a write with a guard, runs scheduled mutations and sends a notice when a row changes.

## Usage and Signals

Each Deployment keeps Usage and Signals in its own `data/usage.sqlite`. Only the owner reads them; the runtime endpoint does not list the tools, so an app cannot read its own Usage. `kernel-rs/kernel/src/usage.rs` implements it, and [ADR-0010](../adr/0010-the-kernel-keeps-usage-and-signals.md) records the decision. The editor's Usage page at `/usage` reads every project the owner holds.

```text
usage_read     { "deploymentId", "from": "2026-11-01", "to": "2026-11-06" }
signals_read   { "deploymentId", "name"?: "order.placed", "cursor"? }
usage_summary  { "deploymentId", "from", "to" }
usage_overview { "from", "to", "cursor"? }
```

All four need `read` and the project grant. Dates are UTC `YYYY-MM-DD`, both included, at most 400 days apart. `usage_read` returns one row per day, kind and name, the totals, and the newest 500 [scheduled runs](#scheduled-work) in the range:

```json
{ "from": "2026-11-01", "to": "2026-11-06",
  "days": [{ "day": "2026-11-06", "kind": "action", "name": "notes.add", "count": 12 }],
  "totals": [{ "kind": "action", "name": "notes.add", "count": 12 }],
  "scheduled": [{ "action": "holds.release", "at": 1762423200000, "outcome": "ok", "changes": 2 }] }
```

| Kind | Name | Counted when |
| --- | --- | --- |
| `session` | | `auth_session` answers. The SDK calls it when an app opens |
| `page_view` | the HTML file served, such as `index.html` | a browser navigation received one of the artifact's HTML pages with 200 |
| `action` | the action | `actions_invoke` runs the action |
| `route` | `POST /hooks/orders` | an HTTP route runs its action, which is not counted again under `action` |
| `upload` | | `files_upload` stores a file |
| `file_read` | | `files_read` or `GET /_konae/files/<id>` returns a file |
| `signup` | | an account becomes active |
| `login` | | a password sign-in or an answered recovery link |
| `refused` | the action | a mutation's [guard](#guards) refused |
| `notify` | `<action>: queued`, or `<action>: <reason>` | a mutation with [`notify`](#notices) changed a row |
| `scheduled` | the action | the timer ran a [scheduled](#scheduled-work) mutation, whether it succeeded or failed |
| `call` | the action | a call action queued a call |

Nothing else the Kernel refuses is counted. The Kernel writes Usage off the request path, and the read tools wait up to five seconds for the queue to drain, so they include every call that finished before them.

`usage_summary` returns the counters, the published actions, daily execution and Signal totals by origin (`runtime`, `route`, `schedule`, `continuation`) and outcome (`ok`, `refused`, `failed`, `unchanged`), the states of retained calls and email, and `coverage`, which says whether queued records finished writing and how far back the store reaches. It also returns `accounts` and `storage`:

- `accounts` counts signed-in app accounts. It returns `active` accounts, app `opens` when `auth_session` answered with the account's session, successful `actions`, active accounts per day, and the 20 most active accounts with their email, days active and last activity. Actions include service calls accepted into the queue, once per call regardless of delivery retries. The store keeps the account's ID only; the email comes from `identity.sqlite` when the summary is read, and is `null` for an account that no longer exists. A visitor without an account is never a row, and a session an owner entered as an account counts nothing.
- `storage` is what the app takes on the installation when the summary is read, whatever the range: the bytes of `database`, `files`, `accounts` (`identity.sqlite`), `usage`, `published` (`dist/`) and `sources` (`app/`), each with its limit when it has one, their `total`, and the number of uploaded files. File bytes are the sizes the file index records, so a store in S3 reports the same figure. `backups` is listed with `exported: false` and left out of the total, because an export does not carry it.

`usage_overview` summarizes up to 50 projects per page with `nextCursor`, keeping each app's account count and storage but not its daily account series or its list of accounts; an unpublished project has `summary: null`. Neither returns Signal properties or request bodies.

A Signal is a named occurrence a Server Action declares. It is recorded when the action succeeds, and for a mutation only when its statement changed a row, with properties from the input:

```json
"newsletter.join": {
  "kind": "signal",
  "access": "anonymous",
  "bindings": [{ "source": "input", "key": "source", "type": "string", "maxLength": 40 }],
  "signal": { "name": "newsletter.joined", "properties": ["source"] }
}
```

The property `actor` records the caller's ID: the app user's, the API key's, or `null` for a visitor or a signed request. An action of `"kind": "signal"` runs no SQL and returns `{ "recorded": true }`. The app records a Signal with `miniapp.actions.call`, through the same access check, input check and rate limit as any action. `signals_read` returns 100 at a time, newest first, as `{ "signals": [{ "name", "at", "properties" }], "nextCursor" }`.

| Refusal | Message |
| --- | --- |
| A Signal on a query | `Action <name> is a query, so it cannot declare a signal. Declare it on a mutation or on a "kind": "signal" action.` |
| A property that is not an input binding's key | `Action <name> names signal property <key>, which is not one of its input bindings.` |
| A property listed twice | `Action <name> names signal property <key> twice.` |
| `actor` listed while an input is named `actor` | `Action <name> names signal property actor, which records the caller's ID, and also has an input named actor. Rename the input.` |
| More than 8 properties | `Action <name> names more than 8 signal properties.` |
| A name that is not dot-separated lowercase words of up to 64 characters | `Action <name> names an invalid signal. Use dot-separated lowercase words, such as "order.placed", up to 64 characters.` |
| `signal` that is not `{ name, properties? }` | `Action <name> has an invalid signal declaration.` |
| A signal action without `signal` | `Action <name> is a signal action, so it must declare a signal.` |
| A signal action with a statement | `Action <name> is a signal action, so it runs no SQL and cannot declare a statement.` |
| A signal action with `http` | `Action <name> is a signal action, so it takes no route. To record a signal from an HTTP route, declare the signal on a mutation with a POST route.` |
| `usage_read` with a date that is not `YYYY-MM-DD` | `from and to must be dates written YYYY-MM-DD.` |
| `usage_read` with `from` after `to` | `from must not be after to.` |
| `usage_read` over more than 400 days | `Usage covers at most 400 days at a time.` |
| `signals_read` with a cursor it did not return | `Invalid cursor. Pass back the nextCursor of an earlier page.` |

The Gate and the Kernel run the cases in `kernel-rs/kernel/src/signal-cases.json`. The store's bounds are on [the limits page](../limits.md#usage-store). An export and a scheduled copy carry it. The editor preview keeps no store.

### Guards

A mutation writes its rule into its `WHERE`. When the rule says no, the statement changes no row, and without a guard the caller gets `{ "changes": 0, "lastInsertId": 0 }`. A guard turns that into a refusal:

```json
"members.promote": {
  "kind": "mutation",
  "access": "authenticated",
  "guard": "Only an organizer can promote a member.",
  "statement": "UPDATE members SET role = 'organizer' WHERE user_id = ?1 AND EXISTS (SELECT 1 FROM members WHERE user_id = ?2 AND role = 'organizer')",
  "bindings": [{ "source": "input", "key": "member", "type": "string", "maxLength": 64 }, { "source": "actor", "field": "id" }]
}
```

When the statement changes no row, `actions_invoke` answers a tool error with the guard's text, so `miniapp.actions.call` rejects with it, and the route answers `403 { "error": "<guard>" }`. Usage counts it under `refused` and no Signal is recorded. The statement ran and changed nothing, so nothing is rolled back. The editor preview refuses the same way, and `conform` probes the guard.

| Declaration | Message |
| --- | --- |
| `guard` on a query | `Action <name> is a query, so it cannot declare a guard. Declare it on a mutation.` |
| `guard` on a signal action | `Action <name> is a signal action, so it cannot declare a guard. Declare it on a mutation.` |
| A guard that is not 1 to 200 characters on one line | `Action <name> has an invalid guard. Use a message of 1 to 200 characters on one line.` |

The cases are in `kernel-rs/kernel/src/guard-cases.json`.

### Scheduled work

A mutation that takes no input may declare when the Kernel's own timer runs it ([ADR-0011](../adr/0011-the-kernel-runs-a-declared-schedule.md)):

```json
"holds.release": {
  "kind": "mutation",
  "access": "authenticated",
  "schedule": { "every": "15m" },
  "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL AND until < ?1",
  "bindings": [{ "source": "kernel", "field": "now" }]
}
```

`"every"` takes whole minutes or hours from `"5m"` to `"24h"`, counted from 1970-01-01 00:00 UTC, so a period that divides the day, such as `"15m"`, runs at :00, :15, :30 and :45. `"daily": "HH:MM"` runs once a day at that UTC time. The Kernel looks once a minute and runs an action at the first look after its period opens. `kernel.now` is the time of the run and an `actor` binding is null. `actions_invoke` and the editor preview refuse a scheduled action with `Action <name> runs only on its schedule.` A scheduled mutation that also declares an `http` route answers there as its callers say.

| Rule | What the Kernel does |
| --- | --- |
| Once a period | The period it last ran for is kept in `usage.sqlite`, which a restart, a copy and an export carry. A Kernel that was down for several periods runs the action once |
| Publishing | The first period an action is seen in counts as done, so publishing never runs it |
| One run at a time | A run takes the Deployment's data lock and the 2-second statement budget. Scheduled actions run in name order |
| Maintenance | An app in maintenance runs nothing, and runs each due action once when it leaves maintenance |
| Failure | A failed run is logged, recorded with `outcome: "failed"` and its error, and tried at the next look |
| Guard and Signal | A run that changes no row is recorded with `changes: 0`, and the guard does not apply. A run that changes a row records the Signal with `actor` null |
| Notice | A run that changes a row sends its `notify`. `to: "owner"` sends to the app's contact email once per run. `to: "actor"` records `no-account` |
| Usage | Each run counts under `scheduled`, and `usage_read` returns the newest 500 runs with `action`, `at`, `outcome`, `changes` and, for a failed run, `error` |

The editor preview runs no schedule; `konae dev` does. `KONAE_SCHEDULE_MIN_SECONDS` runs every schedule every few seconds for a test, and the Kernel refuses it unless `KONAE_BIND` is a loopback address. Nothing is pushed to a page, so a page polls a query with an `after` cursor.

| Declaration | Message |
| --- | --- |
| `schedule` on a query | `Action <name> is a query, so it cannot declare a schedule. Declare it on a mutation.` |
| `schedule` on a signal action | `Action <name> is a signal action, so it cannot declare a schedule. Declare it on a mutation.` |
| `schedule` on a mutation with an input binding | `Action <name> declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values.` |
| A schedule that is not one of `every` and `daily`, such as `"0 8 * * *"` | `Action <name> has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00".` |
| `every` below 5 minutes | `Action <name> runs every 4m, more often than every 5 minutes. Use "5m" or longer.` |
| `every` above 24 hours | `Action <name> runs every 25h, less often than once a day. Use "daily" or a period of 24 hours or less.` |
| `daily` that is not a time from `00:00` to `23:59` | `Action <name> has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59.` |
| `subscribe` on any action | `Action <name> declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead.` |

The cases are in `kernel-rs/kernel/src/schedule-cases.json`.

### Notices

A mutation may send one email when its statement changes a row. `kernel-rs/kernel/src/notify.rs` implements it, and [ADR-0012](../adr/0012-email-leaves-through-one-queue-to-addresses-the-app-has-proved.md) records the decision.

```json
"bookings.create": {
  "kind": "mutation",
  "access": "authenticated",
  "statement": "INSERT INTO bookings (user_id, room, day) VALUES (?1, ?2, ?3)",
  "bindings": [{ "source": "actor", "field": "id" }, { "source": "input", "key": "room", "type": "string", "maxLength": 40 }, { "source": "input", "key": "day", "type": "string", "maxLength": 10 }],
  "notify": { "to": "actor", "subject": "Booked: {input.room} on {input.day}", "body": "Your booking of {input.room} on {input.day} is confirmed." }
}
```

`to` is `owner`, the app's [contact email](./projects-and-releases.md#contact-email), or `actor`, the calling account's email once that account answered a verification, invitation or recovery link. The Kernel never sends to an address from the input. The placeholders are `{input.<key>}` for an input binding, `{actor.id}`, `{kernel.now}` as `2026-11-08 07:00 UTC`, `{changes.count}` and, in the body only, `{changes}`. A value loses its line breaks and keeps 200 characters.

`{changes}` lists the changed rows, one line each, with the columns named in `"changes"`. It lists at most 50 lines, then `- and <n> more`. `"to": { "column": "assignee_id" }` sends one notice to each account whose ID a changed row holds in that column, listing only that account's rows. The Kernel reads the rows back by appending its own `RETURNING`, and the answer stays `{ changes, lastInsertId }`. One run reads at most 1,000 rows and writes to at most 50 accounts.

```json
"tasks.remind": {
  "kind": "mutation", "access": "authenticated", "schedule": { "daily": "07:00" },
  "statement": "UPDATE tasks SET reminded_at = ?1 WHERE due_day = date(?1 / 1000, 'unixepoch') AND reminded_at IS NULL",
  "bindings": [{ "source": "kernel", "field": "now" }],
  "notify": { "to": { "column": "assignee_id" }, "changes": ["title"],
    "subject": "{changes.count} tasks due today", "body": "Due on {kernel.now}:\n{changes}" }
}
```

The body keeps the template's line breaks, the HTML part escapes every value, and the Kernel adds a last line naming the app and why the recipient gets it. A guard's refusal, an `ON CONFLICT DO NOTHING` that stored nothing and a failed statement send nothing, and the mutation's answer never depends on the notice. Usage counts every notice under `notify`, as `bookings.create: queued` or with the reason:

| Reason | What happened |
| --- | --- |
| `no-transport` | The installation has no mail transport. Nothing is stored |
| `owner-has-no-email` | `to` is `owner` and the app has no contact email |
| `no-account` | `to` is `actor` and the caller is a visitor, a signed request, an API key or a timer |
| `unverified-email` | `to` is `actor` and the account has not answered a link, or is not active |
| `not-a-verified-account` | A row's value in the `to` column is not an active account of this Deployment that answered a link |
| `recipient-cap` | The run already wrote to 50 accounts |
| `app-limit`, `creator-limit`, `recipient-limit`, `installation-limit` | A day's budget is used up. Nothing is stored |
| `sender-not-ready` | The sender preflight was not ready. The job is stored as failed and never sent |
| `storage-error` | The queue could not be written. The Kernel logs it |

A queued notice is a job in the account email queue with purpose `notify`, held while its app is in maintenance, and failed as `notice-expired` when it has not left within a day. `notices_read { "deploymentId", "cursor"? }` needs `read` and lists the app's notice jobs newest first, 50 at a time, with the action, the state, attempts, the failure kind and the times; it names no recipient. The editor's What happened panel reads `usage_summary`, `calls_read`, `notices_read` and `signals_read`. Notices count apart from account email, 200 a day per app, 500 per owner and 50 per recipient from one app, and the installation's 1,000 a day counts every email.

| Declaration | Message |
| --- | --- |
| `notify` on a query | `Action <name> is a query, so it cannot declare notify. Declare it on a mutation.` |
| `notify` on a signal action | `Action <name> is a signal action, so it cannot declare notify. Declare it on a mutation.` |
| Not an object of `to`, `subject` and `body`, or another key such as `cc` | `Action <name> has an invalid notify declaration. Give "to", "subject", "body" and, to list changed rows, "changes", and nothing else.` |
| `to` that is not `owner`, `actor` or `{ "column": "<name>" }` | `Action <name> has an invalid notify recipient. Use "owner", "actor" or {"column": "<name>"} for a column of account IDs.` |
| `changes` that is not 1 to 8 different column names | `Action <name> has an invalid notify changes list. Name 1 to 8 columns of the changed table, each once, in letters, digits and underscores.` |
| A subject that is blank, over 120 characters or holds a line break | `Action <name> has an invalid notify subject. Use 1 to 120 characters on one line.` |
| A body that is blank, over 2000 characters or holds a control character other than a line break | `Action <name> has an invalid notify body. Use 1 to 2000 characters, with line breaks and no other control characters.` |
| An unknown placeholder, such as `{actor.email}` | `Action <name> names an unknown placeholder {<placeholder>} in its notify <subject or body>. Use {input.<key>} for one of its input bindings, {actor.id}, {kernel.now}, {changes} or {changes.count}.` |
| `{changes}` in the subject | `Action <name> puts {changes} in its notify subject. Put {changes} in the body, and {changes.count} in the subject.` |
| `{changes}` in the body with no `changes` list | `Action <name> uses {changes} in its notify body without a changes list. Name the columns to show in notify "changes".` |
| A `changes` or `to` column the changed table lacks | `Action <name> names a notify column its statement cannot return. Name columns of the table the statement changes.` |
| A `{` with no `}` after it | `Action <name> has an unclosed placeholder in its notify <subject or body>. Close it with "}".` |

The cases are in `kernel-rs/kernel/src/notify-cases.json` and `kernel-rs/kernel/src/notify-column-cases.json`.

## Read next

- [Refuse a write with guard](../guides/refuse-a-write-with-guard.md)
- [Email when a row changes](../guides/email-when-a-row-changes.md)
- [Run work on a schedule](../guides/run-work-on-a-schedule.md)
- [Limits](../limits.md#usage-store)
