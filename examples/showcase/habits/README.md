# Habits

A personal habit tracker for one person who uses it from a phone and a computer. They sign up once, verify their email, and sign in on both devices. Each habit shows a seven-day strip to tick, the current streak, the best streak and the total days. Habits and check-ins belong to the account; nobody else can read or change them.

The app is a product probe. It uses Littlebox's primitives the way a creator would, and `replay.ts` records what they allowed and what they refused.

## Files

| File | What it holds |
| --- | --- |
| `index.html`, `styles.css`, `app.js` | The page. Plain CSS with the design system's neutral surfaces and indigo accent, light and dark from `prefers-color-scheme`. Listeners are attached in JavaScript, so the page runs under the Kernel's Content Security Policy. |
| `schema.sql` | `habits` and `checkins`. A check-in's `day` is a `YYYY-MM-DD` string with a `CHECK`, unique per habit. |
| `schema.ts` | The Database Model the editor would hold, matching `schema.sql`. |
| `actions.json` | Ten Server Actions. Seven are `authenticated` and bind the actor. Three are `anonymous` with an HTTP route and also bind the actor, for an API key. |
| `replay.ts` | Starts its own Kernel, publishes the app and runs every flow below. |

## Primitives it uses

- A Public Deployment with Open signup, on an installation started with `LITTLEBOX_ACCOUNT_VERIFICATION=required` and `LITTLEBOX_MAIL_TRANSPORT=development`.
- `actor` bindings on every action, with ownership checked inside the SQL: `INSERT ... SELECT ... FROM habits WHERE id = ?1 AND user_id = ?2`.
- A `kernel.now` binding that limits check-ins to the last 30 days and tomorrow.
- A streak computed in one `SELECT` with `ROW_NUMBER() OVER (...)` nested in two subqueries, under a top-level `LIMIT 20`.
- The `habit.completed` Signal on `checkins.add` and on the `POST /api/checkins` route, with `habitId`, `day` and `device` as properties. The owner reads it with `signals_read` and the daily totals with `usage_read`.
- HTTP routes `POST /api/habits`, `GET /api/habits` and `POST /api/checkins`, called with an API key from `api_keys_create`.
- The editor's managed path, simulated over MCP: `project_create` with `sourceAuthority: "managed"`, `project_sources_save`, then `release_plan` and `release_apply` with the editor's payload.
- `littlebox eject` and `littlebox publish` from the ejected folder.
- A scheduled copy to `LITTLEBOX_COPY_DIR` and `littlebox-kernel restore` into an empty data directory.
- `littlebox contract` and `littlebox conform`.
- The editor's Validation Gate (`src/agent/gate.ts` on sql.js) on the same files the Kernel receives.

## Run the replay

From the repository root, with Bun, Cargo and Google Chrome installed:

```sh
LITTLEBOX_SHOWCASE_PORT=21410 bun run examples/showcase/habits/replay.ts
```

The port defaults to 21410. The replay compiles the Kernel with `cargo build --offline`, so the first run takes a few minutes. The Kernels and CLI runs it starts ignore the shell's `LITTLEBOX_` variables. It uses random passwords and a random setup key, and it removes its Kernels, data directories, copies and ejected folder when it exits. It exits 0 when every check passes.

- `LITTLEBOX_SHOWCASE_SCREENSHOTS=<dir>` writes `habits-light.png` and `habits-dark.png` of the signed-in page.
- `LITTLEBOX_SHOWCASE_BROWSER=0` skips the Chrome pass.

The replay runs in this order:

1. The Gate passes the capsule. The Gate and `release_plan` both refuse three variants: the streak query without its `LIMIT`, a `WITH RECURSIVE` calendar, and a Signal property named `user`.
2. The owner signs up with the setup key, which answers 202 in required mode, and verifies through the creator link in `mailbox.jsonl`.
3. The owner saves the sources at revision 1 and publishes them as the editor would, public with Open signup.
4. Ana and Ben sign up and verify through the mailbox. Ana signs in on a phone session and a laptop session.
5. Ana creates habits and check-ins from both sessions. Ben tries to read and change them.
6. The owner issues an API key for the three routes, and the replay calls them with the key, with no credential, and with Ana's cookie.
7. The owner reads Signals and Usage.
8. `littlebox contract` writes the Action Contract and `littlebox conform` runs it against the Deployment.
9. `littlebox eject` writes the project into an empty folder. The replay changes the footer to "Build 2" and runs `littlebox publish` from that folder.
10. The replay waits for a copy newer than the last write, stops the Kernel, restores the copy into an empty directory and starts a Kernel on it.
11. Chrome loads the restored app signed out and signed in, in light and dark, and fails on any policy violation or page error.

## What worked

- **Verified signup.** A signup answers 202 and sets no cookie. The pending account cannot sign in, and `actions_invoke` without a session answers `authentication-required`. A second signup for the pending address keeps the account ID, sends a new link and turns the first link into 410. After Ana verifies, a third signup for her address answers 202, sets no cookie and sends no mail. A guessed password answers 401, and both of Ana's sessions keep working.
- **Two devices.** Two sessions of one account see the same habits and check-ins. A second check-in for the same day from the other device changes 0 rows because of the `UNIQUE (habit_id, day)` key.
- **Rows bound to the actor.** Ben lists no habit, check-in or streak of Ana's. His archive, check-in and uncheck on her habit each change 0 rows. Sending `userId` as an input answers `Unexpected action input: userId`.
- **Streak in SQL.** With check-ins on days -6, -4, -3, -2, -1 and 0, the query returns current 5, best 5, total 6. The Gate and the Kernel accept it because of the top-level `LIMIT 20`.
- **API key as owner.** `POST /api/habits` with the key stores a habit whose `user_id` is the key's ID, `key_...`. `GET /api/habits` with the key returns only that habit. A visitor gets 400 `This action requires an authenticated actor.` Ana's cookie runs the route as Ana when the request comes from the app's own origin, and as a visitor from another origin.
- **Signals and Usage.** `signals_read` returns every `habit.completed` with `habitId`, `day` and `device`. `usage_read` returns the day's totals by action, route, signup, login and session.
- **Action Contract.** `littlebox contract` writes 10 actions with no warning. `littlebox conform` reports `84 passed, 0 failed, 3 skipped.`
- **Eject.** The ejected folder holds `README.md`, `actions.json`, `app.js`, `index.html`, `littlebox.json`, `schema.sql`, `schema.ts` and `styles.css`, with `sourceAuthority: "external"` and the Kernel's URL. The republished build serves "Build 2" at the same URL. Ana's phone session, her rows and Ben's password survive. `project_sources_save` is refused, and `project_sources_read` returns the frozen sources with `readOnly: true`.
- **Copy and restore.** The restored Kernel serves Build 2 and keeps the accounts, passwords, rows, streaks, Signals, the ejected state and the API key, which still reads `GET /api/habits`. It signs out Ana's sessions, the owner's session and the delegated token.
- **Content Security Policy.** Chrome loads the signed-out gate and the signed-in page with no violation or page error.

## Usage the app cannot read

ADR-0010 v1 keeps Usage and Signals away from the app. That does not hurt the person. Their streaks and history come from their own `checkins` rows through Server Actions, so an in-app `usage_read` would add nothing they need. It costs the owner instead, in two ways the replay measured. First, a Signal cannot say which account caused it, because properties may only be input keys, so the owner cannot count active users or completions per person from `habit.completed`. Second, a Signal records whenever the statement runs, including when it changes no row. In the replay 4 of 12 `habit.completed` Signals came from calls that stored nothing: a second tick on the same day, a day outside the 30-day window, Ben's attempt on Ana's habit, and the API key's attempt on it. The Signal count overstates completions by half again. The owner can get exact numbers only with `database_execute` on the rows, which is the Admin Interface and not something the app can show.

## What the primitives refused or could not do

| Primitive | What happened | Exact message or answer |
| --- | --- | --- |
| Query validation, both doors | A streak computed with a recursive calendar is refused. A query must start with `SELECT`, so a `WITH` clause cannot lead. Both doors refuse it, with different words. | Gate: `Runtime query only allows: SELECT. Use schema.sql for structural changes.` Kernel: `Action habits.streaks contains disallowed SQL.` |
| Query bound, both doors | A window function anywhere in the statement needs a top-level `LIMIT`, even when the outer `SELECT` groups to one row per habit. Same message on both doors. | `Action habits.streaks uses OVER, which keeps one result row per stored row, and has no LIMIT. Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.` |
| Signals | A Signal cannot carry the actor. Naming a property that is not an input key is refused by the Gate and the Kernel. | `Action checkins.add names signal property user, which is not one of its input bindings.` |
| Signals | A mutation records its Signal when it changes 0 rows. There is no way to declare "record only when a row changed". | No refusal. 12 Signals for 8 stored check-ins. |
| Signals and Usage in the app | The runtime endpoint does not offer `usage_read` or `signals_read`, so the app cannot show the owner's totals or a person's own Signals. The replay calls both with Ana's app session. | `Unknown tool or insufficient permission.` |
| API key | A key acts as itself, never as a person. Its rows belong to `key_...`. A phone shortcut holding a key cannot tick Ana's habit, and Littlebox has no per-user key. | `POST /api/checkins` with the key on Ana's habit answers `{ "result": { "changes": 0, ... } }`. |
| HTTP route, no credential | An actor-bound route refuses a visitor with 400, not 401. | `{ "error": "This action requires an authenticated actor." }` |
| HTTP route with a cookie | The Kernel compares `Origin` with `Host`. A hand-written client that sends `Host` without the port `Origin` carries runs as a visitor. A browser sends both and is not affected. | 400 `This action requires an authenticated actor.` |
| `littlebox conform` | The runner skips a route that binds the caller's ID even when it was given `--key`. The replay calls all three routes with the key itself. | `SKIP habits.import: answers POST /api/habits. Binds the caller's ID; a route caller with an ID needs an API key or a session cookie.` (same for `habits.mine` and `checkins.log`) |
| `littlebox conform` | It runs every mutation once with generated input, so it adds a habit for the session's account and two `habit.completed` Signals on the Deployment it checks (12 became 14). | No refusal. The docs say to point it at a test Deployment. |
| Required verification, signup | A `password` sent with a signup is accepted and ignored. The caller gets 202 and the password never signs in. | 202 `{ "result": { "status": "link-sent" } }`; login with that password answers 401. |
| Required verification, signup | A signup for an active address answers 202 and sends nothing, so the account holder never learns someone tried. | 202, no mail in `mailbox.jsonl`. |
| Required verification, login | A pending account with no password answers 401 like a wrong password. The docs reserve 403 for a pending account with a correct legacy password. | 401 `{ "error": "Email or password is incorrect." }` |
| CLI under required verification | `littlebox login` cannot create the first owner in one step with `LITTLEBOX_SETUP_KEY`, because the signup answers 202. The replay verified the owner through the mailbox, created tokens with `token_create` from that session and passed them as `LITTLEBOX_MCP_TOKEN`, as `docs/mcp.md` allows. Read from `scripts/littlebox.ts`; not run. | `This installation verifies email. Open the link sent to your address to choose your password, then run login again.` |
| Time | The only clock binding is `kernel.now` in UTC milliseconds, and `usage_read` days are UTC. The app stores the person's local day and the 30-day window uses UTC dates. A person east of UTC who checks in just after their midnight stores the new local day, while Usage files the call under the previous UTC day. | No refusal. No timezone binding exists. |
| Copy and restore | The restore signs out every session and delegated token. The owner signs in again and issues new tokens; a person signs in again on each device. | Stale app cookie: `auth_session` answers `role: "anonymous"`. Stale owner cookie and token: MCP refuses. |
