use crate::runtime::Runtime;
use anyhow::{Result, anyhow};
use deno_core::v8;
use konae_functions_executor::{
    self as executor,
    protocol::{self, Capability, ChildMessage, Output, ParentMessage, WorkerLimits},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const CANCEL_GRACE: Duration = Duration::from_millis(10);

pub fn error_code(error: &anyhow::Error) -> &str {
    executor::error_code(error)
}

enum Watchdog {
    Empty,
    Registered(v8::IsolateHandle),
    Armed(ExecutionWatchdog),
}

pub(crate) struct ExecutionWatchdog {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ExecutionWatchdog {
    pub(crate) fn arm_at(isolate: v8::IsolateHandle, deadline: Instant) -> Self {
        let (stop, signal) = mpsc::channel();
        let thread = thread::spawn(move || {
            if signal.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                isolate.terminate_execution();
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for ExecutionWatchdog {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            thread.join().expect("execution watchdog thread panicked");
        }
    }
}

impl Watchdog {
    fn arm(&mut self) {
        let Self::Registered(isolate) = self else {
            return;
        };
        *self = Self::Armed(ExecutionWatchdog::arm_at(
            isolate.clone(),
            Instant::now() + CANCEL_GRACE,
        ));
    }

    fn disarm(&mut self) {
        if let Self::Armed(watchdog) = std::mem::replace(self, Self::Empty) {
            drop(watchdog);
        }
    }
}

struct ActiveInvocation {
    capability: Capability,
    watchdog: Watchdog,
}

pub(crate) struct IsolateRegistration {
    bridge: Arc<ChildBridge>,
    request: u64,
}

impl Drop for IsolateRegistration {
    fn drop(&mut self) {
        let mut active = self.bridge.active.lock().unwrap();
        if let Some(invocation) = active.as_mut()
            && invocation.capability.request == self.request
        {
            invocation.watchdog.disarm();
        }
    }
}

pub struct ChildBridge {
    active: Mutex<Option<ActiveInvocation>>,
    sequence: AtomicU64,
    cancelled: AtomicBool,
    operation_cancelled: AtomicBool,
    error: Mutex<Option<String>>,
    output: Mutex<io::Stdout>,
    replies: Mutex<mpsc::Receiver<ParentMessage>>,
    operation_limit: usize,
    body_sequence: AtomicU64,
}

impl ChildBridge {
    fn begin(&self, capability: Capability) {
        let mut active = self.active.lock().unwrap();
        assert!(active.is_none());
        self.sequence.store(0, Ordering::SeqCst);
        self.body_sequence.store(0, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.operation_cancelled.store(false, Ordering::SeqCst);
        self.clear_error();
        *active = Some(ActiveInvocation {
            capability,
            watchdog: Watchdog::Empty,
        });
    }

    fn end(&self) {
        if let Some(mut invocation) = self.active.lock().unwrap().take() {
            invocation.watchdog.disarm();
        }
    }

    pub(crate) fn register_isolate(
        self: &Arc<Self>,
        isolate: v8::IsolateHandle,
    ) -> IsolateRegistration {
        let mut active = self.active.lock().unwrap();
        let invocation = active
            .as_mut()
            .expect("isolate requires an active invocation");
        assert!(matches!(invocation.watchdog, Watchdog::Empty));
        invocation.watchdog = Watchdog::Registered(isolate);
        if self.cancelled() {
            invocation.watchdog.arm();
        }
        IsolateRegistration {
            bridge: self.clone(),
            request: invocation.capability.request,
        }
    }

    fn clear_error(&self) {
        *self.error.lock().unwrap() = None;
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub(crate) fn record_error(&self, code: &str) {
        self.error
            .lock()
            .unwrap()
            .get_or_insert_with(|| code.into());
    }

    fn cancel(&self, capability: &Capability) {
        let mut active = self.active.lock().unwrap();
        if !active
            .as_ref()
            .is_some_and(|active| &active.capability == capability)
        {
            return;
        }
        self.cancelled.store(true, Ordering::SeqCst);
        self.record_error("cancelled");
        if !self.operation_cancelled.load(Ordering::SeqCst) {
            active.as_mut().unwrap().watchdog.arm();
        }
    }

    fn is_active(&self, capability: &Capability) -> bool {
        self.active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|invocation| &invocation.capability == capability)
    }

    fn send(&self, message: &ChildMessage) -> io::Result<()> {
        protocol::write(&mut *self.output.lock().unwrap(), message)
    }

    fn reject(&self, code: &str) -> String {
        self.record_error(code);
        code.into()
    }

    fn receive(&self) -> std::result::Result<ParentMessage, String> {
        loop {
            match self
                .replies
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_millis(2))
            {
                Ok(_message) if self.cancelled() => return Err(self.reject("cancelled")),
                Ok(message) => return Ok(message),
                Err(mpsc::RecvTimeoutError::Timeout) if self.cancelled() => {
                    return Err(self.reject("cancelled"));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.reject("worker_protocol_error"));
                }
            }
        }
    }

    pub fn call(
        &self,
        capability: Capability,
        operation: String,
        args: Value,
    ) -> std::result::Result<Value, String> {
        if !self.is_active(&capability) {
            return Err(self.reject("capability_expired"));
        }
        if self.cancelled() {
            return Err(self.reject("cancelled"));
        }
        if let Some(error) = self.error() {
            return Err(error);
        }
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        if sequence > self.operation_limit as u64 {
            return Err(self.reject("operation_limit"));
        }
        if !protocol::operation_fits(&operation, &args) {
            return Err(self.reject("operation_too_large"));
        }
        self.send(&ChildMessage::Operation {
            capability: capability.clone(),
            sequence,
            operation,
            args,
        })
        .map_err(|_| self.reject("worker_protocol_error"))?;
        match self.receive()? {
            ParentMessage::OperationResult {
                capability: returned,
                sequence: received_sequence,
                result,
            } if returned == capability && received_sequence == sequence => {
                if self.cancelled() {
                    return Err(self.reject("cancelled"));
                }
                result.map_err(|code| self.reject(&code))
            }
            _ => Err(self.reject("worker_protocol_error")),
        }
    }

    pub fn read_body(
        &self,
        capability: Capability,
        sequence: u64,
        maximum: usize,
    ) -> std::result::Result<protocol::BodyChunk, String> {
        if !self.is_active(&capability) {
            return Err(self.reject("capability_expired"));
        }
        if self.cancelled() {
            return Err(self.reject("cancelled"));
        }
        if maximum == 0
            || maximum > protocol::BODY_CHUNK_BYTES
            || sequence != self.body_sequence.fetch_add(1, Ordering::SeqCst) + 1
        {
            return Err(self.reject("capability_expired"));
        }
        self.send(&ChildMessage::BodyRead {
            capability: capability.clone(),
            sequence,
            maximum,
        })
        .map_err(|_| self.reject("worker_failed"))?;
        match self.receive()? {
            ParentMessage::BodyChunk {
                capability: received,
                sequence: received_sequence,
                body_len,
                bytes,
                done,
            } if received == capability
                && received_sequence == sequence
                && body_len == bytes.len()
                && bytes.len() <= maximum =>
            {
                Ok(protocol::BodyChunk { bytes, done })
            }
            _ => Err(self.reject("worker_protocol_error")),
        }
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::*;

    #[test]
    fn disarm_wakes_and_joins_an_armed_watchdog() {
        let stopped = Arc::new(AtomicBool::new(false));
        let (stop, signal) = mpsc::channel();
        let watchdog_stopped = stopped.clone();
        let thread = thread::spawn(move || {
            assert!(signal.recv().is_err());
            watchdog_stopped.store(true, Ordering::Release);
        });
        let mut watchdog = Watchdog::Armed(ExecutionWatchdog {
            stop: Some(stop),
            thread: Some(thread),
        });

        watchdog.disarm();

        assert!(stopped.load(Ordering::Acquire));
        assert!(matches!(watchdog, Watchdog::Empty));
    }

    #[test]
    fn dropping_an_armed_watchdog_before_its_deadline_keeps_the_isolate_alive() {
        let mut runtime = deno_core::JsRuntime::new(Default::default());
        let watchdog = ExecutionWatchdog::arm_at(
            runtime.v8_isolate().thread_safe_handle(),
            std::time::Instant::now() + Duration::from_secs(60),
        );

        drop(watchdog);

        runtime.execute_script("watchdog-test", "1 + 1").unwrap();
    }
}

pub fn worker_main() -> Result<()> {
    let strict_linux =
        std::env::var("KONAE_INTERNAL_FUNCTIONS_SANDBOX").as_deref() == Ok("linux-v1");
    let mut input = io::stdin();
    if strict_linux {
        let mut bootstrap = [0u8; protocol::STRICT_WORKER_BOOTSTRAP.len()];
        if input.read_exact(&mut bootstrap).is_err()
            || bootstrap != protocol::STRICT_WORKER_BOOTSTRAP
        {
            return worker_start_failure("sandbox_unavailable");
        }
        if let Err(error) = drop_worker_identity() {
            eprintln!("strict worker identity drop failed: {error:#}");
            return worker_start_failure("sandbox_unavailable");
        }
        if let Err(error) = executor::sandbox::enter_worker_sandbox() {
            eprintln!("strict worker OS sandbox failed: {error:#}");
            let code = error
                .downcast_ref::<executor::WorkerError>()
                .map_or("sandbox_unavailable", executor::WorkerError::code);
            return worker_start_failure(code);
        }
    }

    let ParentMessage::Init {
        abi,
        bundles,
        generation,
        limits,
    } = protocol::read(&mut input)?
    else {
        return Err(anyhow!("init_required"));
    };
    if let Err(code) = validate_init(&abi, &limits, &bundles) {
        return worker_start_failure(code);
    }

    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let (command_tx, command_rx) = mpsc::sync_channel(1);
    let bridge = Arc::new(ChildBridge {
        active: Mutex::new(None),
        sequence: AtomicU64::new(0),
        cancelled: AtomicBool::new(false),
        operation_cancelled: AtomicBool::new(false),
        error: Mutex::new(None),
        output: Mutex::new(io::stdout()),
        replies: Mutex::new(reply_rx),
        operation_limit: limits.operation_limit,
        body_sequence: AtomicU64::new(0),
    });
    let reader_bridge = bridge.clone();
    thread::spawn(move || {
        loop {
            let message = match protocol::read_parent(&mut input) {
                Ok(message) => message,
                Err(_) => std::process::exit(70),
            };
            match message {
                ParentMessage::OperationResult { ref result, .. }
                    if matches!(
                        result,
                        Err(code) if code == "cancelled" || code == "deadline_exceeded"
                    ) =>
                {
                    reader_bridge
                        .operation_cancelled
                        .store(true, Ordering::SeqCst);
                    if reply_tx.try_send(message).is_err() {
                        std::process::exit(70);
                    }
                }
                ParentMessage::OperationResult { .. } | ParentMessage::BodyChunk { .. } => {
                    if reply_tx.try_send(message).is_err() {
                        std::process::exit(70);
                    }
                }
                ParentMessage::Cancel { capability } => {
                    reader_bridge.cancel(&capability);
                }
                ParentMessage::Invoke { .. }
                | ParentMessage::Spin { .. }
                | ParentMessage::Shutdown { .. } => {
                    if command_tx.try_send(message).is_err() {
                        std::process::exit(70);
                    }
                }
                _ => std::process::exit(70),
            }
        }
    });

    bridge.send(&ChildMessage::Ready {
        abi: protocol::ABI.into(),
        generation: generation.clone(),
        engine: Some(protocol::EngineKind::Deno),
    })?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut last_runtime = None;

    while let Ok(command) = command_rx.recv() {
        match command {
            ParentMessage::Invoke {
                capability,
                function,
                input,
            } => {
                if capability.generation != generation {
                    return Err(anyhow!("generation_mismatch"));
                }
                let Some(source) = bundles.get(&function) else {
                    send_invocation_result(
                        &bridge,
                        capability,
                        Err(anyhow!("invalid_function_name")),
                    )?;
                    continue;
                };
                drop(last_runtime.take());
                bridge.begin(capability.clone());
                let (result, completed_runtime) = match executor.block_on(Runtime::new(
                    &function,
                    source.clone(),
                    &limits,
                    bridge.clone(),
                )) {
                    Ok(mut runtime) => {
                        let result = executor.block_on(runtime.invoke(input, capability.clone()));
                        let completed = result.is_ok();
                        (result, completed.then_some(runtime))
                    }
                    Err(error) => (Err(error), None),
                };
                bridge.end();
                last_runtime = if bridge.error().is_none() {
                    completed_runtime
                } else {
                    None
                };
                send_invocation_result(&bridge, capability, result)?;
            }
            ParentMessage::Spin { capability } => {
                if capability.generation != generation {
                    return Err(anyhow!("generation_mismatch"));
                }
                let (function, source) = bundles.iter().next().expect("validated bundle map");
                drop(last_runtime.take());
                bridge.begin(capability.clone());
                let result = executor.block_on(async {
                    let mut runtime =
                        Runtime::new(&function, source.clone(), &limits, bridge.clone()).await?;
                    runtime.spin(capability.clone()).await
                });
                bridge.end();
                send_invocation_result(&bridge, capability, result)?;
            }
            ParentMessage::Shutdown { grace_ms } => {
                let (invoked, error) = if let Some(mut runtime) = last_runtime.take() {
                    let invoked = runtime.has_shutdown;
                    let result = executor
                        .block_on(runtime.shutdown(Duration::from_millis(grace_ms.min(1000))));
                    let error = result
                        .err()
                        .map(|_| bridge.error().unwrap_or_else(|| "shutdown_failed".into()));
                    (invoked, error)
                } else {
                    (false, None)
                };
                bridge.end();
                bridge.send(&ChildMessage::ShutdownComplete { invoked, error })?;
                break;
            }
            _ => return Err(anyhow!("invalid_command")),
        }
    }
    Ok(())
}

fn send_invocation_result(
    bridge: &ChildBridge,
    capability: Capability,
    result: Result<Output>,
) -> Result<()> {
    let reply = match result {
        Ok(output) if bridge.error().is_none() => ChildMessage::Reply { capability, output },
        Err(error) => ChildMessage::Failure {
            capability: Some(capability),
            code: bridge
                .error()
                .unwrap_or_else(|| error_code(&error).to_owned()),
            detail: executor::error_detail(&error).map(str::to_owned),
        },
        _ => ChildMessage::Failure {
            capability: Some(capability),
            code: bridge
                .error()
                .unwrap_or_else(|| "worker_execution_failed".into()),
            detail: None,
        },
    };
    bridge.send(&reply)?;
    Ok(())
}

fn worker_start_failure(code: &str) -> Result<()> {
    let _ = protocol::write(
        &mut io::stdout(),
        &ChildMessage::Failure {
            capability: None,
            code: code.into(),
            detail: None,
        },
    );
    Err(executor::WorkerError::new(code).into())
}

fn validate_init(
    abi: &str,
    limits: &WorkerLimits,
    bundles: &BTreeMap<String, String>,
) -> Result<(), &'static str> {
    if abi != protocol::ABI {
        return Err("worker_protocol_mismatch");
    }
    let total = bundles.values().map(String::len).sum::<usize>();
    if !limits.validate()
        || bundles.is_empty()
        || total > protocol::MAX_SOURCE_BYTES
        || bundles.iter().any(|(name, source)| {
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || source.len() > 1024 * 1024
        })
    {
        return Err("invalid_init");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn drop_worker_identity() -> Result<()> {
    let uid = std::env::var("KONAE_INTERNAL_FUNCTIONS_UID")?.parse::<libc::uid_t>()?;
    let gid = std::env::var("KONAE_INTERNAL_FUNCTIONS_GID")?.parse::<libc::gid_t>()?;
    let supervisor = unsafe { libc::getppid() };
    if unsafe { libc::setgroups(0, std::ptr::null()) } != 0
        || unsafe { libc::setresgid(gid, gid, gid) } != 0
        || unsafe { libc::setresuid(uid, uid, uid) } != 0
        || unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe { libc::getppid() } != supervisor {
        return Err(anyhow!("worker supervisor exited during identity drop"));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn drop_worker_identity() -> Result<()> {
    Err(executor::WorkerError::new("sandbox_unavailable").into())
}
