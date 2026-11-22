use crate::{
    ExecutionBudget, Input, MemoryPolicy, Output, WorkerLimits,
    containment::Containment,
    protocol::{self, Capability, ChildMessage, ParentMessage},
};
use anyhow::Result;
use serde_json::Value;
use std::{
    fmt, fs,
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const CANCEL_GRACE: Duration = Duration::from_millis(50);

struct BufferedBody {
    bytes: Vec<u8>,
    offset: usize,
}
impl crate::RequestBody for Mutex<BufferedBody> {
    fn read_chunk(&self, maximum: usize) -> std::result::Result<crate::RequestBodyChunk, String> {
        let mut body = self.lock().unwrap();
        let end = body.offset.saturating_add(maximum).min(body.bytes.len());
        let bytes = body.bytes[body.offset..end].to_vec();
        body.offset = end;
        Ok(crate::RequestBodyChunk::new(bytes, end == body.bytes.len()))
    }
    fn complete(&self) -> bool {
        let body = self.lock().unwrap();
        body.offset == body.bytes.len()
    }
}

#[derive(Debug)]
pub struct WorkerError {
    code: String,
}
impl WorkerError {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
    pub fn code(&self) -> &str {
        &self.code
    }
}
impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.code)
    }
}
impl std::error::Error for WorkerError {}
pub fn error_code(error: &anyhow::Error) -> &str {
    error
        .downcast_ref::<WorkerError>()
        .map_or("worker_failed", WorkerError::code)
}
fn fail(code: &str) -> anyhow::Error {
    WorkerError::new(code).into()
}
fn nonce() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| fail("entropy_unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn remaining(budget: &impl ExecutionBudget) -> Result<Duration> {
    let left = budget.deadline().saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(fail("deadline_exceeded"));
    }
    if budget.is_cancelled() {
        return Err(fail("cancelled"));
    }
    Ok(left)
}
struct Until(Instant);
impl ExecutionBudget for Until {
    fn deadline(&self) -> Instant {
        self.0
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}

pub struct Worker {
    child: Option<Child>,
    input: Option<ChildStdin>,
    messages: mpsc::Receiver<ChildMessage>,
    reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    reader_failed: Arc<AtomicBool>,
    stderr: Arc<Mutex<Vec<u8>>>,
    limits: WorkerLimits,
    generation: String,
    request: u64,
    busy: bool,
    synchronized: bool,
    shutdown_invoked: bool,
    containment: Containment,
    oom_kills_at_spawn: Option<u64>,
}
impl Worker {
    pub fn spawn_with_policy(
        executable: &Path,
        function: &str,
        bundle_dir: &Path,
        budget: &impl ExecutionBudget,
        limits: WorkerLimits,
        policy: MemoryPolicy,
    ) -> Result<Self> {
        remaining(budget)?;
        if !limits.validate() {
            return Err(fail("invalid_worker_limits"));
        }
        if function.is_empty()
            || function.len() > 64
            || !function
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(fail("invalid_function_name"));
        }
        let primary = bundle_dir.join(format!("{function}.mjs"));
        let path = if primary.exists() {
            primary
        } else {
            bundle_dir.join(format!("{function}.js"))
        };
        let file = fs::File::open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > protocol::MAX_SOURCE_BYTES as u64 {
            return Err(fail("bundle_too_large"));
        }
        let mut bytes = Vec::new();
        file.take(protocol::MAX_SOURCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > protocol::MAX_SOURCE_BYTES {
            return Err(fail("bundle_too_large"));
        }
        let source = String::from_utf8(bytes).map_err(|_| fail("bundle_invalid_utf8"))?;
        remaining(budget)?;
        let generation = nonce()?;
        let strict_linux = matches!(policy, MemoryPolicy::LinuxCgroup { .. });
        #[cfg(target_os = "linux")]
        let worker_identity = if strict_linux {
            Some(worker_identity()?)
        } else {
            None
        };
        let mut containment = Containment::prepare(policy, limits.rss_bytes, &generation)?;
        let oom_kills_at_spawn = containment.oom_kill_count();
        remaining(budget)?;
        let mut child_command = Command::new(executable);
        child_command
            .arg("--function-worker")
            .env_clear()
            .env(
                "KONAE_INTERNAL_FUNCTIONS_SANDBOX",
                if strict_linux { "linux-v1" } else { "disabled" },
            )
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        #[cfg(target_os = "linux")]
        if let Some((uid, gid)) = worker_identity {
            child_command
                .env("KONAE_INTERNAL_FUNCTIONS_UID", uid.to_string())
                .env("KONAE_INTERNAL_FUNCTIONS_GID", gid.to_string());
            let supervisor_pid = std::process::id() as i32;
            unsafe {
                child_command.pre_exec(move || {
                    let oom_path = b"/proc/self/oom_score_adj\0";
                    let oom_fd =
                        libc::open(oom_path.as_ptr().cast(), libc::O_WRONLY | libc::O_CLOEXEC);
                    if oom_fd < 0 || libc::write(oom_fd, b"0".as_ptr().cast(), 1) != 1 {
                        if oom_fd >= 0 {
                            libc::close(oom_fd);
                        }
                        return Err(std::io::Error::last_os_error());
                    }
                    libc::close(oom_fd);
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != supervisor_pid {
                        return Err(std::io::Error::other(
                            "worker supervisor exited before PDEATHSIG setup",
                        ));
                    }
                    Ok(())
                });
            }
        }
        let mut child = child_command.spawn()?;
        if let Err(error) = containment.attach(child.id()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let mut input = child.stdin.take().unwrap();
        if strict_linux && input.write_all(&protocol::STRICT_WORKER_BOOTSTRAP).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            containment.kill();
            containment.cleanup();
            return Err(fail("worker_failed"));
        }
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();
        // El pipe no bloqueante permite aplicar el deadline también al envío.
        let flags = unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(input.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(fail("worker_pipe_failed"));
        }
        let (tx, messages) = mpsc::sync_channel(4);
        let reader_failed = Arc::new(AtomicBool::new(false));
        let failed = reader_failed.clone();
        let reader = thread::spawn(move || {
            loop {
                match protocol::read::<_, ChildMessage>(&mut stdout) {
                    Ok(message) => {
                        if tx.try_send(message).is_err() {
                            failed.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                    Err(_) => {
                        failed.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let captured = stderr.clone();
        let stderr_reader = thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr_pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut tail = captured.lock().unwrap();
                tail.extend_from_slice(&chunk[..n]);
                if tail.len() > 65536 {
                    let excess = tail.len() - 65536;
                    tail.drain(..excess);
                }
            }
        });
        let mut worker = Self {
            child: Some(child),
            input: Some(input),
            messages,
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            reader_failed,
            stderr,
            limits: limits.clone(),
            generation: generation.clone(),
            request: 0,
            busy: false,
            synchronized: true,
            shutdown_invoked: false,
            containment,
            oom_kills_at_spawn,
        };
        let initialized = (|| {
            worker.send(
                &ParentMessage::Init {
                    abi: protocol::ABI.into(),
                    function: function.into(),
                    source,
                    generation,
                    limits,
                },
                budget,
            )?;
            let ready = match worker.receive(budget) {
                Ok(message) => message,
                Err(error) => {
                    worker.stop();
                    let stderr = worker
                        .stderr
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if stderr
                        .windows(b"invalid_init".len())
                        .any(|window| window == b"invalid_init")
                    {
                        return Err(fail("worker_protocol_mismatch"));
                    }
                    return Err(error);
                }
            };
            match ready {
                ChildMessage::Ready { abi, generation }
                    if abi == protocol::ABI && generation == worker.generation =>
                {
                    Ok(())
                }
                ChildMessage::Failure {
                    capability: None,
                    code,
                } => Err(fail(&code)),
                _ => Err(fail("worker_initialization_failed")),
            }
        })();
        if let Err(error) = initialized {
            worker.stop();
            return Err(error);
        }
        Ok(worker)
    }
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
    pub fn diagnostics(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned()
    }
    pub fn shutdown_invoked(&self) -> bool {
        self.shutdown_invoked
    }
    pub fn is_synchronized(&self) -> bool {
        self.synchronized && self.child.is_some()
    }
    fn memory_check(&self) -> Result<()> {
        let pid = self.pid().ok_or_else(|| fail("worker_failed"))?;
        if let Some(rss) = resident_bytes(pid) {
            if rss > self.limits.rss_bytes {
                return Err(fail("worker_memory_limit"));
            }
        }
        Ok(())
    }
    fn send(&mut self, message: &ParentMessage, budget: &impl ExecutionBudget) -> Result<()> {
        let frame = protocol::encode_parent(message).map_err(|_| fail("frame_too_large"))?;
        let mut offset = 0;
        while offset < frame.len() {
            remaining(budget)?;
            self.memory_check()?;
            match self
                .input
                .as_mut()
                .ok_or_else(|| fail("worker_failed"))?
                .write(&frame[offset..])
            {
                Ok(0) => return Err(fail("worker_failed")),
                Ok(n) => offset += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(1))
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return Err(fail("worker_failed")),
            }
        }
        Ok(())
    }
    fn receive(&mut self, budget: &impl ExecutionBudget) -> Result<ChildMessage> {
        loop {
            let left = remaining(budget)?;
            self.memory_check()?;
            match self
                .messages
                .recv_timeout(left.min(Duration::from_millis(5)))
            {
                Ok(message) => return Ok(message),
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(self.exit_error()),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.reader_failed.load(Ordering::SeqCst) {
                        return Err(self.exit_error());
                    }
                }
            }
        }
    }
    fn exit_error(&self) -> anyhow::Error {
        if oom_kill_confirmed(self.oom_kills_at_spawn, self.containment.oom_kill_count()) {
            fail("worker_memory_limit")
        } else {
            fail("worker_failed")
        }
    }
    fn capability(&mut self) -> Result<Capability> {
        self.request = self
            .request
            .checked_add(1)
            .filter(|x| *x < (1u64 << 53))
            .ok_or_else(|| fail("worker_generation_exhausted"))?;
        Ok(Capability {
            generation: self.generation.clone(),
            request: self.request,
            token: nonce()?,
        })
    }
    pub fn invoke_with(
        &mut self,
        mut input: Input,
        budget: &impl ExecutionBudget,
        callback: &mut dyn FnMut(&str, Value) -> std::result::Result<Value, String>,
    ) -> Result<Output> {
        let body = std::mem::take(&mut input.body);
        if body.len() != input.body_len
            || body.len() > 100_000_000
            || header_bytes(&input.headers) > protocol::MAX_HEADER_BYTES
            || input.url.len() > 8192
            || input.actor.len() > 4096
        {
            return Err(fail("invalid_input"));
        }
        let capability = self.capability()?;
        let message = ParentMessage::Invoke {
            capability: capability.clone(),
            input,
        };
        let body: Arc<dyn crate::RequestBody> = Arc::new(Mutex::new(BufferedBody {
            bytes: body,
            offset: 0,
        }));
        self.execute(message, capability, body, budget, callback)
    }
    pub fn invoke_with_body(
        &mut self,
        input: Input,
        body: Arc<dyn crate::RequestBody>,
        budget: &impl ExecutionBudget,
        callback: &mut dyn FnMut(&str, Value) -> std::result::Result<Value, String>,
    ) -> Result<Output> {
        if !input.body.is_empty()
            || input.body_len > 100_000_000
            || header_bytes(&input.headers) > protocol::MAX_HEADER_BYTES
            || input.url.len() > 8192
            || input.actor.len() > 4096
        {
            return Err(fail("invalid_input"));
        }
        let capability = self.capability()?;
        let message = ParentMessage::Invoke {
            capability: capability.clone(),
            input,
        };
        self.execute(message, capability, body, budget, callback)
    }
    pub fn spin(&mut self, budget: &impl ExecutionBudget) -> Result<Output> {
        let capability = self.capability()?;
        self.execute(
            ParentMessage::Spin {
                capability: capability.clone(),
            },
            capability,
            Arc::new(Mutex::new(BufferedBody {
                bytes: Vec::new(),
                offset: 0,
            })),
            budget,
            &mut |_, _| Err("forbidden".into()),
        )
    }
    fn execute(
        &mut self,
        message: ParentMessage,
        capability: Capability,
        body: Arc<dyn crate::RequestBody>,
        budget: &impl ExecutionBudget,
        callback: &mut dyn FnMut(&str, Value) -> std::result::Result<Value, String>,
    ) -> Result<Output> {
        if self.busy {
            return Err(fail("worker_busy"));
        }
        self.busy = true;
        self.synchronized = false;
        let mut can_resynchronize = false;
        let result = (|| {
            self.send(&message, budget)?;
            can_resynchronize = true;
            let mut sequence = 0u64;
            let mut body_sequence = 0u64;
            let mut native_error: Option<String> = None;
            loop {
                match self.receive(budget)? {
                    ChildMessage::Operation {
                        capability: received,
                        sequence: seq,
                        operation,
                        args,
                    } => {
                        if received != capability || seq != sequence + 1 {
                            can_resynchronize = false;
                            return Err(fail("capability_expired"));
                        }
                        sequence = seq;
                        if sequence > self.limits.operation_limit as u64 {
                            can_resynchronize = false;
                            return Err(fail("operation_limit"));
                        }
                        if !protocol::operation_fits(&operation, &args) {
                            can_resynchronize = false;
                            return Err(fail("operation_too_large"));
                        }
                        let mut interrupted = remaining(budget).err();
                        let mut reply = if let Some(error) = &interrupted {
                            Err(error_code(error).to_owned())
                        } else if let Some(code) = &native_error {
                            Err(code.clone())
                        } else {
                            callback(&operation, args)
                        };
                        if interrupted.is_none() {
                            interrupted = remaining(budget).err();
                        }
                        if let Some(error) = &interrupted {
                            reply = Err(error_code(error).to_owned());
                        }
                        if let Err(code) = &reply {
                            native_error.get_or_insert_with(|| code.clone());
                        }
                        let response = ParentMessage::OperationResult {
                            capability: capability.clone(),
                            sequence,
                            result: reply,
                        };
                        let emergency = Until(Instant::now() + CANCEL_GRACE);
                        if interrupted.is_some() {
                            can_resynchronize = false;
                            self.send(&response, &emergency)?;
                            can_resynchronize = true;
                            return Err(interrupted.unwrap());
                        }
                        can_resynchronize = false;
                        self.send(&response, budget)?;
                        can_resynchronize = true;
                    }
                    ChildMessage::BodyRead {
                        capability: received,
                        sequence: seq,
                        maximum,
                    } => {
                        if received != capability
                            || seq != body_sequence + 1
                            || maximum == 0
                            || maximum > protocol::BODY_CHUNK_BYTES
                        {
                            can_resynchronize = false;
                            return Err(fail("worker_protocol_error"));
                        }
                        body_sequence = seq;
                        remaining(budget)?;
                        let chunk = body.read_chunk(maximum).map_err(|code| fail(&code))?;
                        can_resynchronize = false;
                        self.send(
                            &ParentMessage::BodyChunk {
                                capability: capability.clone(),
                                sequence: body_sequence,
                                body_len: chunk.bytes.len(),
                                done: chunk.done,
                                bytes: chunk.bytes.clone(),
                            },
                            budget,
                        )?;
                        can_resynchronize = true;
                    }
                    ChildMessage::Reply {
                        capability: received,
                        output,
                    } => {
                        if received != capability {
                            return Err(fail("capability_expired"));
                        }
                        self.synchronized = true;
                        if let Some(code) = native_error {
                            return Err(fail(&code));
                        }
                        remaining(budget)?;
                        validate_output(&output, &self.limits)?;
                        return Ok(output);
                    }
                    ChildMessage::Failure {
                        capability: Some(received),
                        code,
                    } if received == capability => {
                        self.synchronized = true;
                        return Err(fail(native_error.as_deref().unwrap_or(&code)));
                    }
                    _ => return Err(fail("worker_protocol_error")),
                }
            }
        })();
        if result.is_err() {
            let code = error_code(result.as_ref().err().unwrap());
            let interrupted = matches!(code, "deadline_exceeded" | "cancelled");
            if interrupted && can_resynchronize && !self.synchronized {
                let grace = Until(Instant::now() + CANCEL_GRACE);
                if self
                    .send(
                        &ParentMessage::Cancel {
                            capability: capability.clone(),
                        },
                        &grace,
                    )
                    .is_ok()
                {
                    if let Ok(message) = self.receive(&grace) {
                        match message {
                            ChildMessage::Failure {
                                capability: Some(received),
                                ..
                            } if received == capability => self.synchronized = true,
                            ChildMessage::Reply {
                                capability: received,
                                output,
                            } if received == capability
                                && validate_output(&output, &self.limits).is_ok() =>
                            {
                                self.synchronized = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
            if !interrupted || !self.synchronized {
                self.stop();
            }
        }
        self.busy = false;
        result
    }
    pub fn shutdown(&mut self, grace: Duration) -> Result<()> {
        if self.child.is_none() {
            return Ok(());
        }
        if self.busy {
            self.stop();
            return Err(fail("worker_busy"));
        }
        let grace = grace.min(Duration::from_secs(1));
        let budget = Until(Instant::now() + grace);
        let result = (|| {
            self.send(
                &ParentMessage::Shutdown {
                    grace_ms: grace.as_millis() as u64,
                },
                &budget,
            )?;
            match self.receive(&budget)? {
                ChildMessage::ShutdownComplete { invoked, error } => {
                    self.shutdown_invoked = invoked;
                    error.map_or(Ok(()), |code| Err(fail(&code)))
                }
                _ => Err(fail("worker_protocol_error")),
            }
        })();
        if !self.stop() && result.is_ok() {
            return Err(fail("containment_cleanup_failed"));
        }
        result
    }
    fn stop(&mut self) -> bool {
        self.synchronized = false;
        self.containment.kill();
        if let Some(mut child) = self.child.take() {
            // El hijo pertenece al grupo que creó este supervisor.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        self.input.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        self.containment.cleanup()
    }
}
#[cfg(target_os = "linux")]
pub fn validate_linux_worker_identity() -> Result<()> {
    worker_identity().map(|_| ())
}

pub fn validate_linux_worker_containment(root: &Path) -> Result<()> {
    let generation = nonce()?;
    let mut containment = Containment::prepare(
        MemoryPolicy::LinuxCgroup {
            delegated_root: root.to_owned(),
        },
        1024 * 1024,
        &generation,
    )?;
    if !containment.cleanup() {
        return Err(fail("containment_unavailable"));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn worker_identity() -> Result<(libc::uid_t, libc::gid_t)> {
    let unavailable = || fail("sandbox_unavailable");
    let uid = std::env::var("KONAE_FUNCTIONS_UID")
        .ok()
        .and_then(|value| value.parse::<libc::uid_t>().ok())
        .filter(|uid| *uid != u32::MAX && *uid != unsafe { libc::geteuid() })
        .ok_or_else(unavailable)?;
    let gid = std::env::var("KONAE_FUNCTIONS_GID")
        .ok()
        .and_then(|value| value.parse::<libc::gid_t>().ok())
        .filter(|gid| *gid != u32::MAX && *gid != unsafe { libc::getegid() })
        .ok_or_else(unavailable)?;
    let status = fs::read_to_string("/proc/self/status").map_err(|_| unavailable())?;
    let effective = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:\t"))
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(unavailable)?;
    let required = (1u64 << 6) | (1u64 << 7);
    if effective & required != required {
        return Err(unavailable());
    }
    Ok((uid, gid))
}
#[cfg(not(target_os = "linux"))]
pub fn validate_linux_worker_identity() -> Result<()> {
    Err(fail("sandbox_unavailable"))
}

fn oom_kill_confirmed(before: Option<u64>, after: Option<u64>) -> bool {
    matches!((before, after), (Some(before), Some(after)) if after > before)
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.shutdown(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_only_a_new_cgroup_oom_as_memory_limit() {
        assert!(oom_kill_confirmed(Some(0), Some(1)));
        assert!(!oom_kill_confirmed(Some(1), Some(1)));
        assert!(!oom_kill_confirmed(Some(1), Some(0)));
        assert!(!oom_kill_confirmed(None, Some(1)));
        assert!(!oom_kill_confirmed(Some(1), None));
    }
}
fn header_bytes(headers: &[(String, String)]) -> usize {
    headers.iter().fold(0usize, |sum, (k, v)| {
        sum.saturating_add(k.len()).saturating_add(v.len())
    })
}
fn validate_output(output: &Output, limits: &WorkerLimits) -> Result<()> {
    if output.body.len() > limits.response_bytes
        || header_bytes(&output.headers) > protocol::MAX_HEADER_BYTES
    {
        return Err(fail("response_too_large"));
    }
    if !(200..=599).contains(&output.status) || output.headers.len() > 128 {
        return Err(fail("invalid_response"));
    }
    for (name, value) in &output.headers {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
            || matches!(
                name.to_ascii_lowercase().as_str(),
                "connection"
                    | "transfer-encoding"
                    | "content-length"
                    | "upgrade"
                    | "trailer"
                    | "keep-alive"
                    | "proxy-authenticate"
                    | "proxy-authorization"
                    | "te"
            )
        {
            return Err(fail("invalid_response"));
        }
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn resident_bytes(pid: u32) -> Option<usize> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>();
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    (read == size as i32).then(|| unsafe { info.assume_init().pti_resident_size as usize })
}
#[cfg(target_os = "linux")]
fn resident_bytes(pid: u32) -> Option<usize> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmRSS:")
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<usize>().ok())
            .map(|kib| kib.saturating_mul(1024))
    })
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn resident_bytes(_: u32) -> Option<usize> {
    None
}
