use anyhow::{Result, bail};
use konae_functions_executor::protocol::{self, Capability, ChildMessage, ParentMessage};
use serde_json::Value;
use std::{
    io,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    time::{Duration, Instant},
};
use wasmtime::{Caller, Linker, Memory, StoreLimits};

pub struct Request {
    pub capability: Capability,
    pub cancelled: AtomicBool,
}

pub struct Bridge {
    pub limits: StoreLimits,
    pub deadline: Instant,
    pub request: Option<Arc<Request>>,
    replies: Arc<Receiver<ParentMessage>>,
    max_operations: usize,
    operations: u64,
    body_reads: u64,
    action_result: Option<Vec<u8>>,
    pub failure: Option<String>,
}

impl Bridge {
    pub fn new(
        limits: StoreLimits,
        deadline: Instant,
        request: Option<Arc<Request>>,
        replies: Arc<Receiver<ParentMessage>>,
        max_operations: usize,
    ) -> Self {
        Self {
            limits,
            deadline,
            request,
            replies,
            max_operations,
            operations: 0,
            body_reads: 0,
            action_result: None,
            failure: None,
        }
    }

    pub fn check(&self) -> Result<()> {
        if self
            .request
            .as_ref()
            .is_some_and(|active| active.cancelled.load(Ordering::Acquire))
        {
            return Err(konae_functions_executor::WorkerError::new("cancelled").into());
        }
        if Instant::now() >= self.deadline {
            return Err(konae_functions_executor::WorkerError::new("deadline_exceeded").into());
        }
        Ok(())
    }

    fn failure(&mut self, code: impl Into<String>) {
        self.failure.get_or_insert_with(|| code.into());
    }

    fn capability(&self) -> Result<Capability> {
        self.check()?;
        self.request
            .as_ref()
            .map(|request| request.capability.clone())
            .ok_or_else(|| konae_functions_executor::WorkerError::new("capability_expired").into())
    }

    fn exchange(&self, message: ChildMessage, capability: &Capability) -> Result<ParentMessage> {
        protocol::write(&mut io::stdout(), &message)?;
        loop {
            self.check()?;
            match self.replies.recv_timeout(Duration::from_millis(1)) {
                Ok(message @ ParentMessage::OperationResult { .. })
                | Ok(message @ ParentMessage::BodyChunk { .. }) => {
                    let received = match &message {
                        ParentMessage::OperationResult { capability, .. }
                        | ParentMessage::BodyChunk { capability, .. } => capability,
                        _ => unreachable!(),
                    };
                    if received == capability {
                        self.check()?;
                        return Ok(message);
                    }
                    if received.generation != capability.generation
                        || received.request >= capability.request
                    {
                        bail!("worker_protocol_error");
                    }
                }
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("worker_protocol_error")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn call(&mut self, args: Value) -> Result<Vec<u8>> {
        let capability = self.capability()?;
        self.operations += 1;
        let sequence = self.operations;
        if sequence > self.max_operations as u64 {
            self.failure("operation_limit");
        }
        if !protocol::operation_fits("action", &args) {
            self.failure("operation_too_large");
        }
        if let Some(code) = &self.failure {
            return Ok(serde_json::to_vec(&Err::<Value, _>(code))?);
        }
        let message = ChildMessage::Operation {
            capability: capability.clone(),
            sequence,
            operation: "action".into(),
            args,
        };
        match self.exchange(message, &capability)? {
            ParentMessage::OperationResult {
                sequence: received,
                result,
                ..
            } if received == sequence => {
                if let Err(code) = &result {
                    self.failure(code.clone());
                }
                Ok(serde_json::to_vec(&result)?)
            }
            _ => bail!("worker_protocol_error"),
        }
    }

    fn body(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let capability = self.capability()?;
        if maximum == 0 || maximum > protocol::BODY_CHUNK_BYTES {
            bail!("worker_protocol_error");
        }
        self.body_reads += 1;
        let sequence = self.body_reads;
        let message = ChildMessage::BodyRead {
            capability: capability.clone(),
            sequence,
            maximum,
        };
        match self.exchange(message, &capability)? {
            ParentMessage::BodyChunk {
                sequence: received,
                bytes,
                done,
                ..
            } if received == sequence && bytes.len() <= maximum && (done || !bytes.is_empty()) => {
                Ok(bytes)
            }
            _ => bail!("worker_protocol_error"),
        }
    }
}

fn memory(caller: &mut Caller<'_, Bridge>) -> Result<Memory> {
    caller
        .get_export("memory")
        .and_then(wasmtime::Extern::into_memory)
        .ok_or_else(|| anyhow::anyhow!("guest memory missing"))
}

fn input<'a>(
    memory: Memory,
    caller: &'a Caller<'_, Bridge>,
    offset: u32,
    length: u32,
) -> Result<&'a [u8]> {
    let end = offset as usize + length as usize;
    memory
        .data(caller)
        .get(offset as usize..end)
        .ok_or_else(|| anyhow::anyhow!("guest buffer out of bounds"))
}

pub fn link(linker: &mut Linker<Bridge>) -> Result<()> {
    linker.func_wrap(
        "konae",
        "op_worker_call",
        |mut caller: Caller<'_, Bridge>,
         offset: u32,
         length: u32,
         out: u32,
         capacity: u32|
         -> Result<i32> {
            let memory = memory(&mut caller)?;
            if length == 0 {
                let Some(bytes) = caller.data_mut().action_result.take() else {
                    bail!("missing action result");
                };
                if bytes.len() != capacity as usize {
                    bail!("invalid action result length");
                }
                memory.write(&mut caller, out as usize, &bytes)?;
                return Ok(capacity as i32);
            }
            let bytes = if length as usize > protocol::MAX_OPERATION_BYTES {
                caller.data_mut().failure("operation_too_large");
                serde_json::to_vec(&Err::<Value, _>("operation_too_large"))?
            } else {
                let args = serde_json::from_slice(input(memory, &caller, offset, length)?)?;
                caller.data_mut().call(args)?
            };
            let count = i32::try_from(bytes.len())?;
            caller.data_mut().action_result = Some(bytes);
            Ok(count)
        },
    )?;
    linker.func_wrap(
        "konae",
        "op_worker_body_read",
        |mut caller: Caller<'_, Bridge>, offset: u32, capacity: u32| -> Result<i32> {
            let memory = memory(&mut caller)?;
            let bytes = caller.data_mut().body(capacity as usize)?;
            memory.write(&mut caller, offset as usize, &bytes)?;
            Ok(bytes.len() as i32)
        },
    )?;
    linker.func_wrap(
        "konae",
        "op_worker_failure",
        |mut caller: Caller<'_, Bridge>, offset: u32, length: u32| -> Result<()> {
            let memory = memory(&mut caller)?;
            if length > 64 {
                bail!("invalid failure code");
            }
            let code = std::str::from_utf8(input(memory, &caller, offset, length)?)?;
            let code = match code {
                "unsupported_runtime_api" => "unsupported_runtime_api",
                "response_too_large" => "response_too_large",
                _ => "invalid_response",
            };
            caller.data_mut().failure(code);
            Ok(())
        },
    )?;
    link_libc(linker)
}

// The SDK reactor imports a clock and libc stubs. The guest receives no descriptors.
fn link_libc(linker: &mut Linker<Bridge>) -> Result<()> {
    let wasi = "wasi_snapshot_preview1";
    linker.func_wrap(
        wasi,
        "clock_time_get",
        |mut caller: Caller<'_, Bridge>, clock: i32, _precision: i64, out: u32| -> Result<i32> {
            static STARTED: OnceLock<Instant> = OnceLock::new();
            let duration = match clock {
                0 => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?,
                1 => STARTED.get_or_init(Instant::now).elapsed(),
                _ => return Ok(28),
            };
            let nanos = duration.as_nanos() as u64;
            memory(&mut caller)?.write(&mut caller, out as usize, &nanos.to_le_bytes())?;
            Ok(0)
        },
    )?;
    linker.func_wrap(wasi, "fd_close", |_fd: u32| -> i32 { 8 })?;
    linker.func_wrap(wasi, "fd_fdstat_get", |_fd: u32, _out: u32| -> i32 { 8 })?;
    linker.func_wrap(
        wasi,
        "fd_seek",
        |_fd: u32, _offset: i64, _whence: i32, _out: u32| -> i32 { 8 },
    )?;
    linker.func_wrap(
        wasi,
        "fd_write",
        |_fd: u32, _iov: u32, _len: u32, _out: u32| -> i32 { 8 },
    )?;
    Ok(())
}
