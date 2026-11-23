#[cfg(test)]
use std::path::Path;
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use konae_functions_executor::{
    Budget, Input, MemoryPolicy, Output, Worker, WorkerError, WorkerLimits,
};
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

const INSTANCES: usize = 8;
const ADMISSION: usize = 64;
const SHUTDOWN: Duration = Duration::from_millis(10);

#[derive(Clone)]
struct Config {
    executable: PathBuf,
    policy: MemoryPolicy,
}

struct Slot {
    worker: Arc<AsyncMutex<Option<Worker>>>,
    pins: AtomicUsize,
    last_used: Mutex<Instant>,
    // El permiso se libera después de destruir el worker.
    _allocation: OwnedSemaphorePermit,
}

#[derive(Default)]
struct State {
    slots: HashMap<String, Arc<Slot>>,
    reaper_running: bool,
}

pub struct Runtime {
    config: Option<Config>,
    pub deadline: Duration,
    idle: Duration,
    state: Mutex<State>,
    admission: Arc<Semaphore>,
    active: Arc<Semaphore>,
    allocations: Arc<Semaphore>,
    app_slots: usize,
    protocol_failure: Arc<AtomicBool>,
}

struct Reservation(Arc<Slot>);
impl Drop for Reservation {
    fn drop(&mut self) {
        *self.0.last_used.lock().unwrap() = Instant::now();
        self.0.pins.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Default)]
pub struct RunProgress {
    pub cold: bool,
    pub initialization_started: Option<Instant>,
    pub execution_started: Option<Instant>,
    pub finished: Option<Instant>,
}

pub struct RunResult {
    pub output: anyhow::Result<Output>,
    pub initialization: Duration,
    pub execution: Duration,
    pub cold: bool,
}

pub struct Lease {
    // El guard se suelta antes de que pins permita retirar la instancia.
    worker: OwnedMutexGuard<Option<Worker>>,
    _reservation: Reservation,
    active: Option<OwnedSemaphorePermit>,
    active_limit: Arc<Semaphore>,
    _admission: Option<OwnedSemaphorePermit>,
    config: Config,
    progress: Arc<Mutex<RunProgress>>,
    protocol_failure: Arc<AtomicBool>,
}

impl Runtime {
    pub fn disabled() -> Self {
        Self::new(None, Duration::from_millis(1000), Duration::from_secs(60))
    }

    pub fn configured(
        executable: PathBuf,
        policy: MemoryPolicy,
        deadline: Duration,
        idle: Duration,
    ) -> Self {
        Self::new(Some(Config { executable, policy }), deadline, idle)
    }

    fn new(config: Option<Config>, deadline: Duration, idle: Duration) -> Self {
        Self {
            config,
            deadline,
            idle,
            state: Mutex::new(State::default()),
            admission: Arc::new(Semaphore::new(env_capacity(
                "KONAE_FUNCTIONS_ADMISSION",
                ADMISSION,
            ))),
            active: Arc::new(Semaphore::new(env_capacity(
                "KONAE_FUNCTIONS_INSTANCES",
                INSTANCES,
            ))),
            allocations: Arc::new(Semaphore::new(env_capacity(
                "KONAE_FUNCTIONS_INSTANCES",
                INSTANCES,
            ))),
            app_slots: env_capacity("KONAE_FUNCTIONS_APP_SLOTS", 2),
            protocol_failure: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Test diagnostics: every slot with its pins and whether a worker is present, plus permits.
    #[cfg(test)]
    pub(crate) fn debug_state(&self) -> String {
        let state = self.state.lock().unwrap();
        let slots = state
            .slots
            .iter()
            .map(|(key, slot)| {
                let worker = match slot.worker.try_lock() {
                    Ok(guard) => guard
                        .as_ref()
                        .map(|worker| format!("worker pid={:?} synchronized={}", worker.pid(), worker.is_synchronized()))
                        .unwrap_or_else(|| "no worker".into()),
                    Err(_) => "worker mutex held".into(),
                };
                format!("{key}: pins={} {worker}", slot.pins.load(Ordering::SeqCst))
            })
            .collect::<Vec<_>>();
        format!(
            "slots={slots:?} admission={} active={} allocations={}",
            self.admission.available_permits(),
            self.active.available_permits(),
            self.allocations.available_permits()
        )
    }

    pub fn from_env() -> anyhow::Result<Self> {
        env_capacity_result("KONAE_FUNCTIONS_INSTANCES", INSTANCES)?;
        env_capacity_result("KONAE_FUNCTIONS_APP_SLOTS", 2)?;
        env_capacity_result("KONAE_FUNCTIONS_ADMISSION", ADMISSION)?;
        kernel_environment::create()
    }

    pub(crate) fn admit(&self, budget: &Budget) -> Result<OwnedSemaphorePermit, String> {
        if self.config.is_none() {
            return Err("function.runner-unavailable".into());
        }
        check(budget)?;
        if self.protocol_failure.load(Ordering::SeqCst) {
            return Err("worker_protocol_mismatch".into());
        }
        self.admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| "queue_full".into())
    }

    #[cfg(test)]
    async fn lease(self: &Arc<Self>, key: String, budget: &Budget) -> Result<Lease, String> {
        self.lease_admitted(key, budget, self.admit(budget)?).await
    }

    pub(crate) async fn lease_admitted(
        self: &Arc<Self>,
        key: String,
        budget: &Budget,
        admission: OwnedSemaphorePermit,
    ) -> Result<Lease, String> {
        let config = self.config.clone().ok_or("function.runner-unavailable")?;
        check(budget)?;
        if self.protocol_failure.load(Ordering::SeqCst) {
            return Err("worker_protocol_mismatch".into());
        }
        let reservation = self.reserve(key, budget).await?;
        self.start_reaper();
        let worker = wait(budget, reservation.0.worker.clone().lock_owned()).await?;
        check(budget)?;
        Ok(Lease {
            worker,
            _reservation: reservation,
            active: None,
            active_limit: self.active.clone(),
            _admission: Some(admission),
            config,
            progress: Arc::new(Mutex::new(RunProgress::default())),
            protocol_failure: self.protocol_failure.clone(),
        })
    }

    pub(crate) async fn release_changed(&self, app: &str, version: &str) {
        let prefix = format!("{app}:");
        let retired = {
            let mut state = self.state.lock().unwrap();
            let keys: Vec<_> = state
                .slots
                .keys()
                .filter(|key| {
                    key.strip_prefix(&prefix)
                        .and_then(|key| key.split(':').nth(1))
                        .is_some_and(|installed| installed != version)
                })
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| state.slots.remove(&key))
                .collect::<Vec<_>>()
        };
        if !retired.is_empty() {
            let _ = tokio::task::spawn_blocking(move || {
                for slot in retired {
                    retire(slot);
                }
            })
            .await;
        }
    }

    async fn reserve(&self, key: String, budget: &Budget) -> Result<Reservation, String> {
        let deployment = key.split(':').next().unwrap_or(&key).to_owned();
        loop {
            let retired = {
                let mut state = self.state.lock().unwrap();
                check(budget)?;
                let mut candidates = state
                    .slots
                    .iter()
                    .filter(|(candidate, _)| {
                        candidate.as_str() == key || candidate.starts_with(&format!("{key}#"))
                    })
                    .map(|(candidate, slot)| (candidate.clone(), slot.clone()))
                    .collect::<Vec<_>>();
                candidates.sort_by_key(|(_, slot)| slot.pins.load(Ordering::SeqCst));
                if let Some((_, slot)) = candidates
                    .iter()
                    .find(|(_, slot)| slot.pins.load(Ordering::SeqCst) == 0)
                {
                    slot.pins.fetch_add(1, Ordering::SeqCst);
                    return Ok(Reservation(slot.clone()));
                }
                let app_keys = state
                    .slots
                    .keys()
                    .filter(|candidate| {
                        candidate.as_str() == deployment
                            || candidate.starts_with(&format!("{deployment}:"))
                            || candidate.starts_with(&format!("{deployment}#"))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                if app_keys.len() >= self.app_slots {
                    state
                        .slots
                        .iter()
                        .filter(|(candidate, slot)| {
                            (candidate.as_str() == deployment
                                || candidate.starts_with(&format!("{deployment}:"))
                                || candidate.starts_with(&format!("{deployment}#")))
                                && slot.pins.load(Ordering::SeqCst) == 0
                        })
                        .min_by_key(|(_, slot)| *slot.last_used.lock().unwrap())
                        .map(|(key, _)| key.clone())
                        .and_then(|oldest| state.slots.remove(&oldest))
                } else if let Ok(allocation) = self.allocations.clone().try_acquire_owned() {
                    let index = (0..self.app_slots)
                        .find(|index| !state.slots.contains_key(&format!("{key}#{index}")))
                        .unwrap_or(app_keys.len());
                    let slot_key = if index == 0 && !state.slots.contains_key(&key) {
                        key.clone()
                    } else {
                        format!("{key}#{index}")
                    };
                    let slot = Arc::new(Slot {
                        worker: Arc::new(AsyncMutex::new(None)),
                        pins: AtomicUsize::new(1),
                        last_used: Mutex::new(Instant::now()),
                        _allocation: allocation,
                    });
                    state.slots.insert(slot_key, slot.clone());
                    return Ok(Reservation(slot));
                } else if let Some(oldest) = state
                    .slots
                    .iter()
                    .filter(|(_, slot)| slot.pins.load(Ordering::SeqCst) == 0)
                    .min_by_key(|(_, slot)| *slot.last_used.lock().unwrap())
                    .map(|(key, _)| key.clone())
                {
                    state.slots.remove(&oldest)
                } else {
                    None
                }
            };
            let Some(retired) = retired else {
                wait(budget, tokio::time::sleep(Duration::from_millis(5))).await?;
                continue;
            };
            // La destrucción continúa aunque el solicitante abandone esta espera.
            wait(budget, tokio::task::spawn_blocking(move || retire(retired)))
                .await?
                .map_err(|_| "worker_failed")?;
        }
    }

    fn start_reaper(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap();
        if state.reaper_running || state.slots.is_empty() {
            return;
        }
        state.reaper_running = true;
        let weak = Arc::downgrade(self);
        let interval = self
            .idle
            .clamp(Duration::from_millis(1), Duration::from_millis(100));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(runtime) = weak.upgrade() else {
                    return;
                };
                let (retired, empty) = {
                    let mut state = runtime.state.lock().unwrap();
                    let keys: Vec<_> = state
                        .slots
                        .iter()
                        .filter(|(_, slot)| {
                            slot.pins.load(Ordering::SeqCst) == 0
                                && slot.last_used.lock().unwrap().elapsed() >= runtime.idle
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    let retired: Vec<_> = keys
                        .iter()
                        .filter_map(|key| state.slots.remove(key))
                        .collect();
                    let empty = state.slots.is_empty();
                    if empty {
                        state.reaper_running = false;
                    }
                    (retired, empty)
                };
                drop(runtime);
                if !retired.is_empty() {
                    let _ = tokio::task::spawn_blocking(move || {
                        for slot in retired {
                            retire(slot);
                        }
                    })
                    .await;
                }
                if empty {
                    return;
                }
            }
        });
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.worker.take();
        }
    }
}

impl Lease {
    pub fn take_admission(&mut self) -> Option<OwnedSemaphorePermit> {
        self._admission.take()
    }

    pub fn is_cold(&self) -> bool {
        self.worker.is_none()
    }

    pub fn progress(&self) -> Arc<Mutex<RunProgress>> {
        self.progress.clone()
    }

    #[cfg(test)]
    pub fn run(
        &mut self,
        function: &str,
        bundle_dir: &Path,
        input: Input,
        budget: &Budget,
        callback: &mut dyn FnMut(&str, Value) -> Result<Value, String>,
    ) -> RunResult {
        let primary = bundle_dir.join(format!("{function}.mjs"));
        let source_path = if primary.exists() {
            primary
        } else {
            bundle_dir.join(format!("{function}.js"))
        };
        let source = std::fs::read_to_string(source_path).unwrap_or_default();
        let bundles = BTreeMap::from([(function.to_owned(), source)]);
        self.run_inner(function, &bundles, input, None, budget, callback)
    }

    pub fn run_stream(
        &mut self,
        function: &str,
        bundles: &BTreeMap<String, String>,
        input: Input,
        body: Arc<dyn konae_functions_executor::RequestBody>,
        budget: &Budget,
        callback: &mut dyn FnMut(&str, Value) -> Result<Value, String>,
    ) -> RunResult {
        self.run_inner(function, bundles, input, Some(body), budget, callback)
    }

    fn run_inner(
        &mut self,
        function: &str,
        bundles: &BTreeMap<String, String>,
        input: Input,
        body: Option<Arc<dyn konae_functions_executor::RequestBody>>,
        budget: &Budget,
        callback: &mut dyn FnMut(&str, Value) -> Result<Value, String>,
    ) -> RunResult {
        let cold = self.worker.is_none();
        let mut initialization = Duration::ZERO;
        let mut execution = Duration::ZERO;
        let output = (|| {
            while self.active.is_none() {
                check(budget).map_err(WorkerError::new)?;
                match self.active_limit.clone().try_acquire_owned() {
                    Ok(permit) => self.active = Some(permit),
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            check(budget).map_err(WorkerError::new)?;
            if self.worker.is_none() && self.protocol_failure.load(Ordering::SeqCst) {
                return Err(WorkerError::new("worker_protocol_mismatch").into());
            }
            if cold {
                let started = Instant::now();
                {
                    let mut progress = self.progress.lock().unwrap();
                    progress.cold = true;
                    progress.initialization_started = Some(started);
                }
                let spawned = if std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref()
                    == Ok("wasm")
                {
                    let prepared = crate::functions::prepare_bundles(bundles)?;
                    let descriptors = prepared
                        .into_iter()
                        .map(|(name, path)| Ok((name, std::fs::File::open(path)?)))
                        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
                    Worker::spawn_prepared_with_policy(
                        &self.config.executable,
                        descriptors,
                        budget,
                        WorkerLimits::default(),
                        self.config.policy.clone(),
                    )
                } else {
                    Worker::spawn_with_policy(
                        &self.config.executable,
                        bundles,
                        budget,
                        WorkerLimits::default(),
                        self.config.policy.clone(),
                    )
                };
                initialization = started.elapsed();
                match spawned {
                    Ok(worker) => *self.worker = Some(worker),
                    Err(error) => {
                        if konae_functions_executor::error_code(&error)
                            == "worker_protocol_mismatch"
                        {
                            self.protocol_failure.store(true, Ordering::SeqCst);
                        }
                        return Err(error);
                    }
                }
            }
            check(budget).map_err(WorkerError::new)?;
            let started = Instant::now();
            self.progress.lock().unwrap().execution_started = Some(started);
            let worker = self.worker.as_mut().unwrap();
            let result = match body {
                Some(body) => worker.invoke_with_body(function, input, body, budget, callback),
                None => worker.invoke_with(function, input, budget, callback),
            };
            execution = started.elapsed();
            let output = result?;
            check(budget).map_err(WorkerError::new)?;
            Ok(output)
        })();
        self.progress.lock().unwrap().finished = Some(Instant::now());
        let reusable_failure = output.as_ref().is_err_and(|error| {
            let code = konae_functions_executor::error_code(error);
            let worker_is_synchronized = self.worker.as_ref().is_some_and(Worker::is_synchronized);
            let terminal = matches!(
                code,
                "worker_failed"
                    | "worker_initialization_failed"
                    | "worker_protocol_error"
                    | "worker_protocol_mismatch"
                    | "worker_memory_limit"
                    | "worker_pipe_failed"
                    | "sandbox_unavailable"
                    | "containment_unavailable"
                    | "containment_cleanup_failed"
                    | "invalid_response"
                    | "pending_work"
            );
            worker_is_synchronized && !terminal
        });
        if output.is_err() && !reusable_failure {
            if let Some(mut worker) = self.worker.take() {
                let _ = worker.shutdown(SHUTDOWN);
            }
        }
        RunResult {
            output,
            initialization,
            execution,
            cold,
        }
    }
}

fn env_capacity(name: &str, default: usize) -> usize {
    env_capacity_result(name, default).unwrap_or(default)
}

fn env_capacity_result(name: &str, default: usize) -> anyhow::Result<usize> {
    let value = match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .map_err(|_| anyhow::anyhow!("{name} must be an integer from 1 to 4096"))?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        (1..=4096).contains(&value),
        "{name} must be an integer from 1 to 4096"
    );
    Ok(value)
}

fn retire(slot: Arc<Slot>) {
    if let Some(mut worker) = slot.worker.blocking_lock().take() {
        let _ = worker.shutdown(SHUTDOWN);
    }
}

fn check(budget: &Budget) -> Result<(), String> {
    budget.check().map_err(|error| {
        if budget.cancel().may_have_committed() {
            "mutation_outcome_unknown".to_owned()
        } else {
            error.code().to_owned()
        }
    })
}

async fn wait<T>(budget: &Budget, future: impl Future<Output = T>) -> Result<T, String> {
    check(budget)?;
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => { check(budget)?; return Ok(result); }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(budget.deadline())) => { check(budget)?; }
            _ = tokio::time::sleep(Duration::from_millis(5)) => { check(budget)?; }
        }
    }
}

mod kernel_environment {
    use super::*;

    pub(super) fn create() -> anyhow::Result<Runtime> {
        let Some(executable) = std::env::var_os("KONAE_FUNCTIONS_RUNNER") else {
            return Ok(Runtime::disabled());
        };
        let executable = PathBuf::from(executable);
        anyhow::ensure!(
            executable.is_absolute() && executable.is_file(),
            "KONAE_FUNCTIONS_RUNNER must name an absolute executable file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                executable.metadata()?.permissions().mode() & 0o111 != 0,
                "KONAE_FUNCTIONS_RUNNER must be executable"
            );
        }
        let policy = if std::env::var("KONAE_FUNCTIONS_DEVELOPMENT").as_deref() == Ok("1") {
            MemoryPolicy::DevelopmentPolling
        } else if let Some(root) = std::env::var_os("KONAE_FUNCTIONS_CGROUP") {
            konae_functions_executor::validate_linux_worker_identity()
                .map_err(|_| anyhow::anyhow!("KONAE_FUNCTIONS_UID and KONAE_FUNCTIONS_GID must name a separate worker identity, and the Kernel must hold CAP_KILL, CAP_SETUID and CAP_SETGID"))?;
            let delegated_root = PathBuf::from(root);
            anyhow::ensure!(
                delegated_root.is_absolute(),
                "KONAE_FUNCTIONS_CGROUP must be absolute"
            );
            konae_functions_executor::validate_linux_worker_containment(&delegated_root)
                .map_err(|error| anyhow::anyhow!("KONAE_FUNCTIONS_CGROUP must be a writable cgroup v2 subtree with memory, pids and cgroup.kill enabled: {error}"))?;
            MemoryPolicy::LinuxCgroup { delegated_root }
        } else {
            anyhow::bail!(
                "KONAE_FUNCTIONS_CGROUP is required when KONAE_FUNCTIONS_RUNNER is set, unless KONAE_FUNCTIONS_DEVELOPMENT=1"
            )
        };
        Ok(Runtime::configured(
            executable,
            policy,
            env_duration("KONAE_FUNCTIONS_TIMEOUT_MS", 1000)?,
            env_duration("KONAE_FUNCTIONS_IDLE_MS", 60_000)?,
        ))
    }

    fn env_duration(name: &str, default: u64) -> anyhow::Result<Duration> {
        let milliseconds = match std::env::var(name) {
            Ok(value) => value
                .parse::<u64>()
                .map_err(|_| anyhow::anyhow!("{name} must be a positive millisecond count"))?,
            Err(std::env::VarError::NotPresent) => default,
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(
            milliseconds > 0 && milliseconds <= 86_400_000,
            "{name} must be between 1 and 86400000"
        );
        Ok(Duration::from_millis(milliseconds))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(idle: Duration) -> Arc<Runtime> {
        Arc::new(Runtime::configured(
            PathBuf::from("/nonexistent/konae-worker"),
            MemoryPolicy::DevelopmentPolling,
            Duration::from_secs(2),
            idle,
        ))
    }
    fn budget() -> Budget {
        Budget::new(Duration::from_secs(5))
    }
    async fn eventually(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("condition did not become true");
    }
    fn input(actor: &str, path: &str) -> Input {
        Input {
            url: format!("https://function.invalid/{path}"),
            method: "GET".into(),
            headers: vec![],
            body: vec![],
            body_len: 0,
            actor: actor.into(),
        }
    }

    #[tokio::test]
    async fn disabled_and_expired_requests_create_no_slots_or_tasks() {
        let disabled = Arc::new(Runtime::disabled());
        assert_eq!(
            disabled.lease("a".into(), &budget()).await.err().unwrap(),
            "function.runner-unavailable"
        );
        assert!(disabled.state.lock().unwrap().slots.is_empty());
        assert!(!disabled.state.lock().unwrap().reaper_running);
        let runtime = runtime(Duration::from_secs(60));
        assert_eq!(
            runtime
                .lease("a".into(), &Budget::new(Duration::ZERO))
                .await
                .err()
                .unwrap(),
            "deadline_exceeded"
        );
        assert!(runtime.state.lock().unwrap().slots.is_empty());
        assert!(!runtime.state.lock().unwrap().reaper_running);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires KONAE_TEST_LEGACY_FUNCTION_WORKER pointing to the frozen IPC v1 runner"]
    async fn legacy_runner_mismatch_is_reaped_and_latched_without_retry() {
        let executable =
            PathBuf::from(std::env::var_os("KONAE_TEST_LEGACY_FUNCTION_WORKER").unwrap());
        let runtime = Arc::new(Runtime::configured(
            executable.clone(),
            MemoryPolicy::DevelopmentPolling,
            Duration::from_secs(5),
            Duration::from_secs(60),
        ));
        let root = std::env::temp_dir().join(format!("konae-worker-abi-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("orders.mjs"),
            "throw new Error('bundle must not execute')",
        )
        .unwrap();
        let first_budget = budget();
        let admission = runtime.admit(&first_budget).unwrap();
        let lease = runtime
            .lease_admitted("a:orders".into(), &first_budget, admission)
            .await
            .unwrap();
        let run_budget = budget();
        let run_root = root.clone();
        let (lease, result) = tokio::task::spawn_blocking(move || {
            let mut lease = lease;
            let result = lease.run(
                "orders",
                &run_root,
                input("alice", "orders"),
                &run_budget,
                &mut |_, _| Ok(Value::Null),
            );
            (lease, result)
        })
        .await
        .unwrap();
        assert_eq!(
            konae_functions_executor::error_code(result.output.as_ref().unwrap_err()),
            "worker_protocol_mismatch"
        );
        assert!(lease.worker.is_none());
        drop(lease);
        assert_eq!(
            runtime.admit(&budget()).unwrap_err(),
            "worker_protocol_mismatch"
        );
        let output = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid=,command="])
            .output()
            .unwrap();
        let ppid = std::process::id().to_string();
        let command = String::from_utf8_lossy(&output.stdout);
        assert!(!command.lines().any(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            fields.get(1) == Some(&ppid.as_str())
                && line.contains(executable.to_str().unwrap())
                && line.contains("--function-worker")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn leases_obey_per_deployment_pool_cap() {
        let runtime = runtime(Duration::from_secs(60));
        let other_app = runtime
            .lease("other:orders".into(), &budget())
            .await
            .unwrap();
        drop(other_app);
        let first = runtime.lease("app:orders".into(), &budget()).await.unwrap();
        let second = runtime.lease("app:orders".into(), &budget()).await.unwrap();
        let waiting_runtime = runtime.clone();
        let deadline = Budget::new(Duration::from_millis(80));
        let waiting =
            tokio::spawn(
                async move { waiting_runtime.lease("app:orders".into(), &deadline).await },
            );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiting.is_finished(),
            "third request should queue at the app cap"
        );
        {
            let state = runtime.state.lock().unwrap();
            let app_slots = state
                .slots
                .keys()
                .filter(|key| key.as_str() == "app:orders" || key.starts_with("app:orders#"))
                .count();
            assert_eq!(app_slots, 2);
            assert!(state.slots.contains_key("other:orders"));
        }
        assert_eq!(waiting.await.unwrap().err().unwrap(), "deadline_exceeded");
        assert!(
            runtime
                .state
                .lock()
                .unwrap()
                .slots
                .contains_key("other:orders")
        );
        assert!(first.worker.is_none() && second.worker.is_none());
    }

    #[tokio::test]
    async fn an_unread_body_lease_does_not_hold_an_active_permit() {
        let runtime = runtime(Duration::from_secs(60));
        let available = runtime.active.available_permits();
        let lease = runtime.lease("app:orders".into(), &budget()).await.unwrap();
        assert_eq!(runtime.active.available_permits(), available);
        drop(lease);
        assert_eq!(runtime.active.available_permits(), available);
    }

    #[tokio::test]
    async fn request_admission_outlives_the_worker_lease() {
        let runtime = runtime(Duration::from_secs(60));
        let mut lease = runtime.lease("app:orders".into(), &budget()).await.unwrap();
        let admission = lease.take_admission().unwrap();
        drop(lease);
        assert_eq!(runtime.admission.available_permits(), ADMISSION - 1);
        assert!(
            runtime.state.lock().unwrap().slots["app:orders"]
                .pins
                .load(Ordering::SeqCst)
                == 0
        );
        drop(admission);
        assert_eq!(runtime.admission.available_permits(), ADMISSION);
    }

    #[tokio::test]
    async fn same_deployment_queues_a_third_until_a_worker_is_free() {
        let runtime = runtime(Duration::from_secs(60));
        let held = runtime.lease("a".into(), &budget()).await.unwrap();
        let second = runtime.lease("a".into(), &budget()).await.unwrap();
        let waiting_runtime = runtime.clone();
        let waiting_budget = budget();
        let waiting =
            tokio::spawn(async move { waiting_runtime.lease("a".into(), &waiting_budget).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiting.is_finished(),
            "third request should queue at the app cap"
        );
        assert_eq!(runtime.admission.available_permits(), ADMISSION - 3);
        drop(held);
        let third = tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("queued request should wake when a pin is released")
            .unwrap()
            .unwrap();
        assert_eq!(runtime.state.lock().unwrap().slots.len(), 2);
        assert!(second.worker.is_none());
        assert!(third.worker.is_none());
    }

    #[tokio::test]
    async fn admission_is_bounded_and_released_with_its_permits() {
        let runtime = runtime(Duration::from_secs(60));
        let mut permits = (0..ADMISSION)
            .map(|_| runtime.admit(&budget()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            runtime.lease("b".into(), &budget()).await.err().unwrap(),
            "queue_full"
        );
        permits.pop();
        assert_eq!(runtime.admission.available_permits(), 1);
        drop(permits);
        assert_eq!(runtime.admission.available_permits(), ADMISSION);
    }

    #[tokio::test]
    async fn expired_and_cancelled_requests_do_not_reserve_workers() {
        let runtime = runtime(Duration::from_secs(60));
        let expired = Budget::until(Instant::now() - Duration::from_millis(1));
        assert_eq!(
            runtime.lease("a".into(), &expired).await.err().unwrap(),
            "deadline_exceeded"
        );
        let cancelled = budget();
        cancelled.cancel();
        assert_eq!(
            runtime.lease("a".into(), &cancelled).await.err().unwrap(),
            "cancelled"
        );
        assert!(runtime.state.lock().unwrap().slots.is_empty());
        assert_eq!(runtime.admission.available_permits(), ADMISSION);
    }

    #[tokio::test]
    async fn idle_reaper_preserves_held_leases_then_stops_when_empty() {
        let runtime = runtime(Duration::from_millis(10));
        let held = runtime.lease("a".into(), &budget()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(runtime.state.lock().unwrap().slots.len(), 1);
        drop(held);
        eventually(|| !runtime.state.lock().unwrap().reaper_running).await;
        eventually(|| runtime.allocations.available_permits() == INSTANCES).await;
        assert!(runtime.state.lock().unwrap().slots.is_empty());
        let again = runtime.lease("a".into(), &budget()).await.unwrap();
        assert!(runtime.state.lock().unwrap().reaper_running);
        assert!(again.worker.is_none());
    }

    #[tokio::test]
    async fn spawn_failure_retains_initialization_progress_and_releases_worker() {
        let runtime = runtime(Duration::from_secs(60));
        let mut lease = runtime.lease("a".into(), &budget()).await.unwrap();
        let progress = lease.progress();
        let result = tokio::task::spawn_blocking(move || {
            let result = lease.run(
                "a",
                Path::new("/nonexistent"),
                input("alice", ""),
                &budget(),
                &mut |_, _| panic!("unexpected native callback"),
            );
            assert!(lease.worker.is_none());
            result
        })
        .await
        .unwrap();
        assert!(result.output.is_err());
        assert!(result.cold);
        let progress = progress.lock().unwrap();
        assert!(progress.cold);
        assert!(progress.initialization_started.is_some());
        assert!(progress.execution_started.is_none());
        assert!(progress.finished.is_some());
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("konae-runtime-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(
                path.join("example.mjs"),
                r#"
let count = 0;
export function handle(request, context) {
    if (new URL(request.url).pathname === '/spin') while (true) {}
    if (new URL(request.url).pathname === '/fail') throw new Error('test failure');
    return new Response(`${++count}:${context.actor.userId}`);
}
"#,
            )
            .unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn real_runtime(idle: Duration) -> Arc<Runtime> {
        let path = PathBuf::from(
            std::env::var_os("KONAE_TEST_FUNCTION_WORKER")
                .expect("set KONAE_TEST_FUNCTION_WORKER to the built spike executable"),
        );
        Arc::new(Runtime::configured(
            path,
            MemoryPolicy::DevelopmentPolling,
            Duration::from_secs(2),
            idle,
        ))
    }
    async fn run(
        runtime: &Arc<Runtime>,
        key: &str,
        directory: &Path,
        actor: &str,
        path: &str,
    ) -> (RunResult, Option<u32>) {
        run_with_budget(runtime, key, directory, actor, path, Duration::from_secs(5)).await
    }
    async fn run_with_budget(
        runtime: &Arc<Runtime>,
        key: &str,
        directory: &Path,
        actor: &str,
        path: &str,
        timeout: Duration,
    ) -> (RunResult, Option<u32>) {
        let budget = Budget::new(timeout);
        let mut lease = runtime.lease(key.to_owned(), &budget).await.unwrap();
        let directory = directory.to_owned();
        let input = input(actor, path);
        tokio::task::spawn_blocking(move || {
            let result = lease.run("example", &directory, input, &budget, &mut |_, _| {
                panic!("unexpected native callback")
            });
            let pid = lease.worker.as_ref().and_then(Worker::pid);
            (result, pid)
        })
        .await
        .unwrap()
    }
    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[tokio::test]
    #[ignore = "requires KONAE_TEST_FUNCTION_WORKER and starts real child processes"]
    async fn real_worker_reuses_after_handler_error_and_idle_processes() {
        let fixture = Fixture::new();
        let runtime = real_runtime(Duration::from_millis(200));
        let (first, pid) = run(&runtime, "app:example:v1", &fixture.0, "alice", "").await;
        assert_eq!(first.output.unwrap().body, "1:alice");
        assert!(first.cold);
        let pid = pid.unwrap();
        let (second, second_pid) = run(&runtime, "app:example:v1", &fixture.0, "bob", "").await;
        assert_eq!(second.output.unwrap().body, "1:bob");
        assert!(!second.cold);
        assert_eq!(second_pid, Some(pid));
        let (spun, spin_pid) = run_with_budget(
            &runtime,
            "app:example:v1",
            &fixture.0,
            "alice",
            "spin",
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(
            konae_functions_executor::error_code(spun.output.as_ref().unwrap_err()),
            "deadline_exceeded"
        );
        assert_eq!(spin_pid, Some(pid));
        let (after_spin, after_spin_pid) =
            run(&runtime, "app:example:v1", &fixture.0, "carol", "").await;
        assert_eq!(after_spin.output.unwrap().body, "1:carol");
        assert_eq!(after_spin_pid, Some(pid));
        let (failed, failed_pid) =
            run(&runtime, "app:example:v1", &fixture.0, "alice", "fail").await;
        let handler_failure =
            konae_functions_executor::error_code(failed.output.as_ref().unwrap_err());
        assert_eq!(handler_failure, "handler_error");
        let detail =
            konae_functions_executor::error_detail(failed.output.as_ref().unwrap_err()).unwrap();
        assert!(detail.len() <= 4096);
        assert!(!detail.contains("alice"));
        assert_eq!(failed_pid, Some(pid));
        assert!(alive(pid));
        let (recovered, recovered_pid) =
            run(&runtime, "app:example:v1", &fixture.0, "alice", "").await;
        assert!(!recovered.cold);
        assert_eq!(recovered.output.unwrap().body, "1:alice");
        assert_eq!(recovered_pid, Some(pid));
        eventually(|| !alive(pid)).await;
        eventually(|| runtime.allocations.available_permits() == INSTANCES).await;
        assert!(!runtime.state.lock().unwrap().reaper_running);
    }

    #[tokio::test]
    #[ignore = "requires KONAE_TEST_FUNCTION_WORKER and starts real child processes"]
    async fn real_worker_new_version_and_lru_eviction_reap_before_replacement() {
        let fixture = Fixture::new();
        let runtime = real_runtime(Duration::from_secs(60));
        let mut pids = Vec::new();
        for version in 0..2 {
            let (result, pid) = run(
                &runtime,
                &format!("app:example:v{version}"),
                &fixture.0,
                "alice",
                "",
            )
            .await;
            assert!(result.cold);
            assert_eq!(result.output.unwrap().body, "1:alice");
            pids.push(pid.unwrap());
        }
        assert!(pids.iter().all(|pid| alive(*pid)));
        let (result, third_pid) = run(&runtime, "app:example:v2", &fixture.0, "bob", "").await;
        assert!(result.cold);
        assert_eq!(result.output.unwrap().body, "1:bob");
        assert!(!alive(pids[0]));
        assert!(alive(pids[1]));
        let third_pid = third_pid.unwrap();
        runtime.release_changed("app", "v2").await;
        assert!(pids.iter().all(|pid| !alive(*pid)));
        assert!(alive(third_pid));
        drop(runtime);
        assert!(!alive(third_pid));
    }
}
