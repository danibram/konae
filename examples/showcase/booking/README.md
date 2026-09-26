# Room board

A room booking board for a small office. Anyone with the link sees which rooms are free each hour from 08:00 to 18:00. Staff sign in with an address at the office's domain, book one to four hours, cancel their own bookings and see who holds a room. A door panel checks a booking in over a signed HTTP route.

This app is a product probe. It uses Littlebox the way a creator would and records what the platform let it do and what it refused. It changes nothing in the Kernel, the Validation Gate, the SDK or the editor.

## Primitives it uses

| Primitive | Where |
| --- | --- |
| Public Deployment with anonymous queries | `rooms.list` and `availability.day` answer a signed-out visitor with `{ room, hour }` only, no titles and no names |
| Authenticated mutations | `bookings.create`, `bookings.cancel` and `staff.name` bind the caller with `{ "source": "actor", "field": "id" }` |
| Allowlist signup by `@domain` | `littlebox.json` names `signupPolicy: { mode: "allowlist", allowlist: ["@office.example"] }` and `littlebox publish` installs it on the first release |
| One SQL statement per action | `bookings.create` is one `INSERT ... WITH RECURSIVE ... SELECT` that writes every hour of the span or none |
| CHECK constraints and a partial unique index | `schema.sql` refuses a fractional hour, an hour outside 08 to 17, an invalid date and a second holder of an hour |
| HTTP route, unsigned | `GET /api/availability?day=YYYY-MM-DD` for a lobby screen |
| HTTP route, signed, with an API key | `POST /api/checkin`, signed with the `panel_hook` secret, or called with a key that names `panel.checkin` |
| Signals | `booking.created` and `booking.cancelled`, read by the owner with `signals_read` |
| Action Contract and `conform` | Generated from `actions.json` and `schema.sql`, run with a session token and with an API key |

The owner adds rooms with `database_execute`, because `schema.sql` may only create tables and indexes.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
LITTLEBOX_SHOWCASE_PORT=19473 bun run examples/showcase/booking/replay.ts
```

The replay builds the Kernel offline, starts it on `LITTLEBOX_SHOWCASE_PORT` (default 19470) with a random setup key and a data directory under the system temp folder, and deletes both on exit. It refuses to start when something already answers on the port. `LITTLEBOX_SHOWCASE_BROWSER=0` skips Chrome. `LITTLEBOX_SHOWCASE_SCREENSHOTS=<dir>` writes `booking-visitor-light.png`, `booking-staff-light.png` and `booking-staff-dark.png` there.

It prints one `PASS` line per group and a JSON report, and exits 0 when all ten hold. The last run printed these lines, trimmed where marked `[...]`:

```text
PASS Validation Gate and Kernel. Both pass the Capsule and both refuse a trigger that writes the booking log, a mutation that starts with WITH, two statements in one action, a view of free hours. [...]
PASS Publish. Refused first with "Action panel.checkin signs with the secret panel_hook, which is not set for this project. Set it with secret_set before publishing."; published public with littlebox publish at [...] after secret_set, with littlebox.json's signupPolicy allowlist @office.example installed on the first release.
PASS Allowlist signup. @office.example signs up. pat@elsewhere.example, sam@sub.office.example and, under Closed, cara@office.example all get 403 {"error":"This app is not accepting new accounts for this email address."}. Existing staff still sign in under Closed.
PASS One-statement booking. A two-hour booking writes two rows in one INSERT ... WITH RECURSIVE ... SELECT. An overlap and a fractional hour, five hours, a span past 18:00, a past day, an impossible date, text that is not a date, an unknown room each return { changes: 0 } with no reason. [...]
PASS Public and staff views. A visitor and GET /api/availability get only { room, hour }; titles and names need a session, and authenticated actions answer authentication-required. [...]
PASS Double booking under concurrency. 20 rounds of two overlapping bookings sent together: one wins with 2 rows, the other gets { changes: 0 } (2/0, 0/2). Ten requests for one hour: 1,0,0,0,0,0,0,0,0,0. Owner SQL counts 1 holder.
PASS Signals. 61 bookings.create calls answered without error and 61 booking.created Signals exist, though only 24 bookings were made: a mutation that changed nothing still records its Signal.
PASS Check-in route. POST /api/checkin refuses an unsigned body (401 invalid-signature) and accepts a signature or the panel's API key. [...] A signed-out visitor calls panel.checkin through actions_invoke with no signature and checks in a booking (changes 1).
PASS Action Contract. [...] conform --token --secret: "84 passed, 0 failed, 0 skipped.". conform --token --key: "81 passed, 0 failed, 0 skipped, 1 rate-limit waits.". conform --key alone: "35 passed, 0 failed, 14 skipped, 1 rate-limit waits.".
PASS Chrome. The visitor and staff pages load under "connect-src 'self'" with no policy violation or page error; staff book two hours from the grid. [...]
```

## What worked

- **Allowlist by domain.** `ana@office.example` signs up. `pat@elsewhere.example` and `sam@sub.office.example` get exactly the answer a listed address gets under Closed: status 403 and `{"error":"This app is not accepting new accounts for this email address."}`. The replay compares the two bodies with `deepEqual`, so the refusal does not tell a stranger whether the domain or the policy stopped them. A domain entry does not match its subdomains. Closing signup does not sign out existing staff.
- **The policy is in place before the first request.** `littlebox publish` sends littlebox.json's `signupPolicy` as `initialSignupPolicy`, so the public app never runs Open.
- **Public reads stay narrow.** The anonymous query projects `room` and `hour` and nothing else; titles and names come only from `bookings.day`, which needs a session. A visitor calling `bookings.day`, `bookings.create` or `bookings.recent` gets `authentication-required`.
- **A span of hours in one statement.** `bookings.create` generates the hours with a recursive CTE inside the `INSERT`'s `SELECT`, guards them with `WHERE`, and writes every hour or none. Cancelling is one `UPDATE ... WHERE booking_ref = ? AND user_id = ?` over all the rows of a booking. Another member's cancel changes 0 rows.
- **No double booking.** Twenty rounds of two overlapping requests sent at once, and one round of ten requests for the same hour, each end with exactly one holder. The loser always gets `{ changes: 0 }`, never an error.
- **Backstops in the schema.** Owner SQL that skips the action's `WHERE` still meets `UNIQUE constraint failed: bookings.room_id, bookings.day, bookings.slot` and `CHECK constraint failed: typeof(slot) = 'integer' AND slot BETWEEN 8 AND 17`.
- **Routes.** `GET /api/availability` returns the visitor's rows as JSON. `POST /api/checkin` refuses an unsigned body with 401 `invalid-signature` and accepts a signature or the panel's API key. A staff session cookie sent without a signature gets 401 too.
- **Content Security Policy.** The visitor page and the staff page load in Chrome under the Kernel's policy with no violation and no page error, and a staff member books two hours by clicking the grid.

## Atomic booking and the one-statement rule

The obvious model has two tables: a `slots` table that holds one row per room and hour, and a `bookings` table that records who booked what, plus a log of changes. A booking writes one `bookings` row and N `slots` rows, and a cancel deletes the slots and writes a log row. Littlebox runs exactly one SQL statement per action, so none of those pairs can be one action:

- A second statement in the same action is refused: `Action bookings.cancel must contain one statement.`
- A trigger that writes the second table is refused by the schema authorizer: `schema may only create tables and indexes: not authorized`.
- A view over the bookings is refused the same way.
- A mutation must begin with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`, so `WITH span AS (...) INSERT ...` is refused with `Action bookings.create contains disallowed SQL.` The same CTE placed after `INSERT INTO bookings (...)` passes, which is the form this app uses.
- Two actions called one after the other are two transactions. A failure between them leaves the first write in place, and nothing in the SDK groups them.

The compromise is one `bookings` table where each row is one hour held, carrying the booking's metadata. `booking_ref` (room, day, start hour and creation time) groups the rows of one booking. Cancelling sets `cancelled_at` instead of deleting, so the cancelled rows stay as the activity log the second table would have held, and the unique index on `(room_id, day, slot)` is partial, `WHERE cancelled_at IS NULL`, so a cancelled hour can be booked again. The title and the requester are copied onto every hour of the booking. That duplication is the cost.

Which guard answers depends on who writes:

| Guard | Answer | When it fires |
| --- | --- | --- |
| `WHERE` in `bookings.create` (room exists, valid date, not in the past, whole hours, 1 to 4 hours, ends by 18:00) | `{ changes: 0 }` | Every invalid request through the action |
| `NOT EXISTS` over the span in the same `WHERE` | `{ changes: 0 }` | Any overlap, including the loser of a race |
| Partial unique index `bookings_one_holder` | `UNIQUE constraint failed: ...`, and the whole multi-row insert rolls back | Only if two statements interleave, or the owner writes SQL directly |
| `CHECK` on `day` and `slot` | `CHECK constraint failed: ...` | Only on owner SQL; the action's `WHERE` stops bad input first |

## Double booking under concurrency

The replay sends conflicting `bookings.create` calls from two staff sessions with `Promise.allSettled`. Exactly one wins every time, and the loser gets `{ changes: 0 }`.

The guarantee comes from two places. The Kernel holds a per-Deployment database lock around every action statement, on the runtime endpoint (`kernel-rs/src/main.rs:852`) and on HTTP routes (`kernel-rs/src/http.rs:778`), so the `NOT EXISTS` check and the insert of one request never interleave with another's. The partial unique index holds even without the lock, because SQLite checks it inside the statement. The Action Contract does not state that actions are serialized. A replacement backend that runs statements in parallel would still refuse the second holder, but the loser would see SQLite's `UNIQUE constraint failed` text instead of `{ changes: 0 }`, and `conform` would not notice the difference.

## Notifying staff

A webhook out is not a primitive. Nothing in an app can tell a staff member that a room was booked, freed or checked in:

- `connect-src 'self'` stops the page from calling another origin.
- An action runs one SQL statement or records a Signal. There is no outbound HTTP and no queue.
- The only email the Kernel sends is account mail: verification, recovery and invitations. An app cannot send its own.
- Signals are readable only by the owner through `signals_read`; the runtime endpoint does not list them.

What the app does instead:

- The staff view has a Recent activity list, the ten most recently changed bookings, cancellations included. The open board reloads it every 30 seconds while the tab is visible.
- A lobby screen can poll `GET /api/availability`.
- The owner can read `booking.created` and `booking.cancelled` Signals. Those also count calls that changed nothing; see the table below.

A person who does not have the board open learns about a booking when they next open it.

## What the primitives refused or could not do

Messages are quoted exactly as the replay received them. "Gate" is the editor's Validation Gate, run in-process by the replay on the same files the Kernel was given.

| What the app tried | Door | Exact answer | Consequence |
| --- | --- | --- | --- |
| A trigger on `bookings` that writes a log row | Kernel | `schema may only create tables and indexes: not authorized` | One action writes one table. See [the compromise](#atomic-booking-and-the-one-statement-rule) |
| The same trigger | Gate | `schema.sql may only create tables and indexes, but it contains CREATE TRIGGER. The Kernel applies schema.sql with an authorizer that allows CREATE TABLE and CREATE [UNIQUE] INDEX and nothing else, [...]` | Both doors refuse, with different text |
| A view of taken hours | Kernel | `schema may only create tables and indexes: not authorized` | Each query repeats its own filter |
| The same view | Gate | `schema.sql may only create tables and indexes, but it contains CREATE VIEW. [...]` | Different text again |
| A mutation that opens with `WITH span AS (...) INSERT ...` | Kernel | `Action bookings.create contains disallowed SQL.` | Put the CTE after `INSERT INTO t (...)`, which passes |
| The same mutation | Gate | `actions.json is not a valid server action program. Runtime execute only allows: DELETE, INSERT, REPLACE, UPDATE. Use schema.sql for structural changes.` | The Gate's hint points at schema.sql, which is not where a CTE belongs |
| A cancel that also writes a second statement | Kernel | `Action bookings.cancel must contain one statement.` | No multi-statement transaction from the app |
| The same action | Gate | `actions.json is not a valid server action program. Server actions must contain exactly one SQL statement.` | Same rule, different text |
| Telling the requester why a booking failed | Kernel | `{ changes: 0 }`, nothing else | Overlap, past day, bad date, unknown room, too many hours and a span past 18:00 all look the same. `RETURNING` is refused (`Action <name> cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.`) and there are no triggers to `RAISE`. Letting the unique index fail instead would show staff `UNIQUE constraint failed: bookings.room_id, bookings.day, bookings.slot` |
| Counting bookings with a Signal on `bookings.create` | Kernel | No refusal. 61 calls, 61 `booking.created` Signals, 24 bookings made | A Signal on a mutation records whenever the statement ran without error, including `{ changes: 0 }`. The owner cannot tell a booking from a refused attempt |
| Protecting `panel.checkin` with a signature | Kernel | No refusal. A signed-out visitor calls it through `actions_invoke` with no signature and gets `{ changes: 1 }` | The signature guards only `POST /api/checkin`. Any visitor of a public app can check in any booking through the runtime endpoint. Making the action `authenticated` closes that door and the route with it: `Action <name> is authenticated, so it cannot declare http. Only anonymous actions answer on an HTTP route.` The contract does list "visitor" on the runtime endpoint for this action |
| A staff member checking in from the app with their session | Kernel | 401 `{"error":"invalid-signature"}` | Matches docs/mcp.md: a person needs the signature too. The contract's route section lists "person (the app session cookie, from the app's own origin)" as a caller without saying the signature is still required. A 401 on an action with no actor binding does not show whether the Kernel read the cookie at all |
| The panel's API key on the runtime endpoint | Kernel | No refusal. `availability.day` answers as for a visitor; `bookings.mine` answers `authentication-required` | docs/mcp.md says the runtime endpoint "does not accept" a key. It ignores it instead, so a misconfigured program gets visitor data without an error |
| An API key booking a room | Kernel | Not possible: keys work only on HTTP routes, and `bookings.create` is `authenticated`, which cannot declare `http` | A program, such as a calendar sync, cannot book on a person's behalf |
| A webhook out to notify staff | none | No primitive exists | See [Notifying staff](#notifying-staff) |
| Seeding rooms from `schema.sql` | Kernel | `schema.sql` may only create tables and indexes (docs/mcp.md; not run by the replay) | The owner inserts rooms with `database_execute` after the first publish |
| `bun run littlebox -- publish` with an Allowlist | CLI | Fixed on 2026-11-06. `publishLocal` sent no `initialSignupPolicy`, so a public app published with the CLI started Open until the owner called `signup_policy_set`, and the replay published over MCP to avoid that window | `littlebox.json` now names `project.manifest.signupPolicy` and the replay publishes with `publishLocal` on a token without `manage_users`; the Allowlist is in place before the first request |
| A whole-hour input | Kernel | No integer input type; `"type": "number"` accepts `13.5` | The statement checks `?4 = CAST(?4 AS INTEGER)`, and a CHECK on `typeof(slot)` backs it |
| "Not in the past" in the office's time | Kernel | The `now` binding is UTC milliseconds; there is no time zone | The past-day check compares with the UTC date, so for a few hours around midnight it accepts or refuses a day by the wrong calendar. The app sends the browser's local date for its own lists |
| The booker's name | Kernel | The `actor` binding has only `id` | The app keeps a `staff` table of display names each person sets for themselves. A name is not verified |
| Typed columns in the Action Contract | `contract` | No warning and no `unnamed` entry, but 7 columns typed `unknown` | `bookings.day.booked_by`, `bookings.day.mine`, `bookings.mine.start`, `bookings.recent.start`, `bookings.recent.booked_by`, `bookings.recent.created_at`, `bookings.recent.cancelled_at`. Every expression column is aliased, so all are named; their types come from `coalesce`, `min`, `max` and a comparison, which the contract does not infer |
| `conform` with an API key alone | `conform` | `35 passed, 0 failed, 14 skipped` | Authenticated actions need `--token`. With both `--token` and `--key`, the key is sent only to a signed route whose secret was not given |

## Action Contract and conform

`bun run littlebox -- contract --dir <copy of this folder>` writes the contract below. The replay runs `conform` three times against the published app with the CLI, from a contract file on disk:

| Run | Result |
| --- | --- |
| `--token <session> --secret panel_hook=<secret>` | 84 passed, 0 failed, 0 skipped |
| `--token <session> --key <panel key>` | 81 passed, 0 failed, 0 skipped |
| `--key <panel key>` alone | 35 passed, 0 failed, 14 skipped |

It also runs the same contract in-process with the token and the key from fresh client addresses, with 0 failed. `conform` runs every mutation once with generated input. Here `bookings.create` gets a day that is not a date and answers `{ changes: 0 }`, and `panel.checkin` matches no booking, so the run leaves the board unchanged.

Every expression the queries return carries an `AS` alias, so the contract names every column and lists nothing under `unnamed`. Seven of those columns have the type `unknown`, listed in the table above.

## Screenshots

The replay with `LITTLEBOX_SHOWCASE_SCREENSHOTS` set writes the visitor's grid in light mode and the staff view in light and dark mode, after the staff member booked Birch from 14:00 to 16:00 from the grid.

<details>
<summary>The Markdown Action Contract</summary>

### Action Contract

Generated from actions.json and schema.sql. action-contract.json is the machine-readable form and action-contract.schema.json its schema. Do not edit either; change the Capsule and generate them again.

10 actions. Every action runs on the runtime endpoint `/_littlebox/mcp` as the `actions_invoke` tool, which `miniapp.actions.call(name, input)` calls.

- A query answers an array of row objects keyed by the column names listed. A mutation answers { changes, lastInsertId }.
- Column types are SQLite affinities read from schema.sql. SQLite does not enforce them, so a value can still arrive as another JSON type when it was stored as one.
- Nullability follows NOT NULL and INTEGER PRIMARY KEY in schema.sql. A column read through the right side of a LEFT JOIN is nullable.
- A SQLite error, such as a UNIQUE constraint, is returned with SQLite's own text.
- On a private Deployment every runtime call needs a session, including anonymous actions. An HTTP route answers by the action's access level whatever the visibility.

| Action | Kind | Access | HTTP |
| --- | --- | --- | --- |
| `availability.day` | query | anonymous | `GET /api/availability` |
| `bookings.cancel` | mutation | authenticated |  |
| `bookings.create` | mutation | authenticated |  |
| `bookings.day` | query | authenticated |  |
| `bookings.mine` | query | authenticated |  |
| `bookings.recent` | query | authenticated |  |
| `panel.checkin` | mutation | anonymous | `POST /api/checkin` |
| `rooms.list` | query | anonymous |  |
| `staff.me` | query | authenticated |  |
| `staff.name` | mutation | authenticated |  |

#### availability.day

A query declared anonymous.

```sql
SELECT room_id AS room, slot AS hour FROM bookings WHERE day = ?1 AND cancelled_at IS NULL ORDER BY room_id, slot LIMIT 500
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `day` | string | yes | 10 UTF-16 code units |

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `room` | text | no | bookings.room_id |
| `hour` | integer | no | bookings.slot |

##### Callers

Runtime endpoint: visitor (none; a public Deployment only; actor none); person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

HTTP route `GET /api/availability` on the Deployment host. Input is the query string, each value typed by its input's declared type. It answers `{ "result": ... }` with the same output, or `{ "error": "<message>" }` with a status.

Callers: visitor (none; actor none); service (Authorization: Bearer <API key that names this action>; it replaces the signature; actor key id); person (the app session cookie, from the app's own origin; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` | 400 | both | input has a key no input binding declares |
| `Missing action input: day` | 400 | both | a required input is omitted |
| `day must be a non-empty string.` | 400 | both | a string input is not a string, or is empty or only whitespace |
| `day is too long.` | 400 | both | a string input is longer than its maxLength in UTF-16 code units |
| `Query result exceeds 2 MiB or 1000 rows.` | 400 | both | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |
| `invalid-api-key` | 401 | http | Authorization carries an lbk_ key that is unknown or revoked (401) |
| `This API key may not call availability.day.` | 403 | http | the key does not name the route's action (403) |
| `Repeated query parameter: <key>` | 400 | http | a GET names one input twice (400) |
| `rate-limited` | 429 | http | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 500 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### bookings.cancel

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
UPDATE bookings SET cancelled_at = ?3 WHERE booking_ref = ?1 AND user_id = ?2 AND cancelled_at IS NULL
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `ref` | string | yes | 120 UTF-16 code units |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: ref` |  | runtime | a required input is omitted |
| `ref must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `ref is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### bookings.create

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
INSERT INTO bookings (booking_ref, room_id, day, slot, user_id, title, created_at)
WITH RECURSIVE span(slot) AS (SELECT ?4 UNION ALL SELECT slot + 1 FROM span WHERE slot + 1 < ?4 + min(?5, 4))
SELECT printf('%s/%s/%d/%d', r.id, ?3, ?4, ?7), r.id, ?3, span.slot, ?1, ?6, ?7
FROM rooms r, span
WHERE r.id = ?2
  AND date(?3) IS ?3 AND ?3 >= date(?7 / 1000, 'unixepoch')
  AND ?4 = CAST(?4 AS INTEGER) AND ?5 = CAST(?5 AS INTEGER)
  AND ?4 >= 8 AND ?5 BETWEEN 1 AND 4 AND ?4 + ?5 <= 18
  AND NOT EXISTS (SELECT 1 FROM bookings b WHERE b.room_id = ?2 AND b.day = ?3 AND b.slot >= ?4 AND b.slot < ?4 + ?5 AND b.cancelled_at IS NULL)
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `room` | string | yes | 40 UTF-16 code units |
| `day` | string | yes | 10 UTF-16 code units |
| `start` | number | yes |  |
| `hours` | number | yes |  |
| `title` | string | yes | 80 UTF-16 code units |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: room` |  | runtime | a required input is omitted |
| `room must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `room is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: day` |  | runtime | a required input is omitted |
| `day must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `day is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: start` |  | runtime | a required input is omitted |
| `start must be a number.` |  | runtime | a number input is not a JSON number |
| `Missing action input: hours` |  | runtime | a required input is omitted |
| `hours must be a number.` |  | runtime | a number input is not a JSON number |
| `Missing action input: title` |  | runtime | a required input is omitted |
| `title must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `title is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### bookings.day

A query declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
SELECT b.booking_ref AS ref, b.room_id AS room, b.slot AS hour, b.title AS title, coalesce(s.display, 'A colleague') AS booked_by, b.user_id = ?1 AS mine, b.checked_in_at AS checked_in_at FROM bookings b LEFT JOIN staff s ON s.user_id = b.user_id WHERE b.day = ?2 AND b.cancelled_at IS NULL ORDER BY b.room_id, b.slot LIMIT 500
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `day` | string | yes | 10 UTF-16 code units |

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `ref` | text | no | bookings.booking_ref |
| `room` | text | no | bookings.room_id |
| `hour` | integer | no | bookings.slot |
| `title` | text | no | bookings.title |
| `booked_by` | unknown | unknown |  |
| `mine` | unknown | unknown |  |
| `checked_in_at` | integer | yes | bookings.checked_in_at |

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: day` |  | runtime | a required input is omitted |
| `day must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `day is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 500 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### bookings.mine

A query declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
SELECT booking_ref AS ref, room_id AS room, day, min(slot) AS start, count(*) AS hours, title FROM bookings WHERE user_id = ?1 AND day >= ?2 AND cancelled_at IS NULL GROUP BY booking_ref ORDER BY day, start LIMIT 50
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `from` | string | yes | 10 UTF-16 code units |

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `ref` | text | no | bookings.booking_ref |
| `room` | text | no | bookings.room_id |
| `day` | text | no | bookings.day |
| `start` | unknown | unknown |  |
| `hours` | integer | no |  |
| `title` | text | no | bookings.title |

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: from` |  | runtime | a required input is omitted |
| `from must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `from is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 50 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### bookings.recent

A query declared authenticated.

```sql
SELECT b.booking_ref AS ref, b.room_id AS room, b.day AS day, min(b.slot) AS start, count(*) AS hours, b.title AS title, coalesce(s.display, 'A colleague') AS booked_by, max(b.created_at) AS created_at, max(b.cancelled_at) AS cancelled_at FROM bookings b LEFT JOIN staff s ON s.user_id = b.user_id GROUP BY b.booking_ref ORDER BY max(coalesce(b.cancelled_at, b.created_at)) DESC LIMIT 10
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `ref` | text | no | bookings.booking_ref |
| `room` | text | no | bookings.room_id |
| `day` | text | no | bookings.day |
| `start` | unknown | unknown |  |
| `hours` | integer | no |  |
| `title` | text | no | bookings.title |
| `booked_by` | unknown | unknown |  |
| `created_at` | unknown | unknown |  |
| `cancelled_at` | unknown | unknown |  |

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 10 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### panel.checkin

A mutation declared anonymous.

```sql
UPDATE bookings SET checked_in_at = ?4 WHERE room_id = ?1 AND day = ?2 AND slot = ?3 AND cancelled_at IS NULL AND checked_in_at IS NULL
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `room` | string | yes | 40 UTF-16 code units |
| `day` | string | yes | 10 UTF-16 code units |
| `hour` | number | yes |  |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: visitor (none; a public Deployment only; actor none); person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

HTTP route `POST /api/checkin` on the Deployment host. Input is a JSON object body of at most 64 KiB. It answers `{ "result": ... }` with the same output, or `{ "error": "<message>" }` with a status.

A signed request carries `X-Panel-Signature: t=<unix seconds>,v1=<hex HMAC-SHA256 of "<t>.<raw body>" keyed with the secret>`. The secret is named `panel_hook` and set by the owner; it is never in this file. `t` must be within 300 seconds of the server's clock, and each signature is accepted once.

Callers: visitor (the X-Panel-Signature signature; actor none); service (Authorization: Bearer <API key that names this action>; it replaces the signature; actor key id); person (the app session cookie, from the app's own origin; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` | 400 | both | input has a key no input binding declares |
| `Missing action input: room` | 400 | both | a required input is omitted |
| `room must be a non-empty string.` | 400 | both | a string input is not a string, or is empty or only whitespace |
| `room is too long.` | 400 | both | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: day` | 400 | both | a required input is omitted |
| `day must be a non-empty string.` | 400 | both | a string input is not a string, or is empty or only whitespace |
| `day is too long.` | 400 | both | a string input is longer than its maxLength in UTF-16 code units |
| `Missing action input: hour` | 400 | both | a required input is omitted |
| `hour must be a number.` | 400 | both | a number input is not a JSON number |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |
| `invalid-signature` | 401 | http | a signed route gets no signature, a wrong one, or a t more than 300 seconds from the Kernel's clock (401) |
| `replayed-signature` | 409 | http | the same signature arrives a second time (409) |
| `invalid-api-key` | 401 | http | Authorization carries an lbk_ key that is unknown or revoked (401) |
| `This API key may not call panel.checkin.` | 403 | http | the key does not name the route's action (403) |
| `The request body exceeds 64 KiB.` | 413 | http | a POST body is larger (413) |
| `The request body must be a JSON object.` | 400 | http | a POST body does not parse as JSON (400) |
| `rate-limited` | 429 | http | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.
- A route body of at most 64 KiB.

#### rooms.list

A query declared anonymous.

```sql
SELECT id, name, seats FROM rooms ORDER BY name LIMIT 50
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `id` | text | yes | rooms.id |
| `name` | text | no | rooms.name |
| `seats` | integer | no | rooms.seats |

##### Callers

Runtime endpoint: visitor (none; a public Deployment only; actor none); person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 50 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### staff.me

A query declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
SELECT display FROM staff WHERE user_id = ?1 LIMIT 1
```

##### Input

None. Send `{}`.

##### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `display` | text | no | staff.display |

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- LIMIT 1 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### staff.name

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
INSERT INTO staff (user_id, display) VALUES (?1, ?2) ON CONFLICT(user_id) DO UPDATE SET display = excluded.display
```

##### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `display` | string | yes | 60 UTF-16 code units |

##### Output

`{ changes, lastInsertId }`, both integers.

##### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

##### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to /_littlebox/login |
| `Sign in to use Littlebox MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: display` |  | runtime | a required input is omitted |
| `display must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `display is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, or 120 from one signed-in user |

##### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

#### Any action

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Server action not declared.` |  | runtime | actions.json declares no action with that name |
| `Arguments must be an object.` |  | runtime | input is not an object, e.g. null |
| `No route accepts POST on this path.` | 405 | http | a POST reaches a path no action binds (405) |

</details>
