# Room board

A room booking board for a small office. Anyone with the link sees which rooms are free each hour from 08:00 to 18:00. Staff sign in with an address at the office's domain, book one to four hours, cancel their own bookings and see who holds a room. A booker whose account has proved its address gets a confirmation by email. A door panel checks a booking in over a signed HTTP route.

`replay.ts` publishes it the way an owner would and records what the Kernel allowed and what it refused.

## Platform parts it uses

| Part | Where |
| --- | --- |
| Public Deployment with anonymous queries | `rooms.list` and `availability.day` answer a signed-out visitor with `{ room, hour }` only, no titles and no names |
| Authenticated mutations | `bookings.create`, `bookings.cancel` and `staff.name` bind the caller with `{ "source": "actor", "field": "id" }` |
| Allowlist signup by `@domain` | `konae.json` names `signupPolicy: { mode: "allowlist", allowlist: ["@office.example"] }` and `konae publish` installs it on the first release |
| One SQL statement per action | `bookings.create` is one `INSERT ... WITH RECURSIVE ... SELECT` that writes every hour of the span or none |
| CHECK constraints and a partial unique index | `schema.sql` refuses a fractional hour, an hour outside 08 to 17, an invalid date and a second holder of an hour |
| HTTP route, unsigned | `GET /api/availability?day=YYYY-MM-DD` for a lobby screen |
| HTTP route, signed, with an API key | `POST /api/checkin`, signed with the `panel_hook` secret, or called with a key that names `panel.checkin` |
| Guards | `bookings.create`, `bookings.cancel` and `panel.checkin` declare the sentence a refused call answers with |
| Signals | `booking.created` and `booking.cancelled`, each naming the caller as `actor`, read by the owner with `signals_read` |
| Notices | `bookings.create` declares `"notify": { "to": "actor", ... }`, so a booking that inserted rows emails its booker, when the booker's account has proved its address |
| Schedule | `bookings.release` declares `"schedule": { "every": "15m" }`. The Kernel's timer releases a booking nobody checked in within 15 minutes of its first hour, with nobody calling, and records `booking.released` when it releases one |
| Action Contract and `conform` | Generated from `actions.json` and `schema.sql`, run with a session token and with an API key |

The owner adds rooms with `database_execute`, because `schema.sql` may only create tables and indexes.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
KONAE_SHOWCASE_PORT=19473 bun run examples/showcase/booking/replay.ts
```

The replay builds the Kernel offline, starts it on `KONAE_SHOWCASE_PORT` (default 19470) with a random setup key and a data directory under the system temp folder, and deletes both on exit. It refuses to start when something already answers on the port. `KONAE_SHOWCASE_BROWSER=0` skips Chrome. `KONAE_SHOWCASE_SCREENSHOTS=<dir>` writes `booking-visitor-light.png`, `booking-staff-light.png` and `booking-staff-dark.png` there.

It prints one `PASS` line per group and a JSON report, and exits 0 when every group holds. The replay's Kernel uses the development mail transport, so email lands in `mailbox.jsonl` in its data directory.

To publish it yourself on an installation where you have an owner account, such as the one `bun run dev:local` starts:

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/booking --url http://localhost:4173
bun run konae -- init --dir /tmp/booking --name "Room board"
rm -rf /tmp/booking/app && mkdir /tmp/booking/app
cp examples/showcase/booking/{index.html,styles.css,app.js,schema.sql,actions.json} /tmp/booking/app/
```

1. In `/tmp/booking/konae.json` set `"visibility": "public"` and `"signupPolicy": { "mode": "allowlist", "allowlist": ["@office.example"] }`, with your office's domain.
2. Run `bun run konae -- publish --dir /tmp/booking`. The first run registers the project and is refused, naming the unset signing secret `panel_hook`.
3. Over MCP, set it with `secret_set`, with the `id` from `konae.json` as `projectId`, and publish again.
4. Add the rooms:

    ```sh
    bun run konae -- sql --dir /tmp/booking --prod "INSERT INTO rooms (id, name, seats) VALUES ('oak', 'Oak', 6), ('birch', 'Birch', 4)"
    ```

5. For a door panel that calls the route with a key, issue one over MCP with `api_keys_create` and `actions: ["panel.checkin"]`.

## What worked

- **Allowlist by domain.** `ana@office.example` signs up. `pat@elsewhere.example` and `sam@sub.office.example` get exactly the answer a listed address gets under Closed: status 403 and `{"error":"This app is not accepting new accounts for this email address."}`. The replay compares the two bodies with `deepEqual`, so the refusal does not tell a stranger whether the domain or the policy stopped them. A domain entry does not match its subdomains. Closing signup does not sign out existing staff.
- **The policy is in place before the first request.** `konae publish` sends konae.json's `signupPolicy` as `initialSignupPolicy`, so the public app never runs Open.
- **Public reads stay narrow.** The anonymous query projects `room` and `hour` and nothing else; titles and names come only from `bookings.day`, which needs a session. A visitor calling `bookings.day`, `bookings.create` or `bookings.recent` gets `authentication-required`.
- **A span of hours in one statement.** `bookings.create` generates the hours with a recursive CTE inside the `INSERT`'s `SELECT`, guards them with `WHERE`, and writes every hour or none. Cancelling is one `UPDATE ... WHERE booking_ref = ? AND user_id = ?` over all the rows of a booking. Another member's cancel changes 0 rows and is refused with the cancel guard.
- **No double booking.** Twenty rounds of two overlapping requests sent at once, and one round of ten requests for the same hour, each end with exactly one holder. The loser always gets the create guard, never SQLite's text.
- **Backstops in the schema.** Owner SQL that skips the action's `WHERE` still meets `UNIQUE constraint failed: bookings.room_id, bookings.day, bookings.slot` and `CHECK constraint failed: typeof(slot) = 'integer' AND slot BETWEEN 8 AND 17`.
- **Routes.** `GET /api/availability` returns the visitor's rows as JSON. `POST /api/checkin` refuses an unsigned body with 401 `invalid-signature`, a stale `t` with 401 `stale-timestamp`, and accepts a signature or the panel's API key. A second check-in for the same hour, or one for an hour nobody booked, gets 403 with the guard `No booking holds this room at this hour, or it is already checked in.` instead of `{ changes: 0 }`. A staff session cookie sent without a signature gets 401 too, and `panel.checkin` answers only on its route.
- **A confirmation by email.** `bookings.create` declares `"notify": { "to": "actor", "subject": "Booked: {input.room} on {input.day} at {input.start}:00", ... }`. Cara, who joined by invitation, books two hours and gets one email with that subject; her overlapping attempt is refused with the guard and sends nothing. Ana and Ben signed up under the Allowlist in local mode, where nothing proves an address, so their 25 bookings succeeded and sent nothing; `usage_read` lists `bookings.create: unverified-email` 25 times beside `bookings.create: queued` once. On an installation with `KONAE_ACCOUNT_VERIFICATION=required` every booker has answered a link and gets the email.
- **Releasing a booking nobody claimed.** A booking that nobody checked in within 15 minutes of its first hour holds the room for no one. `bookings.release` is one `UPDATE` over every hour of such a booking, and its `"schedule": { "every": "15m" }` has the Kernel run it with nobody calling. The replay's Kernel sets `KONAE_SCHEDULE_MIN_SECONDS=2`, so it runs every 2 seconds: Ana's booking for 09:00 the day before is released, the one the panel checked in at 10:00 stays, and no booking from today on is touched. A scheduled mutation with no route answers only to the timer, so Ben calling the action himself is refused with `Action bookings.release runs only on its schedule.` `usage_read` lists each run with its outcome and changes, and one `booking.released` Signal records the run that released a row.
- **Content Security Policy.** The visitor page and the staff page load in Chrome under the Kernel's policy with no violation and no page error, and a staff member books two hours by clicking the grid.
- **Errors on the page.** Every status line reads a failure through one `reason(error)` helper, the same in every showcase app but starter. A refusal shows its own sentence, `rate-limited` becomes "Too many requests just now. Wait a few seconds and try again.", and a Kernel the browser cannot reach becomes "The board could not reach its server. Check the connection and try again." The replay takes the page offline and moves to the next day to prove the last one.

## Atomic booking and the one-statement rule

The obvious model has two tables: a `slots` table that holds one row per room and hour, and a `bookings` table that records who booked what, plus a log of changes. A booking writes one `bookings` row and N `slots` rows, and a cancel deletes the slots and writes a log row. Konae runs exactly one SQL statement per action, so none of those pairs can be one action:

- A second statement in the same action is refused: `Action bookings.cancel must contain one statement.`
- A trigger that writes the second table is refused at both doors with `schema.sql may only create tables and indexes, but it contains CREATE TRIGGER.`
- A view over the bookings is refused the same way, naming `CREATE VIEW`.
- A mutation must begin with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`, so `WITH span AS (...) INSERT ...` is refused at both doors with `Action bookings.create is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE. Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.` The same CTE placed after `INSERT INTO bookings (...)` passes, which is the form this app uses.
- Two actions called one after the other are two transactions. A failure between them leaves the first write in place, and nothing in the SDK groups them.

The compromise is one `bookings` table where each row is one hour held, carrying the booking's metadata. `booking_ref` (room, day, start hour and creation time) groups the rows of one booking. Cancelling sets `cancelled_at` instead of deleting, so the cancelled rows stay as the activity log the second table would have held, and the unique index on `(room_id, day, slot)` is partial, `WHERE cancelled_at IS NULL`, so a cancelled hour can be booked again. The title and the requester are copied onto every hour of the booking. That duplication is the cost.

Which guard answers depends on who writes:

| Guard | Answer | When it fires |
| --- | --- | --- |
| `WHERE` in `bookings.create` (room exists, valid date, not in the past, whole hours, 1 to 4 hours, ends by 18:00) | The guard, `Not booked. The room or the day is not on the board, the span is not 1 to 4 whole hours between 08:00 and 18:00, or someone holds one of those hours.` | Every invalid request through the action |
| `NOT EXISTS` over the span in the same `WHERE` | The same guard | Any overlap, including the loser of a race |
| Partial unique index `bookings_one_holder` | `UNIQUE constraint failed: ...`, and the whole multi-row insert rolls back | Only if two statements interleave, or the owner writes SQL directly |
| `CHECK` on `day` and `slot` | `CHECK constraint failed: ...` | Only on owner SQL; the action's `WHERE` stops bad input first |

## Double booking under concurrency

The replay sends conflicting `bookings.create` calls from two staff sessions with `Promise.allSettled`. Exactly one wins every time, and the loser is refused with the create guard.

The guarantee comes from two places. The Kernel holds a per-Deployment write lock around every mutation, on the runtime endpoint and on HTTP routes (`action_lock` in `kernel-rs/kernel/src/lib.rs`), so the `NOT EXISTS` check and the insert of one request never interleave with another's. The partial unique index holds even without the lock, because SQLite checks it inside the statement. The Action Contract does not state that actions are serialized. A replacement backend that runs statements in parallel would still refuse the second holder, but the loser would see SQLite's `UNIQUE constraint failed` text instead of the guard, and `conform` would not notice the difference.

## Notifying staff

A booking emails its own booker (see [What worked](#what-worked)). Nothing else in the app tells another staff member that a room was booked, freed or checked in:

- `connect-src 'self'` stops the page from calling another origin.
- Besides account mail, the Kernel sends only a mutation's `notify`: to the app's contact email, to the caller, or to the accounts a changed row names in a column. This app declares only the notice to the caller. It could declare a notice on `bookings.cancel` to the column that holds the booker, but no notice reaches everyone who works in the office.
- A call action can send a request to another service from the Kernel. This app declares none.
- Signals are readable only by the owner through `signals_read`; the runtime endpoint does not list them.

What the app does instead:

- The staff view has a Recent activity list, the ten most recently changed bookings, cancellations included. The open board reloads it every 30 seconds while the tab is visible.
- A lobby screen can poll `GET /api/availability`.
- The owner can read `booking.created` and `booking.cancelled` Signals. They count only calls that changed a row, and each names who booked or cancelled.

A person who does not have the board open learns about a booking when they next open it.

## What the Kernel refused or could not do

Messages are quoted exactly as the replay receives them. The replay also runs the editor's Validation Gate in-process on the same files, and the Gate refuses each schema and action below with the same sentence, under `actions.json is not a valid server action program.` for an action.

| What the app tried | Door | Exact answer | Consequence |
| --- | --- | --- | --- |
| A trigger on `bookings` that writes a log row | Kernel and Gate | `schema.sql may only create tables and indexes, but it contains CREATE TRIGGER.` | One action writes one table. See [the compromise](#atomic-booking-and-the-one-statement-rule) |
| A view of taken hours | Kernel and Gate | `schema.sql may only create tables and indexes, but it contains CREATE VIEW.` | Each query repeats its own filter |
| A mutation that opens with `WITH span AS (...) INSERT ...` | Kernel and Gate | `Action bookings.create is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE. Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.` | Put the CTE after `INSERT INTO t (...)`, which passes |
| A cancel that also writes a second statement | Kernel and Gate | `Action bookings.cancel must contain one statement.` | No multi-statement transaction from the app |
| Telling the requester why a booking failed | Kernel | The guard `Not booked. The room or the day is not on the board, the span is not 1 to 4 whole hours between 08:00 and 18:00, or someone holds one of those hours.` | One statement cannot say which rule failed, so the guard names them all; overlap, past day, bad date, unknown room, too many hours and a span past 18:00 get one sentence. `RETURNING` is refused (`Action <name> cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.`) and there are no triggers to `RAISE`. Letting the unique index fail instead would show staff `UNIQUE constraint failed: bookings.room_id, bookings.day, bookings.slot` |
| Counting bookings with a Signal on `bookings.create` | Kernel | One `booking.created` Signal per booking made, naming the booker, and the guard refusals under Usage's `refused` | A Signal on a mutation records only when a row changed |
| Protecting `panel.checkin` with a signature | Kernel | `Action panel.checkin answers only on its HTTP route, POST /api/checkin.` for a visitor and for staff | A signed route is its action's only door. The contract lists no runtime caller for the action |
| A staff member checking in from the app with their session | Kernel | 401 `{"error":"invalid-signature"}` | A signed route takes the signature or a key unless its `callers` list `"session"`. The contract's route section lists only the callers the route takes |
| The panel's API key on the runtime endpoint | Kernel | 403 `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.` | A misconfigured program learns where its key works |
| An API key booking a room | Kernel | Not possible: keys work only on HTTP routes, and `bookings.create` is `authenticated`, which cannot declare `http` | A program, such as a calendar sync, cannot book on a person's behalf |
| Telling other staff about a booking | none | No notice reaches everyone in the office, and the app declares no call action | See [Notifying staff](#notifying-staff) |
| A confirmation email to the booker | Kernel | `"notify": { "to": "actor" }` on `bookings.create`. A booker who signed up in local mode has proved no address, so the booking succeeds and Usage counts `bookings.create: unverified-email` | Cara, invited, gets the email; Ana and Ben do not until they answer a link |
| Seeding rooms from `schema.sql` | Kernel | `schema.sql` may only create tables and indexes (not run by the replay) | The owner inserts rooms with `database_execute` after the first publish |
| `bun run konae -- publish` with an Allowlist | CLI | No refusal. `konae.json`'s `signupPolicy` goes as `initialSignupPolicy` on the first release | The replay publishes with `publishLocal` on a token without `manage_users`, and the Allowlist is in place before the first request |
| A whole-hour input | Kernel | No integer input type; `"type": "number"` accepts `13.5` | The statement checks `?4 = CAST(?4 AS INTEGER)`, and a CHECK on `typeof(slot)` backs it |
| "Not in the past" in the office's time | Kernel | The `now` binding is UTC milliseconds; there is no time zone | The check accepts the UTC day before today, so at 21:00 in New York, when the UTC date is already tomorrow, the person can still book their own today. The replay books it. The cost is the other side: east of UTC, just after midnight, yesterday is still accepted for a few hours. The page offers no past day and sends the browser's local date for its own lists |
| The booker's name | Kernel | The `actor` binding has only `id` | The app keeps a `staff` table of display names each person sets for themselves. A name is not verified |
| Typed columns in the Action Contract | `contract` | No warning and no `unnamed` entry, but 7 columns typed `unknown` | `bookings.day.booked_by`, `bookings.day.mine`, `bookings.mine.start`, `bookings.recent.start`, `bookings.recent.booked_by`, `bookings.recent.created_at`, `bookings.recent.cancelled_at`. Every expression column is aliased, so all are named; their types come from `coalesce`, `min`, `max` and a comparison, which the contract does not infer |
| `conform` with an API key alone | `conform` | No failure, and the checks that need a session skipped | Authenticated actions need `--token`. With both `--token` and `--key`, the key is sent only to a signed route whose secret was not given |

## Action Contract and conform

Write the contract from a copy of the app folder:

```sh
bun run konae -- contract --dir /tmp/booking --out /tmp/booking-contract
```

`action-contract.md` there lists each action's input, output, Signal, notice, callers, errors and limits. The replay runs `conform` three times against the published app with the CLI, from a contract file on disk:

| Run | Result |
| --- | --- |
| `--token <session> --secret panel_hook=<secret>` | No failure and no skip |
| `--token <session> --key <panel key>` | No failure and no skip |
| `--key <panel key>` alone | No failure; the checks that need a session are skipped |

`panel.checkin` answers only on its route, so conform probes its inputs and its guard there and not on the runtime endpoint. A scheduled mutation with no route refuses every caller, so conform expects `Action bookings.release runs only on its schedule.` instead of calling it.

The replay also runs the same contract in-process with the token and the key from fresh client addresses, with 0 failed. `conform` runs every mutation once with generated input. Here `bookings.create` gets a day that is not a date and is refused with its guard, which `conform` checks, and `panel.checkin` matches no booking, so the run leaves the board unchanged.

Every expression the queries return carries an `AS` alias, so the contract names every column and lists nothing under `unnamed`. Seven of those columns have the type `unknown`, listed in the table above.

## Screenshots

The replay with `KONAE_SHOWCASE_SCREENSHOTS` set writes the visitor's grid in light mode and the staff view in light and dark mode, after the staff member booked Birch from 14:00 to 16:00 from the grid.
