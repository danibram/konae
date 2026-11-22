use std::{collections::BTreeMap, time::Duration};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

pub const MAX_SAMPLES: i64 = 10_000;
const MAX_SERIES: usize = 128;

#[derive(Clone, Copy)]
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
            && self.error_code.as_ref().is_none_or(|value| !value.is_empty() && value.len() <= 64)
            && self.error_detail.as_ref().is_none_or(|value| value.len() <= 4096)
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
    let names = columns.query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()?;
    drop(columns);
    if !names.contains("error_code") { db.execute("ALTER TABLE function_latency ADD COLUMN error_code TEXT", [])?; }
    if !names.contains("error_detail") { db.execute("ALTER TABLE function_latency ADD COLUMN error_detail TEXT", [])?; }
    Ok(())
}

pub(super) fn write(db: &Connection, at: u64, sample: &FunctionTiming) -> Result<()> {
    db.execute(
        "INSERT INTO function_latency(at,name,version,budget_us,total_us,queue_us,
          initialization_us,execution_us,cold,timed_out,outcome,error_code,error_detail)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![
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
            sample.error_code.as_deref().map(|value| super::truncate(value, 64)),
            sample.error_detail.as_deref().map(|value| super::truncate(value, 4096))
        ],
    )?;
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
    errors: Vec<Value>,
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
        query.query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()?
    };
    let error_fields = if columns.contains("error_code") && columns.contains("error_detail") {
        "substr(error_code,1,64),substr(error_detail,1,4096)"
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
            if group.errors.len() < 5 {
                group.errors.push(json!({"code": code, "detail": row.get::<_, Option<String>>(12)?}));
            }
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
            "errors": group.errors})
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
    use crate::usage::Batch;

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
        assert_eq!(groups[2]["errors"][0]["code"], "deadline_exceeded");
        assert_eq!(groups[2]["errors"][0]["detail"], "request exceeded its deadline");
    }

    #[test]
    fn retention_is_bounded_and_declared_to_the_reader() {
        let fixture = Fixture::new();
        let now = 1_800_000_000_000;
        let batch = Batch {
            function_timings: (0..MAX_SAMPLES + 5)
                .map(|n| (now + n as u64, timing(10)))
                .collect(),
            ..Batch::default()
        };
        crate::usage::write(&fixture.0, &batch, now + 20_000).unwrap();
        let summary = read(&fixture.path(), now as i64, now as i64 + 20_000).unwrap();
        assert_eq!(summary["series"][0]["samples"], MAX_SAMPLES);
        assert_eq!(summary["prunedThrough"], now + 4);
        assert_eq!(summary["retainedFrom"], now + 5);
        assert_eq!(summary["retainedOnly"], true);
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
