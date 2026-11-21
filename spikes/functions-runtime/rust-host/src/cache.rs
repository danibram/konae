use crate::{
    budget::Budget,
    data::Database,
    runtime::{Input, Output},
    worker::{Worker, WorkerLimits},
};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;

const QUEUE_CAPACITY: usize = 64;
const SHUTDOWN_GRACE: Duration = Duration::from_millis(10);

#[derive(Clone, Hash, Eq, PartialEq)]
struct Key {
    app: String,
    function: String,
    version: String,
}
#[derive(Default)]
struct Counters {
    attempts: AtomicU64,
    created: AtomicU64,
    live: AtomicUsize,
    hits: AtomicU64,
    evictions: AtomicU64,
    timeouts: AtomicU64,
    discarded: AtomicU64,
}
struct Activity {
    created: AtomicU64,
    busy: AtomicUsize,
    requests: AtomicU64,
    last: Mutex<Instant>,
}
enum Job {
    Request(Input),
    Spin,
}
struct Command {
    job: Job,
    budget: Budget,
    idempotency_key: Option<String>,
    reply: oneshot::Sender<Result<Output>>,
}
struct Instance {
    sender: Option<mpsc::SyncSender<Command>>,
    thread: Option<std::thread::JoinHandle<()>>,
    activity: Arc<Activity>,
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn stop_worker(worker: &mut Option<Worker>, counters: &Counters) {
    if let Some(mut worker) = worker.take() {
        let _ = worker.shutdown(SHUTDOWN_GRACE);
        counters.live.fetch_sub(1, Ordering::SeqCst);
    }
}

fn finish_activity(activity: &Activity) {
    *activity.last.lock().unwrap() = Instant::now();
    activity.busy.fetch_sub(1, Ordering::SeqCst);
}

pub struct Cache {
    entries: HashMap<Key, Instance>,
    versions: HashMap<(String, String), String>,
    counters: Arc<Counters>,
    capacity: usize,
    ttl: Duration,
    timeout: Duration,
    bundles: PathBuf,
    limits: WorkerLimits,
}
impl Cache {
    pub fn new(
        capacity: usize,
        ttl: Duration,
        timeout: Duration,
        bundles: PathBuf,
        limits: WorkerLimits,
    ) -> Self {
        Self {
            entries: HashMap::new(),
            versions: HashMap::new(),
            counters: Arc::default(),
            capacity,
            ttl,
            timeout,
            bundles,
            limits,
        }
    }
    pub fn reap(&mut self) {
        let ttl = self.ttl;
        let before = self.entries.len();
        self.entries.retain(|_, i| {
            i.activity.busy.load(Ordering::SeqCst) != 0
                || i.activity.last.lock().unwrap().elapsed() < ttl
        });
        self.counters
            .evictions
            .fetch_add((before - self.entries.len()) as u64, Ordering::SeqCst);
    }
    pub fn evict_all(&mut self) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|_, i| i.activity.busy.load(Ordering::SeqCst) != 0);
        let count = before - self.entries.len();
        self.counters
            .evictions
            .fetch_add(count as u64, Ordering::SeqCst);
        count
    }
    pub fn version(&mut self, app: &str, function: &str, version: &str) -> Result<()> {
        if self.entries.iter().any(|(k, i)| {
            k.app == app && k.function == function && i.activity.busy.load(Ordering::SeqCst) > 0
        }) {
            return Err(anyhow!("cache_busy"));
        }
        let before = self.entries.len();
        self.entries
            .retain(|k, _| k.app != app || k.function != function);
        self.counters
            .evictions
            .fetch_add((before - self.entries.len()) as u64, Ordering::SeqCst);
        self.versions
            .insert((app.to_owned(), function.to_owned()), version.to_owned());
        Ok(())
    }
    pub fn dispatch(
        &mut self,
        app: &str,
        function: &str,
        db: Database,
        input: Option<Input>,
        budget: Budget,
        idempotency_key: Option<String>,
    ) -> Result<oneshot::Receiver<Result<Output>>> {
        budget.check()?;
        self.reap();
        budget.check()?;
        let version = self
            .versions
            .get(&(app.to_owned(), function.to_owned()))
            .cloned()
            .unwrap_or_else(|| "v1".into());
        let key = Key {
            app: app.into(),
            function: function.into(),
            version,
        };
        if self.entries.contains_key(&key) {
            self.counters.hits.fetch_add(1, Ordering::SeqCst);
        } else {
            if self.entries.len() >= self.capacity {
                let oldest = self
                    .entries
                    .iter()
                    .filter(|(_, i)| i.activity.busy.load(Ordering::SeqCst) == 0)
                    .min_by_key(|(_, i)| *i.activity.last.lock().unwrap())
                    .map(|(k, _)| k.clone());
                let Some(oldest) = oldest else {
                    return Err(anyhow!("cache_busy"));
                };
                self.entries.remove(&oldest);
                self.counters.evictions.fetch_add(1, Ordering::SeqCst);
            }
            budget.check()?;
            let (sender, receiver) = mpsc::sync_channel::<Command>(QUEUE_CAPACITY);
            let activity = Arc::new(Activity {
                created: AtomicU64::new(0),
                busy: AtomicUsize::new(0),
                requests: AtomicU64::new(0),
                last: Mutex::new(Instant::now()),
            });
            let work = activity.clone();
            let counters = self.counters.clone();
            let bundles = self.bundles.clone();
            let limits = self.limits.clone();
            let function = function.to_owned();
            let thread = std::thread::Builder::new()
                .name(format!("fn-{app}-{function}"))
                .spawn(move || {
                    let mut worker = None;
                    while let Ok(command) = receiver.recv() {
                        if command.reply.is_closed() {
                            command.budget.cancel();
                        }
                        if let Err(error) = command.budget.check() {
                            counters.discarded.fetch_add(1, Ordering::SeqCst);
                            finish_activity(&work);
                            let _ = command.reply.send(Err(error.into()));
                            continue;
                        }
                        if worker.is_none() {
                            counters.attempts.fetch_add(1, Ordering::SeqCst);
                            match Worker::spawn(
                                &function,
                                &bundles,
                                &command.budget,
                                limits.clone(),
                            ) {
                                Ok(created) => {
                                    counters.created.fetch_add(1, Ordering::SeqCst);
                                    work.created.fetch_add(1, Ordering::SeqCst);
                                    counters.live.fetch_add(1, Ordering::SeqCst);
                                    worker = Some(created);
                                }
                                Err(error) => {
                                    if Instant::now() >= command.budget.deadline() {
                                        counters.timeouts.fetch_add(1, Ordering::SeqCst);
                                    }
                                    command.budget.cancel();
                                    finish_activity(&work);
                                    let _ = command.reply.send(Err(error));
                                    continue;
                                }
                            }
                        }
                        if command.reply.is_closed() {
                            command.budget.cancel();
                        }
                        let result = match command.budget.check() {
                            Err(error) => Err(error.into()),
                            Ok(()) => {
                                work.requests.fetch_add(1, Ordering::SeqCst);
                                let running = worker.as_mut().expect("worker was created");
                                match command.job {
                                    Job::Request(input) => running.invoke(
                                        input,
                                        &db,
                                        &command.budget,
                                        command.idempotency_key.as_deref(),
                                    ),
                                    Job::Spin => running.spin(&command.budget),
                                }
                            }
                        };
                        let result = result.and_then(|output| {
                            command.budget.check()?;
                            Ok(output)
                        });
                        if result.is_err() {
                            if Instant::now() >= command.budget.deadline() {
                                counters.timeouts.fetch_add(1, Ordering::SeqCst);
                            }
                            command.budget.cancel();
                            stop_worker(&mut worker, &counters);
                        }
                        finish_activity(&work);
                        let _ = command.reply.send(result);
                    }
                    stop_worker(&mut worker, &counters);
                })?;
            self.entries.insert(
                key.clone(),
                Instance {
                    sender: Some(sender),
                    thread: Some(thread),
                    activity,
                },
            );
        }
        budget.check()?;
        let instance = self.entries.get(&key).unwrap();
        let (reply, receive) = oneshot::channel();
        instance.activity.busy.fetch_add(1, Ordering::SeqCst);
        if let Err(send_error) = instance.sender.as_ref().unwrap().try_send(Command {
            job: input.map(Job::Request).unwrap_or(Job::Spin),
            budget,
            idempotency_key,
            reply,
        }) {
            instance.activity.busy.fetch_sub(1, Ordering::SeqCst);
            match send_error {
                mpsc::TrySendError::Full(_) => return Err(anyhow!("queue_full")),
                mpsc::TrySendError::Disconnected(_) => {
                    self.entries.remove(&key);
                    return Err(anyhow!("worker_failed"));
                }
            }
        }
        Ok(receive)
    }
    pub fn stats(&self, mode: &str, enabled: bool) -> Value {
        let c = &self.counters;
        json!({
            "mode": mode, "functionsEnabled": enabled,
            "engineInitialized": c.attempts.load(Ordering::SeqCst) > 0,
            "initializationAttempts": c.attempts.load(Ordering::SeqCst),
            "queueCapacity": QUEUE_CAPACITY, "executionModel": "child-process",
            "runtimesCreated": c.created.load(Ordering::SeqCst), "runtimesLive": c.live.load(Ordering::SeqCst),
            "cacheHits": c.hits.load(Ordering::SeqCst), "evictions": c.evictions.load(Ordering::SeqCst),
            "timeouts": c.timeouts.load(Ordering::SeqCst), "discardedJobs": c.discarded.load(Ordering::SeqCst),
            "capacity": self.capacity, "ttlMs": self.ttl.as_millis(), "requestTimeoutMs": self.timeout.as_millis(),
            "workerLimits": {"heapBytes": self.limits.heap_bytes, "responseBytes": self.limits.response_bytes,
                "rssBytes": self.limits.rss_bytes, "operationLimit": self.limits.operation_limit},
            "instances": self.entries.iter().map(|(k,i)| json!({"app": k.app, "function": k.function, "version": k.version,
                "runtimesCreated": i.activity.created.load(Ordering::SeqCst), "requests": i.activity.requests.load(Ordering::SeqCst), "busy": i.activity.busy.load(Ordering::SeqCst),
                "idleMs": i.activity.last.lock().unwrap().elapsed().as_millis()})).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expired_request_creates_neither_a_slot_nor_a_worker() {
        let database = Arc::new(Mutex::new(rusqlite::Connection::open_in_memory().unwrap()));
        let mut cache = Cache::new(
            1,
            Duration::from_secs(1),
            Duration::from_millis(100),
            PathBuf::from("missing-bundles"),
            WorkerLimits::default(),
        );
        let rejected = cache.dispatch(
            "a",
            "orders",
            database,
            None,
            Budget::until(Instant::now()),
            None,
        );
        assert!(rejected.is_err());
        let stats = cache.stats("deno", true);
        assert_eq!(stats["initializationAttempts"], 0);
        assert_eq!(stats["runtimesLive"], 0);
        assert_eq!(stats["instances"], json!([]));
    }
}
