//! Scheduled work, ADR-0017.
//!
//! A mutation may declare `"schedule": { "every": "15m" }` or
//! `"schedule": { "daily": "02:00" }` (UTC). The Kernel runs it from one timer
//! task inside its one process, as `copies.rs` runs copies: no process per
//! app, no code of the app's own. Each tick reads the Deployments the Kernel
//! holds, so a republish takes effect at the next tick.
//!
//! Time is cut into slots: `every` cuts it into periods counted from
//! 1970-01-01 00:00 UTC, and `daily` into days that start at its time. An
//! action runs once in a slot, at the first tick after the slot opens. The
//! slot it last ran for is kept in the Deployment's `usage.sqlite`, so a
//! restart, a copy, a package or a migration resumes from it and a daily job
//! does not run twice. A Kernel that was down for many slots runs the action
//! once, not once per missed slot. The first slot an action is seen in counts
//! as done, so publishing never runs it.
//!
//! A run takes the Deployment's data lock and runs the statement as an
//! action call does, with the 2-second statement budget. The timer runs one
//! Deployment's actions one after another and skips a Deployment whose last
//! tick is still running, so runs never pile up. A Deployment in maintenance
//! is skipped and runs once when it leaves it. A run binds `kernel.now`, its
//! literals and NULL for `actor.id`, since nobody called it; a statement can
//! test `?n IS NULL` to tell the timer from a person, because a call from a
//! person always binds an ID. A guard does not apply: a run that changes no
//! row had nothing to do. A Signal is recorded when the run changed a row.
//! Each run is recorded under Usage's `scheduled` kind with its outcome and
//! changes. A run that fails leaves its slot open and the next tick tries
//! again.
//!
//! `parseSchedule` in src/kernel/actions.ts makes the same checks in the same
//! order with the same messages, and kernel-rs/kernel/src/schedule-cases.json holds
//! the cases both test suites run.

use super::refusal::{refuse, Refusal};
use super::{usage, ActionDefinition, ActionProgram, AppState};
use rusqlite::types::Value as SqlValue;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The shortest period a declaration may name, in minutes.
pub const MIN_MINUTES: u64 = 5;
/// The longest period, one day.
pub const MAX_MINUTES: u64 = 24 * 60;
const DAY_MS: u64 = 86_400_000;
/// How often the timer looks for due work on a real clock.
const TICK: Duration = Duration::from_secs(60);
/// The test clock's variable and its largest value.
const TEST_CLOCK: &str = "KONAE_SCHEDULE_MIN_SECONDS";
const MAX_TEST_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schedule {
    Every { minutes: u64 },
    Daily { minute_of_day: u64 },
}

impl Schedule {
    /// The declaration as the store keeps it, such as `every 15m` or `daily 02:00`.
    pub fn key(&self) -> String {
        match self {
            Schedule::Every { minutes } => format!("every {minutes}m"),
            Schedule::Daily { minute_of_day } => format!("daily {:02}:{:02}", minute_of_day / 60, minute_of_day % 60),
        }
    }

    /// The slot that holds `now`: whole periods since the first slot opened.
    fn slot(&self, now: u64) -> i64 {
        let (period, offset) = match self {
            Schedule::Every { minutes } => (minutes * 60_000, 0),
            Schedule::Daily { minute_of_day } => (DAY_MS, minute_of_day * 60_000),
        };
        (now as i64 - offset as i64).div_euclid(period as i64)
    }
}

/// When scheduled work runs. A real clock follows each declaration; the test
/// clock runs every declared schedule once every few seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    test_seconds: Option<u64>,
}

impl Clock {
    /// Reads `KONAE_SCHEDULE_MIN_SECONDS`. It shortens every schedule to
    /// that many seconds so a test can watch a run happen, and the Kernel
    /// refuses to start with it unless it listens only on a loopback address.
    pub fn from_env(bind: &str) -> anyhow::Result<Self> {
        Self::parse(bind, std::env::var(TEST_CLOCK).ok().as_deref())
    }

    fn parse(bind: &str, value: Option<&str>) -> anyhow::Result<Self> {
        let Some(value) = value.filter(|value| !value.is_empty()) else { return Ok(Self { test_seconds: None }) };
        if !super::is_local_host(bind) {
            anyhow::bail!("KONAE_SCHEDULE_MIN_SECONDS is for tests, and KONAE_BIND={bind} is reachable from other machines. Remove it, or bind the Kernel to 127.0.0.1.");
        }
        let seconds = value.parse::<u64>().ok().filter(|seconds| (1..=MAX_TEST_SECONDS).contains(seconds));
        let Some(seconds) = seconds else {
            anyhow::bail!("KONAE_SCHEDULE_MIN_SECONDS must be a whole number of seconds from 1 to 300.");
        };
        Ok(Self { test_seconds: Some(seconds) })
    }

    fn tick(&self) -> Duration {
        match self.test_seconds {
            Some(seconds) => Duration::from_millis((seconds * 250).clamp(250, 60_000)),
            None => TICK,
        }
    }

    /// The slot that holds `now`, and the key the store keeps the schedule
    /// under. The test clock's key names its period, so a Kernel started
    /// without it later starts the schedule afresh.
    fn place(&self, schedule: &Schedule, now: u64) -> (i64, String) {
        match self.test_seconds {
            Some(seconds) => ((now / (seconds * 1000)) as i64, format!("{} (test clock {seconds}s)", schedule.key())),
            None => (schedule.slot(now), schedule.key()),
        }
    }
}

/// Parses and checks one action's `schedule` key. `Ok(None)` when it has none.
pub fn declaration(name: &str, action: &ActionDefinition) -> Result<Option<Schedule>, Refusal> {
    let Some(value) = action.schedule.as_ref() else { return Ok(None) };
    // Each message is written out so src/sdk/contract.test.ts finds it.
    if action.kind == "query" {
        return Err(refuse("schedule.not-mutation", format!("Action {name} is a query, so it cannot declare a schedule. Declare it on a mutation.")));
    }
    if action.kind != "mutation" {
        return Err(refuse("schedule.not-mutation", format!("Action {name} is a signal action, so it cannot declare a schedule. Declare it on a mutation.")));
    }
    if action.bindings.iter().any(|binding| binding.source == "input") {
        return Err(refuse("schedule.input", format!(
            "Action {name} declares a schedule, so it cannot take input. A scheduled run has no caller to supply it; bind only actor, kernel and literal values."
        )));
    }
    let invalid = || {
        refuse("schedule.invalid", format!(r#"Action {name} has an invalid schedule. Declare "every" with minutes or hours, such as "15m" or "6h", or "daily" with a UTC time, such as "02:00"."#))
    };
    let record = value.as_object().filter(|record| record.len() == 1).ok_or_else(invalid)?;
    let (key, text) = record.iter().next().map(|(key, text)| (key.as_str(), text.as_str())).ok_or_else(invalid)?;
    let text = text.ok_or_else(invalid)?;
    match key {
        "every" => {
            let (digits, unit) = text.split_at(text.len().saturating_sub(1));
            let well_formed = (1..=5).contains(&digits.len())
                && !digits.starts_with('0')
                && digits.bytes().all(|byte| byte.is_ascii_digit())
                && (unit == "m" || unit == "h");
            if !well_formed {
                return Err(invalid());
            }
            let minutes = digits.parse::<u64>().map_err(|_| invalid())? * if unit == "h" { 60 } else { 1 };
            if minutes < MIN_MINUTES {
                return Err(refuse("schedule.too-often", format!(r#"Action {name} runs every {text}, more often than every 5 minutes. Use "5m" or longer."#)));
            }
            if minutes > MAX_MINUTES {
                return Err(refuse("schedule.too-rare", format!(r#"Action {name} runs every {text}, less often than once a day. Use "daily" or a period of 24 hours or less."#)));
            }
            Ok(Some(Schedule::Every { minutes }))
        }
        "daily" => {
            let bytes = text.as_bytes();
            let digit = |index: usize| bytes.get(index).filter(|byte| byte.is_ascii_digit()).map(|byte| u64::from(byte - b'0'));
            let time = match (bytes.len(), bytes.get(2), digit(0), digit(1), digit(3), digit(4)) {
                (5, Some(b':'), Some(h1), Some(h2), Some(m1), Some(m2)) => Some((h1 * 10 + h2, m1 * 10 + m2)),
                _ => None,
            };
            match time {
                Some((hour, minute)) if hour < 24 && minute < 60 => Ok(Some(Schedule::Daily { minute_of_day: hour * 60 + minute })),
                _ => Err(refuse("schedule.daily-invalid", format!("Action {name} has an invalid daily time. Use HH:MM in UTC, from 00:00 to 23:59."))),
            }
        }
        _ => Err(invalid()),
    }
}

/// Refuses `subscribe` on any action: the Kernel pushes nothing to a page.
pub fn check_subscribe(name: &str, action: &ActionDefinition) -> Result<(), Refusal> {
    if action.subscribe.is_some() {
        return Err(refuse("action.subscribe-unsupported", format!(
            "Action {name} declares subscribe, but the Kernel pushes no updates to a page. Poll a query that reads rows after a cursor instead."
        )));
    }
    Ok(())
}

/// Starts the timer. It runs for as long as the Kernel does.
pub fn spawn(state: AppState, clock: Clock) {
    if let Some(seconds) = clock.test_seconds {
        println!("Scheduled work runs every {seconds} seconds ({TEST_CLOCK}).");
    }
    let running: Arc<Mutex<HashSet<String>>> = Arc::default();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(clock.tick());
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            tick(&state, clock, &running).await;
        }
    });
}

/// Starts a run for each Deployment with scheduled work that is not in
/// maintenance and whose previous run has finished.
async fn tick(state: &AppState, clock: Clock, running: &Arc<Mutex<HashSet<String>>>) {
    let deployments = state.deployments.read().await.values().cloned().collect::<Vec<_>>();
    for deployment in deployments {
        let Ok(program) = serde_json::from_str::<ActionProgram>(&deployment.actions) else { continue };
        let mut work = program
            .actions
            .into_iter()
            .filter_map(|(name, action)| match declaration(&name, &action) {
                Ok(Some(schedule)) => Some((name, action, schedule)),
                _ => None,
            })
            .collect::<Vec<_>>();
        if work.is_empty() || super::maintenance::pause(&state.data_root, &deployment.id).is_some() {
            continue;
        }
        if !running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(deployment.id.clone()) {
            continue;
        }
        work.sort_by(|a, b| a.0.cmp(&b.0));
        let (state, running, id) = (state.clone(), running.clone(), deployment.id.clone());
        tokio::spawn(async move {
            if let Err(error) = run_due(&state, clock, &deployment, &work, &super::now_ms).await {
                eprintln!("Scheduled work for deployment {id} could not be read or recorded: {error:#}");
            }
            running.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&id);
        });
    }
}

/// Runs each of one Deployment's actions whose slot has not run yet. `now`
/// is the clock in milliseconds, which tests replace.
async fn run_due(
    state: &AppState,
    clock: Clock,
    deployment: &super::Deployment,
    work: &[(String, ActionDefinition, Schedule)],
    now: &(dyn Fn() -> u64 + Sync),
) -> anyhow::Result<()> {
    let id = &deployment.id;
    let folder = crate::layout::data(&state.data_root, id);
    if !folder.is_dir() {
        return Ok(());
    }
    let done = blocking(&folder, usage::schedule_slots).await?;
    for (name, action, schedule) in work {
        let (slot, key) = clock.place(schedule, now());
        match done.get(name) {
            Some((ran_under, ran_slot)) if *ran_under == key && *ran_slot >= slot => continue,
            Some((ran_under, _)) if *ran_under == key => {}
            // Never seen, or declared differently since: start from the next slot.
            _ => {
                let (name, key) = (name.clone(), key.clone());
                blocking(&folder, move |folder| usage::schedule_baseline(folder, &name, &key, slot)).await?;
                continue;
            }
        }
        let _lock = super::action_lock(state, id, &action.kind).await;
        // Maintenance may have begun while an earlier action ran.
        if super::maintenance::pause(&state.data_root, id).is_some() || !folder.is_dir() {
            return Ok(());
        }
        let at = now();
        let outcome = super::run_action(folder.join("database.sqlite"), action, parameters(action, at)).await;
        let (ran, error) = match outcome {
            Ok(ran) => (Some(ran), None),
            Err(error) => {
                eprintln!("Scheduled run of {name} on deployment {id} failed; the next tick tries again: {error}");
                (None, Some(error.to_string()))
            }
        };
        let changes = ran.as_ref().map(|ran| ran.answer["changes"].as_u64().unwrap_or(0));
        let (action_name, run_key) = (name.clone(), key.clone());
        blocking(&folder, move |folder| {
            usage::record_run(folder, &usage::Run { action: &action_name, schedule: &run_key, slot, at, changes, error })
        })
        .await?;
        if let Some(ran) = ran.filter(|_| changes.is_some_and(|changes| changes > 0)) {
            usage::signal(&state.data_root, id, name, action, &json!({}), None);
            // The notice shows the run's own time, the one kernel.now bound.
            super::notify::record(
                state, deployment, name, action, &json!({}),
                super::notify::Caller { account: None, actor_id: None },
                ran.changed(at),
            ).await;
        }
    }
    Ok(())
}

/// The values a scheduled run binds: `kernel.now`, the literals, and NULL
/// for `actor.id`. The parser refuses input
/// bindings on a scheduled action.
fn parameters(action: &ActionDefinition, now: u64) -> Vec<SqlValue> {
    action
        .bindings
        .iter()
        .map(|binding| match binding.source.as_str() {
            "kernel" => SqlValue::Integer(now as i64),
            "literal" => super::json_sql(binding.value.as_ref().unwrap_or(&Value::Null)).unwrap_or(SqlValue::Null),
            _ => SqlValue::Null,
        })
        .collect()
}

async fn blocking<T: Send + 'static>(
    folder: &Path,
    work: impl FnOnce(&Path) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let folder = folder.to_path_buf();
    tokio::task::spawn_blocking(move || work(&folder)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(actions: &Value) -> ActionProgram {
        serde_json::from_value(json!({"version": 1, "actions": actions})).unwrap()
    }

    /// The editor runs the same file in src/kernel/actions.test.ts and
    /// through the Validation Gate in src/agent/gate.test.ts.
    #[test]
    fn schedules_refuse_what_the_editor_refuses_with_the_same_message() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("schedule-cases.json")).unwrap();
        assert!(cases.len() >= 20);
        for case in cases {
            let program = program(&case["actions"]);
            let mut names = program.actions.keys().collect::<Vec<_>>();
            names.sort();
            let outcome = names.into_iter().try_for_each(|name| crate::validate_action(name, &program.actions[name]));
            crate::refusal::assert_case(&case, outcome);
        }
    }

    #[test]
    fn slots_follow_the_declaration_in_utc() {
        let at = utc;
        let quarter = Schedule::Every { minutes: 15 };
        assert_eq!(quarter.slot(at("2026-11-07T10:14:59")), quarter.slot(at("2026-11-07T10:00:00")));
        assert_eq!(quarter.slot(at("2026-11-07T10:15:00")), quarter.slot(at("2026-11-07T10:00:00")) + 1);
        let two = Schedule::Daily { minute_of_day: 120 };
        assert_eq!(two.key(), "daily 02:00");
        // 01:59 belongs to the day that opened at 02:00 yesterday; 02:00 opens a new one.
        assert_eq!(two.slot(at("2026-11-07T01:59:59")), two.slot(at("2026-11-06T02:00:00")));
        assert_eq!(two.slot(at("2026-11-07T02:00:00")), two.slot(at("2026-11-06T02:00:00")) + 1);
        assert_eq!(two.slot(at("2026-11-07T23:00:00")), two.slot(at("2026-11-07T02:00:00")));
        assert_eq!(Schedule::Every { minutes: 1440 }.slot(at("2026-11-07T23:59:59")), Schedule::Every { minutes: 1440 }.slot(at("2026-11-07T00:00:00")));
    }

    /// Seconds since 1970 for `YYYY-MM-DDTHH:MM:SS` in UTC, in milliseconds.
    fn utc(text: &str) -> u64 {
        let number = |range: std::ops::Range<usize>| text[range].parse::<i64>().unwrap();
        let (y, m, d) = (number(0..4), number(5..7), number(8..10));
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
        ((days * 86_400 + number(11..13) * 3600 + number(14..16) * 60 + number(17..19)) * 1000) as u64
    }

    #[test]
    fn the_test_clock_needs_a_loopback_bind_and_a_sane_value() {
        assert_eq!(Clock::parse("0.0.0.0", None).unwrap(), Clock { test_seconds: None });
        assert_eq!(Clock::parse("127.0.0.1", Some("2")).unwrap(), Clock { test_seconds: Some(2) });
        assert_eq!(Clock::parse("::1", Some("300")).unwrap(), Clock { test_seconds: Some(300) });
        assert_eq!(
            Clock::parse("0.0.0.0", Some("2")).unwrap_err().to_string(),
            "KONAE_SCHEDULE_MIN_SECONDS is for tests, and KONAE_BIND=0.0.0.0 is reachable from other machines. Remove it, or bind the Kernel to 127.0.0.1."
        );
        for bad in ["0", "301", "1.5", "-1", "two"] {
            assert_eq!(
                Clock::parse("127.0.0.1", Some(bad)).unwrap_err().to_string(),
                "KONAE_SCHEDULE_MIN_SECONDS must be a whole number of seconds from 1 to 300.",
                "{bad}"
            );
        }
        let clock = Clock { test_seconds: Some(2) };
        assert_eq!(clock.tick(), Duration::from_millis(500));
        let (slot, key) = clock.place(&Schedule::Daily { minute_of_day: 0 }, 10_000);
        assert_eq!((slot, key.as_str()), (5, "daily 00:00 (test clock 2s)"));
    }

    /// A Deployment with a `holds` table, its scheduled work, and a clock the test moves.
    struct Rig {
        root: std::path::PathBuf,
        state: AppState,
        id: String,
        now: std::sync::atomic::AtomicU64,
    }

    impl Rig {
        async fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!("konae-schedule-{label}-{}", uuid::Uuid::new_v4()));
            let (state, _) = crate::tests::login_test_state(&root, "public").await;
            let id = state.deployments.read().await.values().next().unwrap().id.clone();
            let db = rusqlite::Connection::open(crate::layout::database(&root, &id)).unwrap();
            db.execute_batch("CREATE TABLE holds (id INTEGER PRIMARY KEY, until INTEGER NOT NULL, released_at INTEGER);").unwrap();
            Self { root, state, id, now: Default::default() }
        }

        fn at(&self, text: &str) {
            self.now.store(utc(text), std::sync::atomic::Ordering::SeqCst);
        }

        async fn run(&self, actions: &Value, clock: Clock) {
            let program = program(actions);
            let mut work = program
                .actions
                .into_iter()
                .filter_map(|(name, action)| declaration(&name, &action).unwrap().map(|schedule| (name, action, schedule)))
                .collect::<Vec<_>>();
            work.sort_by(|a, b| a.0.cmp(&b.0));
            let now = || self.now.load(std::sync::atomic::Ordering::SeqCst);
            let deployment = self.state.deployments.read().await.values().find(|deployment| deployment.id == self.id).unwrap().clone();
            run_due(&self.state, clock, &deployment, &work, &now).await.unwrap();
        }

        fn sql(&self, statement: &str) {
            let db = rusqlite::Connection::open(crate::layout::database(&self.root, &self.id)).unwrap();
            db.execute_batch(statement).unwrap();
        }

        fn released(&self) -> i64 {
            let db = rusqlite::Connection::open(crate::layout::database(&self.root, &self.id)).unwrap();
            db.query_row("SELECT COUNT(*) FROM holds WHERE released_at IS NOT NULL", [], |row| row.get(0)).unwrap()
        }

        /// The runs `usage_read` returns for the whole of November 2026, oldest first.
        fn runs(&self) -> Vec<(String, String, i64)> {
            usage::flush();
            let path = crate::layout::usage(&self.root, &self.id);
            let usage = usage::read_usage(&path, "2026-11-01", "2026-11-30").unwrap();
            let mut runs = usage["scheduled"]
                .as_array()
                .unwrap()
                .iter()
                .map(|run| (run["action"].as_str().unwrap().to_owned(), run["outcome"].as_str().unwrap().to_owned(), run["changes"].as_i64().unwrap()))
                .collect::<Vec<_>>();
            runs.reverse();
            runs
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn release(schedule: Value) -> Value {
        json!({"holds.release": {"kind": "mutation", "access": "authenticated", "schedule": schedule,
            "guard": "Nothing to release.", "signal": {"name": "holds.released", "properties": ["actor"]},
            "statement": "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL AND until < ?1 AND ?2 IS NULL",
            "bindings": [{"source": "kernel", "field": "now"}, {"source": "actor", "field": "id"}]}})
    }

    #[tokio::test]
    async fn the_timer_runs_once_a_slot_resumes_from_the_store_and_retries_a_failure() {
        let rig = Rig::new("every").await;
        let real = Clock { test_seconds: None };
        let actions = release(json!({"every": "15m"}));
        rig.sql(&format!("INSERT INTO holds(until) VALUES ({}), ({}), ({})", utc("2026-11-07T10:00:00"), utc("2026-11-07T10:10:00"), utc("2026-11-07T12:00:00")));

        // Publishing at 10:07 runs nothing: the first slot counts as done.
        rig.at("2026-11-07T10:07:00");
        rig.run(&actions, real).await;
        assert_eq!((rig.released(), rig.runs().len()), (0, 0));

        // 10:15 opens a slot: one run releases the two lapsed holds.
        rig.at("2026-11-07T10:16:00");
        rig.run(&actions, real).await;
        assert_eq!(rig.released(), 2);
        assert_eq!(rig.runs(), vec![("holds.release".into(), "ok".into(), 2)]);

        // A second tick in the same slot, and a restart in it, run nothing.
        rig.at("2026-11-07T10:29:59");
        rig.run(&actions, real).await;
        rig.run(&actions, real).await;
        assert_eq!(rig.runs().len(), 1);

        // A run that changes no row is a run, not a guard refusal.
        rig.at("2026-11-07T10:30:00");
        rig.run(&actions, real).await;
        assert_eq!(rig.runs()[1], ("holds.release".into(), "ok".into(), 0));

        // A failure leaves the slot open: the next tick tries again, once.
        rig.sql("ALTER TABLE holds RENAME TO holds_away");
        rig.at("2026-11-07T10:45:00");
        rig.run(&actions, real).await;
        rig.at("2026-11-07T10:46:00");
        rig.run(&actions, real).await;
        rig.sql("ALTER TABLE holds_away RENAME TO holds");
        rig.at("2026-11-07T10:47:00");
        rig.run(&actions, real).await;
        rig.at("2026-11-07T10:48:00");
        rig.run(&actions, real).await;
        let outcomes = rig.runs().into_iter().skip(2).map(|run| run.1).collect::<Vec<_>>();
        assert_eq!(outcomes, ["failed", "failed", "ok"]);

        // A Kernel that was down from 10:48 to 13:05 runs once, not once per missed slot.
        rig.at("2026-11-07T13:05:00");
        rig.run(&actions, real).await;
        rig.run(&actions, real).await;
        assert_eq!(rig.runs().len(), 6);
        assert_eq!(rig.released(), 3);

        // Maintenance skips the run; leaving it runs the open slot once.
        crate::maintenance::pause_for_test(&rig.root, &rig.id, true);
        rig.at("2026-11-07T13:15:00");
        rig.run(&actions, real).await;
        assert_eq!(rig.runs().len(), 6);
        crate::maintenance::pause_for_test(&rig.root, &rig.id, false);
        rig.run(&actions, real).await;
        assert_eq!(rig.runs().len(), 7);

        // Usage counts every run under scheduled, and the runs that changed
        // a row record their Signal with no actor.
        let path = crate::layout::usage(&rig.root, &rig.id);
        let usage = usage::read_usage(&path, "2026-11-07", "2026-11-07").unwrap();
        assert!(usage["totals"].as_array().unwrap().contains(&json!({"kind": "scheduled", "name": "holds.release", "count": 7})), "{usage}");
        assert!(!usage["totals"].as_array().unwrap().iter().any(|total| total["kind"] == "refused"));
        let signals = usage::read_signals(&path, Some("holds.released"), None).unwrap();
        assert_eq!(signals["signals"].as_array().unwrap().len(), 2);
        assert_eq!(signals["signals"][0]["properties"], json!({"actor": null}));
        let failed = &usage["scheduled"].as_array().unwrap().iter().find(|run| run["outcome"] == "failed").unwrap()["error"];
        assert!(failed.as_str().unwrap().contains("no such table: holds"), "{failed}");
    }

    #[tokio::test]
    async fn a_daily_job_runs_at_its_time_once_and_a_changed_schedule_starts_afresh() {
        let rig = Rig::new("daily").await;
        let real = Clock { test_seconds: None };
        let daily = release(json!({"daily": "02:00"}));
        rig.at("2026-11-07T10:00:00");
        rig.run(&daily, real).await;
        rig.at("2026-11-08T01:59:59");
        rig.run(&daily, real).await;
        assert!(rig.runs().is_empty(), "nothing before 02:00");
        rig.at("2026-11-08T02:00:30");
        rig.run(&daily, real).await;
        // A restart at 03:00 the same day finds the run in the store.
        rig.at("2026-11-08T03:00:00");
        rig.run(&daily, real).await;
        assert_eq!(rig.runs().len(), 1);
        // A republish that moves it to 04:00 starts from the slot it is in, so it does not run at once.
        let moved = release(json!({"daily": "04:00"}));
        rig.at("2026-11-08T05:00:00");
        rig.run(&moved, real).await;
        assert_eq!(rig.runs().len(), 1);
        rig.at("2026-11-09T04:00:00");
        rig.run(&moved, real).await;
        assert_eq!(rig.runs().len(), 2);
        // The test clock keys its own slots, so it starts afresh too.
        rig.run(&moved, Clock { test_seconds: Some(2) }).await;
        assert_eq!(rig.runs().len(), 2);
        rig.at("2026-11-09T04:00:02");
        rig.run(&moved, Clock { test_seconds: Some(2) }).await;
        assert_eq!(rig.runs().len(), 3);
    }

    #[test]
    fn a_run_binds_now_its_literals_and_no_actor() {
        let program = program(&json!({
            "holds.release": {"kind": "mutation", "access": "authenticated", "schedule": {"every": "5m"},
                "statement": "UPDATE holds SET released_at = ?1 WHERE until < ?1 AND (?2 IS NULL) AND kind = ?3",
                "bindings": [{"source": "kernel", "field": "now"}, {"source": "actor", "field": "id"}, {"source": "literal", "value": "room"}]}
        }));
        assert_eq!(
            parameters(&program.actions["holds.release"], 42),
            vec![SqlValue::Integer(42), SqlValue::Null, SqlValue::Text("room".into())]
        );
    }
}
