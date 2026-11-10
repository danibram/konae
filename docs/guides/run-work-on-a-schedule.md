# Run work on a schedule

Declare `schedule` on a mutation that takes no input, and the Kernel's own timer runs it when nobody is looking.

## How it works

A schedule fits work such as releasing a hold that lapsed or closing a day. The Kernel's timer looks once a minute and runs the action at the first look after its period opens. A run has no caller, so an `actor` binding is null and the guard does not apply. A run takes the Deployment's data lock and has the same 2-second statement budget as a call.

## Steps

1. Pick the timing. `{ "every": "15m" }` runs on a period from `5m` to `24h`, counted from 00:00 UTC, so `15m` runs at :00, :15, :30 and :45. `{ "daily": "02:00" }` runs once a day at that UTC time. A cron string such as `"0 8 * * *"` is refused.
2. Write a mutation that binds only `kernel`, `actor` or literal values. An input binding is refused, because no caller can supply it. This one, from the habits app, closes each day at 00:05 UTC.

    ```json
    "days.close": {
      "kind": "mutation",
      "access": "authenticated",
      "schedule": { "daily": "00:05" },
      "statement": "INSERT INTO days(user_id, day, habits, done, closed_at) SELECT h.user_id, date(?1 / 1000, 'unixepoch', '-1 day'), count(*), count(c.id), ?1 FROM habits h LEFT JOIN checkins c ON c.habit_id = h.id AND c.day = date(?1 / 1000, 'unixepoch', '-1 day') WHERE h.archived_at IS NULL AND h.created_at < (?1 / 86400000) * 86400000 GROUP BY h.user_id ON CONFLICT(user_id, day) DO NOTHING",
      "bindings": [
        { "source": "kernel", "field": "now" }
      ]
    }
    ```

3. Make the statement safe to repeat. A run that changes no row had nothing to do and is recorded with `changes: 0`. A Kernel that was down for several periods runs the action once when it is back.
4. Read the results with a normal query. Nothing is pushed to a page. A `subscribe` key is refused, so the page polls a query.

    ```json
    "days.recent": {
      "kind": "query",
      "access": "authenticated",
      "statement": "SELECT day, habits, done FROM days WHERE user_id = ?1 ORDER BY day DESC LIMIT 30",
      "bindings": [{ "source": "actor", "field": "id" }]
    }
    ```

5. To watch a run without waiting, start `konae dev` with `KONAE_SCHEDULE_MIN_SECONDS`, a number from 1 to 300. Every schedule then runs once every that many seconds. The Kernel takes it only on a loopback address.

    ```sh
    KONAE_SCHEDULE_MIN_SECONDS=10 bun run konae -- dev --dir <folder>
    ```

6. Publish. Publishing never runs the action, because the first period the Kernel sees it in counts as done.

## Rules worth knowing

- Only the timer runs it. `miniapp.actions.call`, `actions_invoke` and the editor preview refuse it with `Action <name> runs only on its schedule.` It can also declare an `http` route, which then runs it.
- A Deployment in maintenance runs nothing and runs each due action once when it leaves maintenance.
- A run that changes a row can `notify` the app's contact email, or with `{ "column": "<name>" }` the app users its rows name. A notice to `actor` records `no-account`, because the timer has no caller.
- The Kernel keeps the last period it ran in `data/usage.sqlite`, so a restart, a copy, or an export and its import does not run it twice.

## Check it worked

- Read the runs with `usage_read` and the Deployment's `deploymentId`, plus `from` and `to` dates. It returns the newest 500 scheduled runs in the range with `action`, `at`, `outcome`, `changes` and, for a failed run, `error`. On `konae dev`, `bun run konae -- events --dir <folder>` lists them.
- Call the query that reads the rows the run wrote, through the page.
- After `release_apply`, read the Deployment back with `project_inspect`.

The editor preview has no schedule timer and no Usage store. Try schedules on `konae dev` or on a published Deployment.

## Read next

- [Email when a row changes](./email-when-a-row-changes.md)
- [Refuse a write with guard](./refuse-a-write-with-guard.md)
- [Server Actions](../server-actions.md)
- [Preview and Deployment differences](../divergences.md)
