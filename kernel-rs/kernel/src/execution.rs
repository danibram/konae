use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Durable module work is signalled by app ID and claimed from that app's database.
/// The dispatcher keeps only ready owners in memory, never pending jobs.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum WorkResource {
    Calls,
    Notices,
    FileIntake,
    Documents,
}

#[derive(Default)]
struct ReadyQueue {
    apps: VecDeque<String>,
    queued: HashSet<String>,
    active: HashMap<String, usize>,
    active_total: usize,
}

/// Fair, coalescing wake-up queues and capacity for each registered work resource.
pub struct Dispatcher {
    queues: HashMap<WorkResource, Arc<WorkQueue>>,
}

#[derive(Default)]
struct WorkQueue {
    ready: Mutex<ReadyQueue>,
    changed: tokio::sync::Notify,
}

#[derive(Clone, Copy)]
pub(crate) struct WorkLimits {
    total: NonZeroUsize,
    per_app: NonZeroUsize,
}

impl WorkLimits {
    pub(crate) const SERIAL: Self = Self::new(NonZeroUsize::MIN, NonZeroUsize::MIN);

    pub(crate) const fn new(total: NonZeroUsize, per_app: NonZeroUsize) -> Self {
        Self { total, per_app }
    }
}

/// A module holds its turn until it finishes processing or gives up the job.
#[must_use]
pub(crate) struct WorkTurn {
    work: Arc<WorkQueue>,
    app: String,
}

impl WorkTurn {
    pub(crate) fn app(&self) -> &str {
        &self.app
    }
}

impl Drop for WorkTurn {
    fn drop(&mut self) {
        let mut queue = self.work.ready.lock().unwrap_or_else(|error| error.into_inner());
        let active = queue.active.get_mut(&self.app).expect("the turn owns an active slot");
        *active -= 1;
        if *active == 0 {
            queue.active.remove(&self.app);
        }
        queue.active_total -= 1;
        drop(queue);
        self.work.changed.notify_waiters();
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        let queues = crate::modules::work_resources()
            .map(|resource| (resource, Arc::default()))
            .collect();
        Self { queues }
    }
}

impl Dispatcher {

    /// Adds one ready app. Repeated signals coalesce by app ID.
    /// Callers use IDs from a Deployment loaded or validated by the Kernel.
    pub(crate) async fn wake(&self, resource: WorkResource, app: impl Into<String>) {
        let work = &self.queues[&resource];
        let app = app.into();
        let mut queue = work.ready.lock().unwrap_or_else(|error| error.into_inner());
        if !queue.queued.insert(app.clone()) {
            return;
        }
        queue.apps.push_back(app);
        drop(queue);
        work.changed.notify_waiters();
    }

    /// Takes the first ready app with capacity. Every consumer of a resource uses the same limits.
    pub(crate) async fn next_turn(&self, resource: WorkResource, limits: WorkLimits) -> WorkTurn {
        let work = &self.queues[&resource];
        loop {
            let notified = work.changed.notified();
            {
                let mut queue = work.ready.lock().unwrap_or_else(|error| error.into_inner());
                let eligible = (queue.active_total < limits.total.get())
                    .then(|| {
                        queue.apps.iter().position(|app| {
                            queue.active.get(app).copied().unwrap_or_default() < limits.per_app.get()
                        })
                    })
                    .flatten();
                if let Some(app) = eligible.and_then(|index| queue.apps.remove(index)) {
                    queue.queued.remove(&app);
                    *queue.active.entry(app.clone()).or_default() += 1;
                    queue.active_total += 1;
                    return WorkTurn { work: work.clone(), app };
                }
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn next(&self, resource: WorkResource) -> String {
        self.next_turn(resource, WorkLimits::SERIAL).await.app.clone()
    }
}

/// Keeps one timer per app and moves it earlier when a new deadline arrives.
#[derive(Default)]
pub struct AppTimers {
    deadlines: BTreeMap<String, Instant>,
    retries: std::collections::HashMap<String, u32>,
}

impl AppTimers {
    pub fn schedule(&mut self, app: String, deadline: Instant) {
        self.deadlines
            .entry(app)
            .and_modify(|old| *old = (*old).min(deadline))
            .or_insert(deadline);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.values().copied().min()
    }

    pub fn cancel(&mut self, app: &str) {
        self.deadlines.remove(app);
    }

    pub fn schedule_retry(&mut self, app: String, now: Instant) {
        let attempt = self.retries.entry(app.clone()).or_default();
        let seconds = 5u64.saturating_mul(1u64 << (*attempt).min(6)).min(300);
        *attempt = attempt.saturating_add(1);
        self.schedule(app, now + std::time::Duration::from_secs(seconds));
    }

    pub fn reset_retries(&mut self, app: &str) {
        self.retries.remove(app);
    }

    pub fn remove(&mut self, app: &str) {
        self.cancel(app);
        self.reset_retries(app);
    }

    pub fn take_due(&mut self, now: Instant) -> Vec<String> {
        let due: Vec<String> = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= now)
            .map(|(app, _)| app.clone())
            .collect();
        for app in &due {
            self.deadlines.remove(app);
        }
        due
    }
}

/// Identity and bounded authority carried by one Kernel operation.
/// The release is pinned by the caller before this value is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub app: String,
    pub release: String,
    pub actor: Actor,
    pub limits: Limits,
    operations: BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_operations: usize,
    pub max_input_bytes: usize,
    pub deadline: Option<std::time::Instant>,
}

/// A Kernel-owned effect that must be prepared on the operation's SQLite
/// connection before commit. Implementations own their table and validation.
pub trait TransactionEffect {
    fn prepare(&self, connection: &rusqlite::Connection) -> rusqlite::Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    Anonymous,
    Human(String),
    Job { id: String, authority: JobAuthority },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobAuthority {
    ScheduledAction,
    ScheduledFunction,
    CallContinuation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationError {
    OperationNotAllowed,
    AuthenticationRequired,
    ActorNotAllowed,
    InvalidAccess,
}

impl std::fmt::Display for AuthorizationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OperationNotAllowed => "operation-not-allowed",
            Self::AuthenticationRequired => "authentication-required",
            Self::ActorNotAllowed => "actor-not-allowed",
            Self::InvalidAccess => "invalid-access",
        })
    }
}

impl std::error::Error for AuthorizationError {}

impl Context {
    pub fn new(
        app: impl Into<String>,
        release: impl Into<String>,
        actor: Actor,
        operations: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            app: app.into(),
            release: release.into(),
            actor,
            limits: Limits {
                max_operations: 64,
                max_input_bytes: 1024 * 1024,
                deadline: None,
            },
            operations: operations.into_iter().collect(),
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn permits(&self, operation: &str) -> bool {
        self.operations.contains(operation)
    }

    /// Check the action's declared actor class after checking this context's
    /// operation allowlist. Module-specific access checks still run separately.
    pub fn authorize_action(&self, name: &str, access: &str) -> Result<(), AuthorizationError> {
        if !self.permits(name) {
            return Err(AuthorizationError::OperationNotAllowed);
        }
        match (&self.actor, access) {
            (_, "anonymous") => Ok(()),
            (Actor::Job { .. }, "scheduled") => Ok(()),
            (
                Actor::Job {
                    authority: JobAuthority::ScheduledAction,
                    ..
                },
                "authenticated",
            ) => Ok(()),
            (Actor::Human(_), "authenticated") => Ok(()),
            (Actor::Anonymous, "authenticated") | (Actor::Anonymous, "scheduled") => {
                Err(AuthorizationError::AuthenticationRequired)
            }
            (Actor::Human(_), "scheduled") | (Actor::Job { .. }, "authenticated") => {
                Err(AuthorizationError::ActorNotAllowed)
            }
            _ => Err(AuthorizationError::InvalidAccess),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionMode {
    #[default]
    Live,
    Test,
}

impl ExecutionMode {
    pub fn is_test(self) -> bool {
        self == Self::Test
    }

    pub fn parse(value: Option<&str>, bind: &str) -> anyhow::Result<Self> {
        let mode = match value.unwrap_or("live") {
            "live" => Self::Live,
            "test" => Self::Test,
            other => anyhow::bail!("KONAE_EXECUTION_MODE={other} is not valid. Use live or test."),
        };
        if mode.is_test() && !super::is_local_host(bind) {
            anyhow::bail!(
                "KONAE_EXECUTION_MODE=test requires a loopback KONAE_BIND. Set KONAE_BIND to 127.0.0.1 or ::1."
            );
        }
        Ok(mode)
    }
}

pub const TEST_CAPABILITY_VERSION: u8 = 1;

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"executionTest": TEST_CAPABILITY_VERSION, "schemaTransforms": 1})
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn context_pins_app_release_actor_and_operation_allowlist() {
        let human = Context::new(
            "app-a",
            "release-7",
            Actor::Human("user-1".into()),
            ["orders.read".into()],
        );
        assert_eq!(human.app, "app-a");
        assert_eq!(human.release, "release-7");
        assert!(
            human
                .authorize_action("orders.read", "authenticated")
                .is_ok()
        );
        assert_eq!(
            human.authorize_action("orders.write", "authenticated"),
            Err(AuthorizationError::OperationNotAllowed)
        );
        assert_eq!(
            Context::new(
                "app-b",
                "release-7",
                Actor::Human("user-1".into()),
                ["orders.read".into()]
            )
            .app,
            "app-b"
        );
        assert_eq!(
            human.authorize_action("orders.read", "scheduled"),
            Err(AuthorizationError::ActorNotAllowed)
        );
    }

    #[test]
    fn anonymous_and_job_authority_remain_distinct() {
        let anonymous = Context::new("app", "r1", Actor::Anonymous, ["public.read".into()]);
        assert!(
            anonymous
                .authorize_action("public.read", "anonymous")
                .is_ok()
        );
        assert_eq!(
            anonymous.authorize_action("public.read", "authenticated"),
            Err(AuthorizationError::AuthenticationRequired)
        );
        let job = Context::new(
            "app",
            "r1",
            Actor::Job {
                id: "cron:daily".into(),
                authority: JobAuthority::ScheduledFunction,
            },
            ["daily.run".into()],
        );
        assert!(job.authorize_action("daily.run", "scheduled").is_ok());
        assert_eq!(
            job.authorize_action("daily.run", "authenticated"),
            Err(AuthorizationError::ActorNotAllowed)
        );
        let forged_name = Context::new(
            "app",
            "r1",
            Actor::Job {
                id: "schedule:daily.run".into(),
                authority: JobAuthority::CallContinuation,
            },
            ["daily.run".into()],
        );
        assert_eq!(
            forged_name.authorize_action("daily.run", "authenticated"),
            Err(AuthorizationError::ActorNotAllowed)
        );
        let scheduled_action = Context::new(
            "app",
            "r1",
            Actor::Job {
                id: "daily.run".into(),
                authority: JobAuthority::ScheduledAction,
            },
            ["daily.run".into()],
        );
        assert!(
            scheduled_action
                .authorize_action("daily.run", "authenticated")
                .is_ok()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatcher_gives_a_ready_app_a_turn_while_another_app_is_saturated() {
        let dispatcher = Dispatcher::default();
        let resource = WorkResource::Calls;
        let limits = WorkLimits::new(NonZeroUsize::new(2).unwrap(), NonZeroUsize::MIN);
        dispatcher.wake(resource, "app-a").await;
        let a = dispatcher.next_turn(resource, limits).await;
        assert_eq!(a.app(), "app-a");
        for _ in 0..100 {
            dispatcher.wake(resource, "app-a").await;
        }
        dispatcher.wake(resource, "app-b").await;
        dispatcher.wake(WorkResource::Notices, "mail-app").await;

        let b = dispatcher.next_turn(resource, limits).await;
        assert_eq!(b.app(), "app-b");
        assert_eq!(dispatcher.next(WorkResource::Notices).await, "mail-app");

        dispatcher.wake(resource, "app-c").await;
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let mut pending = std::pin::pin!(dispatcher.next_turn(resource, limits));
        assert!(std::future::Future::poll(pending.as_mut(), &mut context).is_pending());
        drop(b);
        let c = pending.await;
        assert_eq!(c.app(), "app-c");

        let mut retried = std::pin::pin!(dispatcher.next_turn(resource, limits));
        assert!(std::future::Future::poll(retried.as_mut(), &mut context).is_pending());
        drop(a);
        assert_eq!(retried.await.app(), "app-a");
    }

    #[tokio::test]
    async fn inventory_with_more_than_2048_apps_keeps_every_app_ready() {
        let dispatcher = Dispatcher::default();
        let path =
            std::env::temp_dir().join(format!("konae-dispatcher-{}.sqlite", uuid::Uuid::new_v4()));
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE work (app TEXT PRIMARY KEY, pending INTEGER NOT NULL);")
            .unwrap();
        for index in 0..2048 + 37 {
            db.execute("INSERT INTO work VALUES (?1, 1)", [format!("app-{index}")])
                .unwrap();
        }
        let scan = || -> Vec<String> {
            db.prepare("SELECT app FROM work WHERE pending=1 ORDER BY app")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        for app in scan() {
            dispatcher.wake(WorkResource::Calls, app).await;
        }

        let mut handled = BTreeSet::new();
        while handled.len() < 2048 + 37 {
            let app = dispatcher.next(WorkResource::Calls).await;
            handled.insert(app.clone());
            db.execute("UPDATE work SET pending=0 WHERE app=?1", [&app])
                .unwrap();
        }
        assert_eq!(handled.len(), 2048 + 37);
        assert!(handled.contains("app-2084"));
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn app_timers_coalesce_to_one_earliest_deadline_per_app() {
        let now = Instant::now();
        let mut timers = AppTimers::default();
        timers.schedule("app-a".into(), now + std::time::Duration::from_secs(30));
        timers.schedule("app-a".into(), now + std::time::Duration::from_secs(20));
        timers.schedule("app-a".into(), now + std::time::Duration::from_secs(40));
        timers.schedule("app-b".into(), now + std::time::Duration::from_secs(25));
        assert_eq!(
            timers.next_deadline(),
            Some(now + std::time::Duration::from_secs(20))
        );
        assert_eq!(
            timers.take_due(now + std::time::Duration::from_secs(21)),
            vec!["app-a"]
        );
        assert_eq!(
            timers.next_deadline(),
            Some(now + std::time::Duration::from_secs(25))
        );
        timers.schedule_retry("app-a".into(), now);
        assert_eq!(
            timers.next_deadline(),
            Some(now + std::time::Duration::from_secs(5))
        );
        timers.take_due(now + std::time::Duration::from_secs(5));
        timers.schedule_retry("app-a".into(), now);
        assert_eq!(
            timers.next_deadline(),
            Some(now + std::time::Duration::from_secs(10))
        );
        timers.remove("app-a");
        assert_eq!(
            timers.next_deadline(),
            Some(now + std::time::Duration::from_secs(25))
        );
    }

    #[test]
    fn execution_mode_defaults_to_live_and_rejects_unknown_or_reachable_test_mode() {
        assert_eq!(
            ExecutionMode::parse(None, "0.0.0.0").unwrap(),
            ExecutionMode::Live
        );
        assert_eq!(
            ExecutionMode::parse(Some("live"), "0.0.0.0").unwrap(),
            ExecutionMode::Live
        );
        assert_eq!(
            ExecutionMode::parse(Some("test"), "127.0.0.1").unwrap(),
            ExecutionMode::Test
        );
        assert_eq!(
            ExecutionMode::parse(Some("test"), "::1").unwrap(),
            ExecutionMode::Test
        );
        assert!(
            ExecutionMode::parse(Some("other"), "127.0.0.1")
                .unwrap_err()
                .to_string()
                .contains("Use live or test")
        );
        assert!(
            ExecutionMode::parse(Some("test"), "0.0.0.0")
                .unwrap_err()
                .to_string()
                .contains("requires a loopback")
        );
    }

    #[test]
    fn capabilities_publish_the_supported_test_policy_version() {
        assert_eq!(
            capabilities(),
            serde_json::json!({"executionTest": 1, "schemaTransforms": 1})
        );
    }
}
