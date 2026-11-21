use crate::{
    budget::Budget,
    data::{self, Database},
    runtime::{Input, Output, Runtime},
};
use anyhow::{Result, anyhow};
pub use executor::WorkerLimits;
use konae_functions_executor::{
    self as executor,
    protocol::{self, Capability, ChildMessage, ParentMessage},
};
use serde_json::Value;
use std::{
    io::{self, Read},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

pub fn error_code(error: &anyhow::Error) -> &str {
    executor::error_code(error)
}
pub struct Worker {
    process: executor::Worker,
    orders: bool,
}
impl Worker {
    pub fn spawn(
        function: &str,
        bundle_dir: &Path,
        budget: &Budget,
        limits: WorkerLimits,
    ) -> Result<Self> {
        let process = executor::Worker::spawn_with_policy(
            &std::env::current_exe()?,
            function,
            bundle_dir,
            budget,
            limits,
            executor::MemoryPolicy::DevelopmentPolling,
        )?;
        Ok(Self {
            process,
            orders: function == "orders",
        })
    }
    pub fn invoke(
        &mut self,
        input: Input,
        db: &Database,
        budget: &Budget,
        idempotency_key: Option<&str>,
    ) -> Result<Output> {
        let actor = input.actor.clone();
        let mut callback = |operation: &str, args: Value| -> std::result::Result<Value, String> {
            if !self.orders {
                return Err("forbidden".into());
            }
            let object = args.as_object().ok_or("invalid_input")?;
            let id = object
                .get("id")
                .and_then(Value::as_i64)
                .ok_or("invalid_input")?;
            match operation {
                "readOrder" if object.len() == 1 => data::read(db, &actor, id, budget)
                    .map_err(|error| error.code().to_owned())
                    .and_then(|order| {
                        serde_json::to_value(order).map_err(|_| "storage_error".into())
                    }),
                "updateOrder" if object.len() == 2 => {
                    let quantity = object
                        .get("quantity")
                        .and_then(Value::as_i64)
                        .ok_or("invalid_input")?;
                    data::update(
                        db,
                        &actor,
                        id,
                        quantity,
                        idempotency_key.unwrap_or(""),
                        budget,
                    )
                    .map_err(|error| error.code().to_owned())
                    .and_then(|outcome| {
                        serde_json::to_value(outcome.order).map_err(|_| "storage_error".into())
                    })
                }
                _ => Err("forbidden".into()),
            }
        };
        let result = self.process.invoke_with(input, budget, &mut callback);
        match result {
            Err(error) => {
                if budget.cancel().may_have_committed() {
                    Err(executor::WorkerError::new("mutation_outcome_unknown").into())
                } else {
                    Err(error)
                }
            }
            Ok(output)
                if output.status >= 400 && budget.mutation_outcome().may_have_committed() =>
            {
                budget.cancel();
                Err(executor::WorkerError::new("mutation_outcome_unknown").into())
            }
            Ok(output) => Ok(output),
        }
    }
    pub fn spin(&mut self, budget: &Budget) -> Result<Output> {
        self.process.spin(budget)
    }
    pub fn shutdown(&mut self, grace: Duration) -> Result<()> {
        self.process.shutdown(grace)
    }
}

pub struct ChildBridge {
    active: Mutex<Option<Capability>>,
    sequence: AtomicU64,
    cancelled: AtomicBool,
    error: Mutex<Option<String>>,
    output: Mutex<io::Stdout>,
    replies: Mutex<mpsc::Receiver<ParentMessage>>,
    operation_limit: usize,
    body_sequence: AtomicU64,
}
impl ChildBridge {
    pub fn begin(&self, capability: Capability) {
        *self.active.lock().unwrap() = Some(capability);
        self.sequence.store(0, Ordering::SeqCst);
        self.body_sequence.store(0, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);
        self.clear_error();
    }
    pub fn end(&self) {
        *self.active.lock().unwrap() = None;
    }
    pub fn clear_error(&self) {
        *self.error.lock().unwrap() = None;
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    pub fn record_error(&self, code: &str) {
        self.error
            .lock()
            .unwrap()
            .get_or_insert_with(|| code.into());
    }
    fn send(&self, message: &ChildMessage) -> io::Result<()> {
        protocol::write(&mut *self.output.lock().unwrap(), message)
    }
    fn reject(&self, code: &str) -> String {
        self.record_error(code);
        code.into()
    }
    pub fn call(
        &self,
        capability: Capability,
        operation: String,
        args: Value,
    ) -> std::result::Result<Value, String> {
        if self.active.lock().unwrap().as_ref() != Some(&capability) {
            return Err(self.reject("capability_expired"));
        }
        if self.cancelled() {
            return Err(self.reject("cancelled"));
        }
        if self.error().is_some() {
            return Err(self.error().unwrap());
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
        let reply = self
            .replies
            .lock()
            .unwrap()
            .recv()
            .map_err(|_| self.reject("worker_protocol_error"))?;
        match reply {
            ParentMessage::OperationResult {
                capability: returned,
                sequence: seq,
                result,
            } if returned == capability && seq == sequence => {
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
        if self.active.lock().unwrap().as_ref() != Some(&capability)
            || self.cancelled()
            || maximum == 0
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
        match self.replies.lock().unwrap().recv() {
            Ok(ParentMessage::BodyChunk {
                capability: received,
                sequence: received_sequence,
                body_len,
                bytes,
                done,
            }) if received == capability
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

pub fn worker_main() -> Result<()> {
    let strict_linux =
        std::env::var("KONAE_INTERNAL_FUNCTIONS_SANDBOX").as_deref() == Ok("linux-v1");
    let mut input = io::stdin();
    if strict_linux {
        let mut bootstrap = [0u8; executor::protocol::STRICT_WORKER_BOOTSTRAP.len()];
        if input.read_exact(&mut bootstrap).is_err()
            || bootstrap != executor::protocol::STRICT_WORKER_BOOTSTRAP
        {
            eprintln!("strict worker bootstrap marker was invalid");
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
        function,
        source,
        generation,
        limits,
    } = protocol::read(&mut input)?
    else {
        return Err(anyhow!("init_required"));
    };
    if let Err(code) = validate_init(&abi, &limits, &source) {
        return worker_start_failure(code);
    }
    let (reply_tx, reply_rx) = mpsc::sync_channel(1);
    let (command_tx, command_rx) = mpsc::sync_channel(1);
    let bridge = Arc::new(ChildBridge {
        active: Mutex::new(None),
        sequence: AtomicU64::new(0),
        body_sequence: AtomicU64::new(0),
        cancelled: AtomicBool::new(false),
        error: Mutex::new(None),
        output: Mutex::new(io::stdout()),
        replies: Mutex::new(reply_rx),
        operation_limit: limits.operation_limit,
    });
    let reader_bridge = bridge.clone();
    std::thread::spawn(move || {
        loop {
            let message = match protocol::read_parent(&mut input) {
                Ok(message) => message,
                // EOF significa que el supervisor cerró el pipe o murió, incluso si V8 está bloqueado.
                Err(_) => std::process::exit(70),
            };
            match message {
                ParentMessage::OperationResult { .. } => {
                    if reply_tx.try_send(message).is_err() {
                        std::process::exit(70);
                    }
                }
                ParentMessage::BodyChunk { .. } => {
                    if reply_tx.try_send(message).is_err() {
                        std::process::exit(70);
                    }
                }
                ParentMessage::Cancel { capability } => {
                    if reader_bridge.active.lock().unwrap().as_ref() == Some(&capability) {
                        reader_bridge.cancelled.store(true, Ordering::SeqCst);
                    }
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
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut runtime =
        executor.block_on(Runtime::new(&function, source, &limits, bridge.clone()))?;
    bridge.send(&ChildMessage::Ready {
        abi: protocol::ABI.into(),
        generation: generation.clone(),
    })?;
    while let Ok(command) = command_rx.recv() {
        let (capability, result) = match command {
            ParentMessage::Invoke { capability, input } => {
                if capability.generation != generation {
                    return Err(anyhow!("generation_mismatch"));
                }
                let result = executor.block_on(runtime.invoke(input, capability.clone()));
                (capability, result)
            }
            ParentMessage::Spin { capability } => {
                if capability.generation != generation {
                    return Err(anyhow!("generation_mismatch"));
                }
                let result = executor.block_on(runtime.spin(capability.clone()));
                (capability, result)
            }
            ParentMessage::Shutdown { grace_ms } => {
                let result =
                    executor.block_on(runtime.shutdown(Duration::from_millis(grace_ms.min(1000))));
                bridge.send(&ChildMessage::ShutdownComplete {
                    invoked: runtime.has_shutdown,
                    error: result
                        .err()
                        .map(|_| bridge.error().unwrap_or_else(|| "shutdown_failed".into())),
                })?;
                break;
            }
            _ => return Err(anyhow!("invalid_command")),
        };
        bridge.end();
        let reply = match result {
            Ok(output) if bridge.error().is_none() => ChildMessage::Reply { capability, output },
            _ => ChildMessage::Failure {
                capability: Some(capability),
                code: bridge
                    .error()
                    .unwrap_or_else(|| "worker_execution_failed".into()),
            },
        };
        bridge.send(&reply)?;
    }
    Ok(())
}

fn worker_start_failure(code: &str) -> Result<()> {
    let _ = protocol::write(
        &mut io::stdout(),
        &ChildMessage::Failure {
            capability: None,
            code: code.into(),
        },
    );
    Err(executor::WorkerError::new(code).into())
}

fn validate_init(
    abi: &str,
    limits: &protocol::WorkerLimits,
    source: &str,
) -> Result<(), &'static str> {
    if abi != protocol::ABI {
        return Err("worker_protocol_mismatch");
    }
    if !limits.validate() || source.len() > protocol::MAX_SOURCE_BYTES {
        return Err("invalid_init");
    }
    Ok(())
}

#[cfg(test)]
mod protocol_compatibility_tests {
    use super::*;

    #[test]
    fn old_kernel_protocol_is_refused_before_bundle_evaluation() {
        let limits = protocol::WorkerLimits::default();
        assert_eq!(
            validate_init(
                "konae-functions/1",
                &limits,
                "throw new Error('must not run')"
            ),
            Err("worker_protocol_mismatch")
        );
    }
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
