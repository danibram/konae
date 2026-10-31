# Habits

A personal habit tracker for one person who uses it from a phone and a computer. They sign up once, verify their email, and sign in on both devices. Each habit shows a seven-day strip to tick, the current streak, the best streak and the total days. Habits and check-ins belong to the account; nobody else can read or change them.

The app is a product probe. It uses Konae's primitives the way a creator would, and `replay.ts` records what they allowed and what they refused.

## Files

| File | What it holds |
| --- | --- |
| `index.html`, `styles.css`, `app.js` | The page. Plain CSS with the design system's neutral surfaces and indigo accent, light and dark from `prefers-color-scheme`. Listeners are attached in JavaScript, so the page runs under the Kernel's Content Security Policy. |
| `schema.sql` | `habits`, `checkins` and `days`. A check-in's `day` is a `YYYY-MM-DD` string with a `CHECK`, unique per habit. A `days` row is one person's closed day. |
| `schema.ts` | The Database Model the editor would hold, matching `schema.sql`. |
| `actions.json` | Twelve Server Actions. Nine are `authenticated`. Eight of them bind the actor; the ninth, `days.close`, declares a schedule and binds only `kernel.now`. Three are `anonymous` with an HTTP route that declares `"callers": ["key"]` and also bind the actor, for an API key. `habits.create`, `habits.import` and `habits.archive` declare a guard. |
| `replay.ts` | Starts its own Kernel, publishes the app and runs every flow below. |

## Primitives it uses

- A Public Deployment with Open signup, on an installation started with `KONAE_ACCOUNT_VERIFICATION=required` and `KONAE_MAIL_TRANSPORT=development`.
- `actor` bindings on every action, with ownership checked inside the SQL: `INSERT ... SELECT ... FROM habits WHERE id = ?1 AND user_id = ?2`.
- A `kernel.now` binding that limits check-ins to the last 30 days and tomorrow.
- A schedule: `days.close` declares `"schedule": { "daily": "00:05" }`, and the Kernel closes yesterday for each person, the habits they tracked and how many they ticked, with nobody calling. `days.recent` reads the closed days.
- A streak computed in one `SELECT` with `ROW_NUMBER() OVER (...)` nested in two subqueries, under a top-level `LIMIT 20`.
- The `habit.completed` Signal on `checkins.add` and on the `POST /api/checkins` route, with `habitId`, `day` and `device` as properties. The owner reads it with `signals_read` and the daily totals with `usage_read`.
- HTTP routes `POST /api/habits`, `GET /api/habits` and `POST /api/checkins`, each with `"callers": ["key"]`, called with an API key from `api_keys_create`.
- Guards: the twenty-first habit is refused with `You already track 20 habits. Archive one first.`, and an archive that changes nothing with `This habit is already archived, or it is not yours.`
- The editor's path, simulated over MCP: `project_create`, `project_sources_save`, then `release_plan` and `release_apply` with the editor's payload.
- `konae publish` from a folder that holds the same project's files.
- A scheduled copy to `KONAE_COPY_DIR` and `konae-kernel restore` into an empty data directory.
- `konae contract` and `konae conform`.
- The editor's Validation Gate (`src/agent/gate.ts` on sql.js) on the same files the Kernel receives.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
KONAE_SHOWCASE_PORT=21410 bun run examples/showcase/habits/replay.ts
```

The port defaults to 21410. The replay compiles the Kernel with `cargo build --offline`, so the first run takes a few minutes. The Kernels and CLI runs it starts ignore the shell's `KONAE_` variables. It uses random passwords and a random setup key, and it removes its Kernels, data directories, copies and folder when it exits. It exits 0 when every check passes.

- `KONAE_SHOWCASE_SCREENSHOTS=<dir>` writes `habits-light.png` and `habits-dark.png` of the signed-in page.
- `KONAE_SHOWCASE_BROWSER=0` skips the Chrome pass.

The replay runs in this order:

1. The Gate passes the capsule. The Gate and `release_plan` both refuse three variants: the streak query without its `LIMIT`, a `WITH RECURSIVE` calendar, and a Signal property named `user`.
2. The owner signs up with the setup key, which answers 202 in required mode, and verifies through the creator link in `mailbox.jsonl`.
3. The owner saves the sources at revision 1 and publishes them as the editor would, public with Open signup.
4. Ana and Ben sign up and verify through the mailbox. Ana signs in on a phone session and a laptop session.
5. Ana creates habits and check-ins from both sessions. Ben tries to read and change them.
6. The owner dates Ana's habits a week back, and the replay waits for `days.close` to close yesterday. Its Kernels run with `KONAE_SCHEDULE_MIN_SECONDS=2`, so the daily schedule runs every 2 seconds.
7. The owner issues an API key for the three routes, and the replay calls them with the key, with no credential, and with Ana's cookie, which the routes refuse.
8. The owner reads Signals and Usage.
9. `konae contract` writes the Action Contract and `konae conform` runs it against the Deployment.
10. The replay writes the project's files and a `konae.json` into a folder, changes the footer to "Build 2" and runs `konae publish` from that folder.
11. The replay waits for a copy newer than the last write, stops the Kernel, restores the copy into an empty directory and starts a Kernel on it.
12. Chrome loads the restored app signed out and signed in, in light and dark, and fails on any policy violation or page error. Signed in, it double-presses Add habit and then adds the twenty-first habit.

## What worked

- **Verified signup.** A signup answers 202 and sets no cookie. The pending account cannot sign in, and `actions_invoke` without a session answers `authentication-required`. A second signup for the pending address keeps the account ID, sends a new link and turns the first link into 410. After Ana verifies, a third signup for her address answers 202, sets no cookie and sends no mail. A guessed password answers 401, and both of Ana's sessions keep working.
- **Two devices.** Two sessions of one account see the same habits and check-ins. A second check-in for the same day from the other device changes 0 rows because of the `UNIQUE (habit_id, day)` key.
- **Rows bound to the actor.** Ben lists no habit, check-in or streak of Ana's. His check-in and uncheck on her habit each change 0 rows, and his archive is refused with `This habit is already archived, or it is not yours.` Sending `userId` as an input answers `Unexpected action input: userId`. A check-in and an uncheck stay without a guard on purpose: a second tick of the same day from the other device is not an error.
- **Streak in SQL.** With check-ins on days -6, -4, -3, -2, -1 and 0, the query returns current 5, best 5, total 6. The Gate and the Kernel accept it because of the top-level `LIMIT 20`.
- **Close the day.** `days.close` is one `INSERT ... SELECT ... GROUP BY user_id ... ON CONFLICT DO NOTHING` that writes, for each person, yesterday's UTC date, the habits they tracked before today and how many of them have a check-in that day. With nobody calling, the Kernel's timer closes Ana's yesterday as `{ "day": <yesterday>, "habits": 2, "done": 1 }`. Ben started his habit today, so he gets no row. A scheduled mutation with no route answers only to the timer, so Ben calling `days.close` himself is refused with `Action days.close runs only on its schedule.` `usage_read` lists every run as ok, and only the first changed a row: the later runs found the day closed.
- **API key as owner.** `POST /api/habits` with the key stores a habit whose `user_id` is the key's ID, `key_...`. `GET /api/habits` with the key returns only that habit. Since 2026-11-07 the routes declare `"callers": ["key"]`, the narrowest list for a phone shortcut: a visitor gets 401 `authentication-required` on POST and GET, and so does Ana's cookie from the app's own origin. The page checks in through `miniapp.actions.call`, never through the routes.
- **Signals and Usage.** `signals_read` returns one `habit.completed` per check-in that added a row, with `habitId`, `day`, `device` and `actor`, the ID of the account or key that checked in. `usage_read` returns the day's totals by action, route, signup, login and session.
- **Action Contract.** `konae contract` writes 12 actions with no warning. `konae conform` reported `78 passed, 0 failed, 2 skipped.` on 2026-11-07 after `days.close` and `days.recent` were added, from 72 before them, calling the three actor-bound routes with the API key. The count fell from 87 because the routed actions answer only on their routes now, so their inputs are probed there and not on the runtime endpoint. The two skips are the guards of `habits.create` and `habits.import`: below the limit of 20, conform's second identical call inserts again and cannot make the statement change nothing. The Chrome pass proves the limit's guard.
- **Publish from a folder.** `konae publish` from a folder with the project's files serves "Build 2" at the same URL. Ana's phone session, her rows and Ben's password survive, and the editor still saves the project's sources.
- **Copy and restore.** The restored Kernel serves Build 2 and keeps the accounts, passwords, rows, streaks, Signals, the closed day and the scheduled runs and the API key, which still reads `GET /api/habits`. It signs out Ana's sessions, the owner's session and the delegated token.
- **Content Security Policy.** Chrome loads the signed-out gate and the signed-in page with no violation or page error.
- **The page.** A double press of Add habit adds one habit, because the button stays disabled until the call answers. The twenty-first habit is refused on the page with the guard's sentence; before 2026-11-07 the page wrote that sentence itself from `{ changes: 0 }`. Every status line reads a failure through one `reason(error)` helper, the same in all five showcase apps.

## Usage the app cannot read

ADR-0010 v1 keeps Usage and Signals away from the app. That does not hurt the person. Their streaks and history come from their own `checkins` rows through Server Actions, so an in-app `usage_read` would add nothing they need. It cost the owner instead, in two ways the first replay measured. A Signal could not say which account caused it, so the owner could not count completions per person from `habit.completed`. And a Signal recorded whenever the statement ran: 4 of 12 `habit.completed` Signals came from calls that stored nothing, a second tick on the same day, a day outside the 30-day window, Ben's attempt on Ana's habit, and the API key's attempt on it. Both were fixed on 2026-11-06. The Signal lists `actor`, and a mutation records its Signal only when it changed a row, so the replay now reads 8 Signals for 8 check-ins, 7 naming Ana and 1 naming the key. The app still cannot show the owner's totals; that takes `usage_read` or `signals_read` in the Admin Interface.

## What the primitives refused or could not do

| Primitive | What happened | Exact message or answer |
| --- | --- | --- |
| Query validation, both doors | A streak computed with a recursive calendar is refused. A query must start with `SELECT`, so a `WITH` clause cannot lead. Fixed on 2026-11-06: both doors refuse it with the same words and point at a subquery. Before, the Gate said `Runtime query only allows: SELECT. Use schema.sql for structural changes.` and the Kernel `Action habits.streaks contains disallowed SQL.` | `Action habits.streaks is a query, so it must start with SELECT. Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).` |
| Query bound, both doors | A window function anywhere in the statement needs a top-level `LIMIT`, even when the outer `SELECT` groups to one row per habit. Same message on both doors. | `Action habits.streaks uses OVER, which keeps one result row per stored row, and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.` |
| Signals | Fixed on 2026-11-06: `actor` among a Signal's properties records the caller's ID. Another name that is not an input key, such as `user`, is still refused by the Gate and the Kernel. | `Action checkins.add names signal property user, which is not one of its input bindings.` |
| Signals | Fixed on 2026-11-06: a mutation records its Signal only when it changed a row. | 8 Signals for 8 stored check-ins. Before, 12. |
| Signals and Usage in the app | The runtime endpoint does not offer `usage_read` or `signals_read`, so the app cannot show the owner's totals or a person's own Signals. The replay calls both with Ana's app session. | `Unknown tool or insufficient permission.` |
| API key | A key acts as itself, never as a person. Its rows belong to `key_...`. A phone shortcut holding a key cannot tick Ana's habit, and Konae has no per-user key. | `POST /api/checkins` with the key on Ana's habit answers `{ "result": { "changes": 0, ... } }`. |
| HTTP route, no credential | Fixed in the app on 2026-11-07. An actor-bound route without `callers` refused a visitor with 400, not 401. The routes now declare `"callers": ["key"]`, so a visitor is refused before the action runs. A route that leaves `callers` out still answers 400. | 401 `{ "error": "authentication-required" }`. Before, 400 `{ "error": "This action requires an authenticated actor." }` |
| HTTP route with a cookie | The Kernel compares `Origin` with `Host`, and a hand-written client that sent `Host` without the port `Origin` carries ran as a visitor. No longer reachable here: since 2026-11-07 the routes take only a key, so any cookie gets 401. | 401 `authentication-required`. Before, 400 `This action requires an authenticated actor.` |
| `konae conform` | Fixed on 2026-11-06: the runner sends the key, or the session cookie, to a route that binds the caller's ID. | `PASS habits.import: answers POST /api/habits with an API key.` (same for `habits.mine` and `checkins.log`). Before, `SKIP ... Binds the caller's ID; ...` |
| `konae conform` | It runs every mutation once with generated input, so it adds a habit for the session's account and one for the key on the Deployment it checks. | No refusal. The docs say to point it at a test Deployment. |
| Required verification, signup | A `password` sent with a signup is accepted and ignored. The caller gets 202 and the password never signs in. | 202 `{ "result": { "status": "link-sent" } }`; login with that password answers 401. |
| Required verification, signup | A signup for an active address answers 202 and sends nothing, so the account holder never learns someone tried. | 202, no mail in `mailbox.jsonl`. |
| Required verification, login | Fixed on 2026-11-07. A pending account with no password still answers 401, and in the browser that asked for the link the message says to open it first; another browser gets the plain answer, so nobody learns whether the address has an account. | 401 `{ "error": "Email or password is incorrect. If you signed up with this email in this browser, open the link we emailed to choose your password first." }`. Was `Email or password is incorrect.` in every browser |
| CLI under required verification | Fixed on 2026-11-07: `konae login --verify-from <data folder>/mailbox.jsonl` creates the owner, answers the link from the development mailbox and saves the credential in one run, on a loopback Kernel only. The replay still verifies through the mailbox itself and creates tokens with `token_create`, which `docs/mcp.md` allows. | Was `This installation verifies email. Open the link sent to your address to choose your password, then run login again.` Without the flag the message now names both steps and the flag |
| Time | The only clock binding is `kernel.now` in UTC milliseconds, and `usage_read` days are UTC. The app stores the person's local day and the 30-day window uses UTC dates. A person east of UTC who checks in just after their midnight stores the new local day, while Usage files the call under the previous UTC day. | No refusal. No timezone binding exists. |
| Schedule and time | `days.close` closes the UTC day at 00:05 UTC, not the person's midnight. A schedule has no time zone, like `kernel.now`, so a person in New York sees yesterday closed at 19:05 their time. The replay cannot wait for 00:05, so it runs on a Kernel with the test clock. | No refusal. `"daily": "00:05"` is UTC. |
| Copy and restore | The restore signs out every session and delegated token. The owner signs in again and issues new tokens; a person signs in again on each device. | Stale app cookie: `auth_session` answers `role: "anonymous"`. Stale owner cookie and token: MCP refuses. |
