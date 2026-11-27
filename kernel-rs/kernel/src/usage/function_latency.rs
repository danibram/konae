use std::{collections::BTreeMap, time::Duration};

use super::FunctionActionTiming;
use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

pub const MAX_SAMPLES: i64 = 10_000;
const MAX_SERIES: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FunctionOutcome {
    Ok,
    Refused,
    Failed,
    DeadlineExceeded,
    MutationOutcomeUnknown,
    Replayed,
}

impl FunctionOutcome {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::MutationOutcomeUnknown => "mutation_outcome_unknown",
            Self::Replayed => "replayed",
        }
    }
}

pub struct FunctionTiming {
    pub name: String,
    pub version: String,
    pub budget: Duration,
    pub total: Duration,
    pub queue: Duration,
    pub initialization: Duration,
    pub execution: Duration,
    pub cold: bool,
    pub timed_out: bool,
    pub outcome: FunctionOutcome,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

impl FunctionTiming {
    pub(super) fn valid(&self) -> bool {
        !self.name.is_empty()
            && self.name.len() <= 64
            && !self.version.is_empty()
            && self.version.len() <= 128
            && self
                .error_code
                .as_ref()
                .is_none_or(|value| !value.is_empty() && value.len() <= 64)
            && self
                .error_detail
                .as_ref()
                .is_none_or(|value| value.len() <= 4096)
            && self.budget >= Duration::from_millis(1)
            && [
                self.budget,
                self.total,
                self.queue,
                self.initialization,
                self.execution,
            ]
            .iter()
            .all(|duration| *duration <= Duration::from_secs(86_400))
    }
}

pub(super) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS function_latency (
            id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL,
            name TEXT NOT NULL, version TEXT NOT NULL, budget_us INTEGER NOT NULL,
            total_us INTEGER NOT NULL, queue_us INTEGER NOT NULL,
            initialization_us INTEGER NOT NULL, execution_us INTEGER NOT NULL,
            cold INTEGER NOT NULL, timed_out INTEGER NOT NULL, outcome TEXT NOT NULL,
            error_code TEXT, error_detail TEXT
        );
        CREATE INDEX IF NOT EXISTS function_latency_by_time ON function_latency(at);",
    )?;
    let mut columns = db.prepare("PRAGMA table_info(function_latency)")?;
    let names = columns
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()?;
    drop(columns);
    if !names.contains("error_code") {
        db.execute(
            "ALTER TABLE function_latency ADD COLUMN error_code TEXT",
            [],
        )?;
    }
    if !names.contains("error_detail") {
        db.execute(
            "ALTER TABLE function_latency ADD COLUMN error_detail TEXT",
            [],
        )?;
    }
    db.execute(
        "UPDATE function_latency SET error_detail=NULL WHERE error_detail IS NOT NULL",
        [],
    )?;
    Ok(())
}

pub(super) fn clear_legacy_details(db: &Connection) -> Result<()> {
    if !exists(db)? {
        return Ok(());
    }
    let has_detail: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('function_latency') WHERE name='error_detail')", [], |row| row.get(0))?;
    if has_detail {
        db.execute(
            "UPDATE function_latency SET error_detail=NULL WHERE error_detail IS NOT NULL",
            [],
        )?;
    }
    Ok(())
}

pub(super) fn initialize_actions(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS function_action_latency (
        id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, action TEXT NOT NULL,
        version TEXT NOT NULL, origin TEXT NOT NULL, duration_us INTEGER NOT NULL,
        sqlite_us INTEGER NOT NULL, rows_read INTEGER, rows_written INTEGER,
        reply_bytes INTEGER, error_code TEXT
    ); CREATE INDEX IF NOT EXISTS function_action_latency_by_time ON function_action_latency(at);",
    )?;
    Ok(())
}

pub(super) fn write_action(db: &Connection, at: u64, sample: &FunctionActionTiming) -> Result<()> {
    db.execute("INSERT INTO function_action_latency(at,action,version,origin,duration_us,sqlite_us,rows_read,rows_written,reply_bytes,error_code)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![at as i64, super::truncate(&sample.action, 64), super::truncate(&sample.version, 128), sample.origin,
        sample.duration.as_micros() as i64, sample.sqlite.as_micros() as i64, sample.rows_read.map(i64::from), sample.rows_written.map(i64::from), sample.reply_bytes.map(i64::from),
        sample.error_code.as_deref().map(|code| super::truncate(code, 64))])?;
    Ok(())
}

pub(super) fn prune_actions(db: &Connection, now: u64) -> Result<()> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='function_action_latency')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let cutoff = now.saturating_sub(super::USAGE_DAYS as u64 * 86_400_000) as i64;
    let removed: Option<i64> = db.query_row(
        "SELECT MAX(at) FROM function_action_latency WHERE at < ?1 OR id IN
        (SELECT id FROM function_action_latency ORDER BY id DESC LIMIT -1 OFFSET ?2)",
        params![cutoff, MAX_SAMPLES],
        |row| row.get(0),
    )?;
    if let Some(at) = removed {
        db.execute(
            "INSERT INTO metric_meta(key,value) VALUES('function_action_latency_pruned_through',?1)
            ON CONFLICT(key) DO UPDATE SET value=max(value,excluded.value)",
            [at],
        )?;
    }
    db.execute(
        "DELETE FROM function_action_latency WHERE at < ?1 OR id IN
        (SELECT id FROM function_action_latency ORDER BY id DESC LIMIT -1 OFFSET ?2)",
        params![cutoff, MAX_SAMPLES],
    )?;
    Ok(())
}

pub(super) fn read_actions(path: &std::path::Path, from: i64, to: i64) -> Result<Value> {
    let empty = json!({"series": [], "maxSamples": MAX_SAMPLES, "seriesLimit": MAX_SERIES, "seriesTruncated": false, "retainedOnly": true});
    let Some(db) = super::read_only(path)? else {
        return Ok(empty);
    };
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='function_action_latency')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(empty);
    }
    let pruned: Option<i64> = db
        .query_row(
            "SELECT value FROM metric_meta WHERE key='function_action_latency_pruned_through'",
            [],
            |row| row.get(0),
        )
        .ok();
    let retained_from: Option<i64> =
        db.query_row("SELECT MIN(at) FROM function_action_latency", [], |row| {
            row.get(0)
        })?;
    let mut query = db.prepare("SELECT action,version,origin,duration_us,sqlite_us,rows_read,rows_written,reply_bytes,error_code,at FROM function_action_latency WHERE at >= ?1 AND at < ?2 ORDER BY id DESC LIMIT ?3")?;
    let mut rows = query.query(params![from, to, MAX_SAMPLES])?;
    let mut groups: BTreeMap<
        (String, String, String),
        (
            Vec<i64>,
            Vec<i64>,
            u64,
            u64,
            u64,
            BTreeMap<String, u64>,
            i64,
        ),
    > = BTreeMap::new();
    let mut sample_count = 0u64;
    while let Some(row) = rows.next()? {
        sample_count += 1;
        let key = (row.get(0)?, row.get(1)?, row.get(2)?);
        let group = groups.entry(key).or_default();
        group.0.push(row.get(3)?);
        group.1.push(row.get(4)?);
        group.2 += row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as u64;
        group.3 += row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64;
        group.4 += row.get::<_, Option<i64>>(7)?.unwrap_or(0).max(0) as u64;
        if let Some(code) = row.get::<_, Option<String>>(8)? {
            *group.5.entry(code).or_default() += 1;
        }
        group.6 = group.6.max(row.get(9)?);
    }
    let mut series: Vec<Value> = groups.into_iter().map(|((action,version,origin),(mut duration,mut sqlite,rows_read,rows_written,reply_bytes,errors,last_at))|
        json!({"action":action,"version":version,"origin":origin,"samples":duration.len(),"duration":percentiles(&mut duration),"sqlite":percentiles(&mut sqlite),"rowsRead":rows_read,"rowsWritten":rows_written,"replyBytes":reply_bytes,"errors":errors,"lastAt":last_at})).collect();
    series.sort_by_key(|value| std::cmp::Reverse(value["lastAt"].as_i64().unwrap_or_default()));
    let truncated = series.len() > MAX_SERIES;
    series.truncate(MAX_SERIES);
    Ok(
        json!({"series":series,"maxSamples":MAX_SAMPLES,"seriesLimit":MAX_SERIES,"seriesTruncated":truncated,"retainedOnly":true,"samples":sample_count,"queueDrops":super::dropped(),"prunedThrough":pruned,"retainedFrom":retained_from}),
    )
}

pub(super) fn write(db: &Connection, at: u64, sample: &FunctionTiming) -> Result<()> {
    db.prepare_cached(
        "INSERT INTO function_latency(at,name,version,budget_us,total_us,queue_us,
          initialization_us,execution_us,cold,timed_out,outcome,error_code,error_detail)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
    )?
    .execute(params![
            at as i64,
            sample.name,
            sample.version,
            sample.budget.as_micros() as i64,
            sample.total.as_micros() as i64,
            sample.queue.as_micros() as i64,
            sample.initialization.as_micros() as i64,
            sample.execution.as_micros() as i64,
            sample.cold,
            sample.timed_out,
            sample.outcome.name(),
            sample
                .error_code
                .as_deref()
                .map(|value| super::truncate(value, 64)),
            None::<&str>
    ])?;
    Ok(())
}

fn exists(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='function_latency')",
        [],
        |row| row.get(0),
    )?)
}

pub(super) fn prune(db: &Connection, now: u64) -> Result<()> {
    if !exists(db)? {
        return Ok(());
    }
    let cutoff = now.saturating_sub(super::USAGE_DAYS as u64 * 86_400_000) as i64;
    let removed: Option<i64> = db.query_row(
        "SELECT MAX(at) FROM function_latency WHERE at < ?1
         OR id IN (SELECT id FROM function_latency ORDER BY id DESC LIMIT -1 OFFSET ?2)",
        params![cutoff, MAX_SAMPLES],
        |row| row.get(0),
    )?;
    if let Some(at) = removed {
        db.execute(
            "INSERT INTO metric_meta(key,value) VALUES('function_latency_pruned_through',?1)
            ON CONFLICT(key) DO UPDATE SET value=max(value,excluded.value)",
            [at],
        )?;
        db.execute(
            "DELETE FROM function_latency WHERE at < ?1
            OR id IN (SELECT id FROM function_latency ORDER BY id DESC LIMIT -1 OFFSET ?2)",
            params![cutoff, MAX_SAMPLES],
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct Group {
    total: Vec<i64>,
    queue: Vec<i64>,
    initialization: Vec<i64>,
    execution: Vec<i64>,
    cold: u64,
    timeouts: u64,
    failures: u64,
    unknown: u64,
    replays: u64,
    last_at: i64,
    errors: BTreeMap<String, u64>,
}

fn percentiles(values: &mut [i64]) -> Value {
    values.sort_unstable();
    let at = |percent: usize| values[(values.len() * percent).div_ceil(100) - 1] as f64 / 1000.0;
    json!({"p50Ms": at(50), "p95Ms": at(95), "p99Ms": at(99),
        "maxMs": values.last().copied().unwrap_or(0) as f64 / 1000.0})
}

pub(super) fn read(path: &std::path::Path, from: i64, to: i64) -> Result<Value> {
    let empty = json!({"series": [], "maxSamples": MAX_SAMPLES, "seriesLimit": MAX_SERIES,
        "seriesTruncated": false, "retainedOnly": true,
        "prunedThrough": null, "retainedFrom": null});
    let Some(db) = super::read_only(path)? else {
        return Ok(empty);
    };
    if !exists(&db)? {
        return Ok(empty);
    }
    let pruned: Option<i64> = db
        .query_row(
            "SELECT value FROM metric_meta WHERE key='function_latency_pruned_through'",
            [],
            |row| row.get(0),
        )
        .ok();
    let retained_from: Option<i64> =
        db.query_row("SELECT MIN(at) FROM function_latency", [], |row| row.get(0))?;
    let columns = {
        let mut query = db.prepare("PRAGMA table_info(function_latency)")?;
        query
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()?
    };
    let error_fields = if columns.contains("error_code") {
        "substr(error_code,1,64),NULL"
    } else {
        "NULL,NULL"
    };
    let mut groups: BTreeMap<(String, String, i64), Group> = BTreeMap::new();
    let mut query = db.prepare(&format!(
        "SELECT substr(name,1,64),substr(version,1,128),budget_us,total_us,
        queue_us,initialization_us,execution_us,cold,timed_out,substr(outcome,1,32),at,
        {error_fields} FROM function_latency WHERE at >= ?1 AND at < ?2 ORDER BY id DESC LIMIT ?3"
    ))?;
    let mut rows = query.query(params![from, to, MAX_SAMPLES])?;
    while let Some(row) = rows.next()? {
        let budget: i64 = row.get(2)?;
        anyhow::ensure!(budget > 0, "Invalid function timing budget.");
        let group = groups
            .entry((row.get(0)?, row.get(1)?, budget))
            .or_default();
        group.total.push(row.get(3)?);
        group.queue.push(row.get(4)?);
        group.initialization.push(row.get(5)?);
        group.execution.push(row.get(6)?);
        group.cold += u64::from(row.get::<_, bool>(7)?);
        group.timeouts += u64::from(row.get::<_, bool>(8)?);
        let outcome: String = row.get(9)?;
        group.failures += u64::from(!matches!(outcome.as_str(), "ok" | "replayed"));
        group.unknown += u64::from(outcome == "mutation_outcome_unknown");
        group.replays += u64::from(outcome == "replayed");
        group.last_at = group.last_at.max(row.get(10)?);
        if let Some(code) = row.get::<_, Option<String>>(11)? {
            *group.errors.entry(code).or_default() += 1;
        }
    }
    let mut series: Vec<Value> = groups
        .into_iter()
        .map(|((name, version, budget), mut group)| {
            let total = percentiles(&mut group.total);
            json!({"name": name, "version": version, "limitMs": budget as f64 / 1000.0,
            "samples": group.total.len(), "coldStarts": group.cold, "timeouts": group.timeouts,
            "failures": group.failures, "uncertain": group.unknown, "replays": group.replays,
            "budgetP95Percent": total["p95Ms"].as_f64().unwrap() * 100_000.0 / budget as f64,
            "total": total, "queue": percentiles(&mut group.queue),
            "initialization": percentiles(&mut group.initialization),
            "execution": percentiles(&mut group.execution), "lastAt": group.last_at,
            "errorCounts": group.errors})
        })
        .collect();
    series.sort_by_key(|row| std::cmp::Reverse(row["lastAt"].as_i64().unwrap_or(0)));
    let truncated = series.len() > MAX_SERIES;
    series.truncate(MAX_SERIES);
    Ok(
        json!({"series": series, "maxSamples": MAX_SAMPLES, "seriesLimit": MAX_SERIES,
        "seriesTruncated": truncated, "retainedOnly": true,
        "prunedThrough": pruned, "retainedFrom": retained_from}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{Batch, DAY_MS, Message, day, read_usage, writer};
    use std::sync::mpsc::sync_channel;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let folder = std::env::temp_dir()
                .join(format!("konae-function-latency-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&folder).unwrap();
            Self(folder)
        }
        fn path(&self) -> std::path::PathBuf {
            self.0.join(crate::usage::USAGE_DATABASE)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn timing(milliseconds: u64) -> FunctionTiming {
        FunctionTiming {
            name: "orders".into(),
            version: "version-a".into(),
            budget: Duration::from_millis(100),
            total: Duration::from_millis(milliseconds),
            queue: Duration::from_millis(2),
            initialization: Duration::ZERO,
            execution: Duration::from_millis(milliseconds.saturating_sub(2)),
            cold: false,
            timed_out: false,
            outcome: FunctionOutcome::Ok,
            error_code: None,
            error_detail: None,
        }
    }

    #[test]
    fn exact_percentiles_and_budget_ratio_come_from_retained_samples() {
        let fixture = Fixture::new();
        let now = 1_800_000_000_000;
        let batch = Batch {
            function_timings: (1..=100).map(|n| (now + n, timing(n))).collect(),
            ..Batch::default()
        };
        crate::usage::write(&fixture.0, &batch, now + 100).unwrap();
        let summary = read(&fixture.path(), now as i64, now as i64 + 101).unwrap();
        let group = &summary["series"][0];
        assert_eq!(group["samples"], 100);
        assert_eq!(
            group["total"],
            json!({"p50Ms":50.0,"p95Ms":95.0,"p99Ms":99.0,"maxMs":100.0})
        );
        assert_eq!(group["budgetP95Percent"], 95.0);
        assert_eq!(group["queue"]["p95Ms"], 2.0);
        assert_eq!(group["initialization"]["maxMs"], 0.0);
    }

    #[test]
    fn versions_and_limits_do_not_mix_and_timeouts_are_kept() {
        let fixture = Fixture::new();
        let now = 1_800_000_000_000;
        let mut cold = timing(110);
        cold.cold = true;
        cold.initialization = Duration::from_millis(90);
        cold.timed_out = true;
        cold.outcome = FunctionOutcome::DeadlineExceeded;
        cold.error_code = Some("deadline_exceeded".into());
        cold.error_detail = Some("request exceeded its deadline".into());
        let mut next = timing(20);
        next.version = "version-b".into();
        let mut new_limit = timing(30);
        new_limit.budget = Duration::from_millis(50);
        new_limit.outcome = FunctionOutcome::MutationOutcomeUnknown;
        let batch = Batch {
            function_timings: vec![(now, cold), (now + 1, next), (now + 2, new_limit)],
            ..Batch::default()
        };
        crate::usage::write(&fixture.0, &batch, now + 2).unwrap();
        let summary = read(&fixture.path(), now as i64, now as i64 + 3).unwrap();
        let groups = summary["series"].as_array().unwrap();
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0]["uncertain"], 1);
        assert_eq!(groups[0]["budgetP95Percent"], 60.0);
        assert_eq!(groups[1]["version"], "version-b");
        assert_eq!(groups[2]["timeouts"], 1);
        assert_eq!(groups[2]["coldStarts"], 1);
        assert_eq!(groups[2]["budgetP95Percent"], 110.0);
        assert_eq!(groups[2]["errorCounts"]["deadline_exceeded"], 1);
        assert!(groups[2].get("errors").is_none());
        let stored_detail: Option<String> = Connection::open(fixture.path())
            .unwrap()
            .query_row(
                "SELECT error_detail FROM function_latency ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_detail, None);
        Connection::open(fixture.path())
            .unwrap()
            .execute(
                "UPDATE function_latency SET error_detail='private fixture input'",
                [],
            )
            .unwrap();
        crate::usage::write(&fixture.0, &Batch::default(), now + 3).unwrap();
        let db = Connection::open(fixture.path()).unwrap();
        let cleared: Option<String> = db
            .query_row(
                "SELECT error_detail FROM function_latency ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cleared, None);
        drop(db);
        let safe = read(&fixture.path(), now as i64, now as i64 + 3)
            .unwrap()
            .to_string();
        assert!(!safe.contains("private fixture input"));
    }

    #[test]
    fn action_latency_groups_function_and_sqlite_samples() {
        let fixture = Fixture::new();
        let now = 1_800_000_000_000;
        let first = FunctionActionTiming {
            action: "inventory.read".into(),
            version: "v1".into(),
            origin: "function".into(),
            duration: Duration::from_micros(120),
            sqlite: Duration::from_micros(30),
            rows_read: Some(2),
            rows_written: None,
            reply_bytes: Some(64),
            error_code: None,
        };
        let mut second = first.clone();
        second.duration = Duration::from_micros(240);
        second.sqlite = Duration::from_micros(60);
        second.rows_read = Some(3);
        second.error_code = Some("function.action-failed".into());
        let mut batch = Batch::default();
        batch.function_action_timings = vec![(now, first), (now + 1, second)];
        crate::usage::write(&fixture.0, &batch, now + 1).unwrap();
        let summary = read_actions(&fixture.path(), now as i64, now as i64 + 2).unwrap();
        assert_eq!(summary["series"][0]["samples"], 2);
        assert_eq!(summary["series"][0]["duration"]["p50Ms"], 0.12);
        assert_eq!(summary["series"][0]["sqlite"]["p95Ms"], 0.06);
        assert_eq!(summary["series"][0]["rowsRead"], 5);
        assert_eq!(summary["series"][0]["errors"]["function.action-failed"], 1);
    }

    #[test]
    fn action_latency_retention_is_capped_at_ten_thousand_samples() {
        let fixture = Fixture::new();
        let now = 1_800_000_000_000;
        let timing = FunctionActionTiming {
            action: "inventory.read".into(),
            version: "v1".into(),
            origin: "function".into(),
            duration: Duration::from_micros(50),
            sqlite: Duration::from_micros(20),
            rows_read: Some(1),
            rows_written: None,
            reply_bytes: Some(32),
            error_code: None,
        };
        let mut batch = Batch::default();
        batch.function_action_timings = (0..MAX_SAMPLES + 5)
            .map(|n| (now + n as u64, timing.clone()))
            .collect();
        crate::usage::write(&fixture.0, &batch, now + MAX_SAMPLES as u64 + 5).unwrap();
        let summary =
            read_actions(&fixture.path(), now as i64, now as i64 + MAX_SAMPLES + 6).unwrap();
        assert_eq!(summary["series"][0]["samples"], MAX_SAMPLES);
        assert_eq!(summary["samples"], MAX_SAMPLES);
        assert_eq!(summary["retainedFrom"], now as i64 + 5);
        assert!(summary["queueDrops"].as_u64().is_some());
    }

    #[test]
    fn retention_is_bounded_and_declared_to_the_reader() {
        let fixture = Fixture::new();
        let midnight = crate::now_ms() / DAY_MS * DAY_MS;
        let now = midnight - 5;
        let (send, receive) = sync_channel((MAX_SAMPLES + 5) as usize);
        for n in 0..MAX_SAMPLES + 5 {
            send.send(Message::FunctionLatency {
                folder: fixture.0.clone(),
                at: now + n as u64,
                timing: timing(10),
            })
            .unwrap();
        }
        drop(send);
        writer(receive);
        let summary = read(&fixture.path(), now as i64, now as i64 + 20_000).unwrap();
        assert_eq!(summary["series"][0]["samples"], MAX_SAMPLES);
        assert_eq!(summary["prunedThrough"], now + 4);
        assert_eq!(summary["retainedFrom"], now + 5);
        assert_eq!(summary["retainedOnly"], true);
        let yesterday = day(now);
        let today = day(midnight);
        let usage = read_usage(&fixture.path(), &yesterday, &today).unwrap();
        assert_eq!(
            usage["days"],
            json!([
                {"day": yesterday, "kind": "function", "name": "orders", "count": 5},
                {"day": today, "kind": "function", "name": "orders", "count": MAX_SAMPLES},
            ])
        );
        assert_eq!(
            usage["totals"],
            json!([
                {"kind": "function", "name": "orders", "count": MAX_SAMPLES + 5},
            ])
        );
        let db = Connection::open(fixture.path()).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM function_latency", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            MAX_SAMPLES
        );
    }

    #[test]
    fn ordinary_usage_does_not_create_function_history() {
        let fixture = Fixture::new();
        crate::usage::write(&fixture.0, &Batch::default(), 1_800_000_000_000).unwrap();
        let summary = read(&fixture.path(), 0, i64::MAX).unwrap();
        assert_eq!(summary["series"], json!([]));
        assert!(!exists(&Connection::open(fixture.path()).unwrap()).unwrap());
    }

    #[test]
    fn oversized_or_zero_budget_samples_are_not_admitted() {
        let mut sample = timing(10);
        assert!(sample.valid());
        sample.budget = Duration::ZERO;
        assert!(!sample.valid());
        sample.budget = Duration::from_secs(1);
        sample.name = "x".repeat(65);
        assert!(!sample.valid());
    }
}
