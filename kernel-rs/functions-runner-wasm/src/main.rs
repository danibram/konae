mod bridge;
mod engine_config;
#[cfg(not(feature = "starling-runner"))]
mod runtime;
#[cfg(feature = "starling-runner")]
mod starling;

use anyhow::{Result, bail};
use bridge::{Bridge, Request};
use konae_functions_executor::{
    WorkerError,
    protocol::{self, ChildMessage, ParentMessage},
};
#[cfg(not(feature = "starling-runner"))]
use runtime::{Compiler, Invocation};
use sha2::{Digest, Sha256};
#[cfg(feature = "starling-runner")]
use starling::{Compiler, Invocation};
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

// IPC v2 has no deadline field. The executor sends Cancel at its deadline.
const PARENT_FAILURE_LIMIT: Duration = Duration::from_secs(65);

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--function-worker") => worker(),
        #[cfg(feature = "starling-runner")]
        Some("--prepare") => starling::prepare(std::path::Path::new(
            &args
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected a bundle path"))?,
        )),
        #[cfg(not(feature = "starling-runner"))]
        Some("--engine-size") => {
            println!("{}", runtime::COMPILED_ENGINE.len());
            Ok(())
        }
        _ => bail!("expected --function-worker"),
    }
}

enum Command {
    Invoke(Arc<Request>, protocol::Input),
    Spin(Arc<Request>),
    Shutdown(u64),
}

fn worker() -> Result<()> {
    if std::env::var("KONAE_INTERNAL_FUNCTIONS_SANDBOX").as_deref() == Ok("linux-v1") {
        bail!("the Wasm spike requires the executor development policy");
    }
    let mut input = io::stdin();
    let ParentMessage::Init {
        abi,
        source,
        generation,
        limits,
        ..
    } = protocol::read_parent(&mut input)?
    else {
        bail!("init_required")
    };
    let validation = if abi != protocol::ABI {
        Some("worker_protocol_mismatch")
    } else if !limits.validate() || source.len() > protocol::MAX_SOURCE_BYTES {
        Some("invalid_init")
    } else {
        None
    };
    if let Some(code) = validation {
        send(&ChildMessage::Failure {
            capability: None,
            code: code.into(),
        })?;
        bail!("{code}");
    }

    let compiler = Compiler::new()?;
    let (command_tx, command_rx) = mpsc::sync_channel(1);
    let (replies_tx, replies_rx) = mpsc::sync_channel(1);
    let replies = Arc::new(replies_rx);
    let active: Arc<Mutex<Option<Arc<Request>>>> = Arc::new(Mutex::new(None));
    let waiting = active.clone();
    let interrupt = compiler.engine.clone();
    let invoke_generation = generation.clone();
    thread::spawn(move || {
        while let Ok(message) = protocol::read_parent(&mut input) {
            match message {
                ParentMessage::OperationResult { .. } | ParentMessage::BodyChunk { .. } => {
                    if replies_tx.try_send(message).is_err() {
                        break;
                    }
                }
                ParentMessage::Cancel { capability } => {
                    let active = waiting.lock().unwrap();
                    if let Some(request) = active
                        .as_ref()
                        .filter(|request| request.capability == capability)
                    {
                        request.cancelled.store(true, Ordering::Release);
                        interrupt.increment_epoch();
                    }
                }
                ParentMessage::Invoke { ref capability, .. }
                | ParentMessage::Spin { ref capability } => {
                    if capability.generation != invoke_generation {
                        break;
                    }
                    let request = Arc::new(Request {
                        capability: capability.clone(),
                        cancelled: AtomicBool::new(false),
                    });
                    *waiting.lock().unwrap() = Some(request.clone());
                    let command = match message {
                        ParentMessage::Invoke { input, .. } => Command::Invoke(request, input),
                        _ => Command::Spin(request),
                    };
                    if command_tx.try_send(command).is_err() {
                        break;
                    }
                }
                ParentMessage::Shutdown { grace_ms } => {
                    let _ = command_tx.try_send(Command::Shutdown(grace_ms));
                    return;
                }
                ParentMessage::Init { .. } => break,
            }
        }
        if let Some(request) = waiting.lock().unwrap().as_ref() {
            request.cancelled.store(true, Ordering::Release);
            interrupt.increment_epoch();
        }
    });

    let engine = compiler.engine.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(2));
            engine.increment_epoch();
        }
    });
    let bundle_hash = Sha256::digest(source.as_bytes());
    let compile_bridge = Bridge::new(
        store_limits(limits.heap_bytes),
        Instant::now() + PARENT_FAILURE_LIMIT,
        None,
        replies.clone(),
        0,
    );
    // IPC v2 pins one bundle per process, so this cache has a single slot.
    let bytecode = Invocation::pack(&compiler, compile_bridge, &limits, &source).map_err(|error| {
        let code = worker_error_code(&error);
        if code == "worker_execution_failed" {
            eprintln!("bundle {bundle_hash:x}: {error:#}");
        }
        code.to_owned()
    });
    drop(source);
    send(&ChildMessage::Ready { abi, generation })?;

    let mut last = None;
    while let Ok(command) = command_rx.recv() {
        let (request, input) = match command {
            Command::Invoke(request, input) => (request, Some(input)),
            Command::Spin(request) => (request, None),
            Command::Shutdown(grace) => {
                let (invoked, error) = last
                    .as_mut()
                    .map_or((false, None), |guest: &mut Invocation| {
                        guest.shutdown(grace)
                    });
                send(&ChildMessage::ShutdownComplete { invoked, error })?;
                return Ok(());
            }
        };
        drop(last.take());
        let bridge = Bridge::new(
            store_limits(limits.heap_bytes),
            Instant::now() + PARENT_FAILURE_LIMIT,
            Some(request.clone()),
            replies.clone(),
            limits.operation_limit,
        );
        let result = bytecode
            .as_ref()
            .map_err(|code| WorkerError::new(code.as_str()).into())
            .and_then(|bytecode| Invocation::new(&compiler, bridge, &limits, bytecode))
            .and_then(|mut guest| {
                if let Some(input) = input {
                    let output = guest.invoke(input, limits.response_bytes)?;
                    last = Some(guest);
                    Ok(output)
                } else {
                    guest.spin()?;
                    bail!("unexpected_spin_result");
                }
            });
        let capability = request.capability.clone();
        let response = match result {
            Ok(output) => ChildMessage::Reply { capability, output },
            Err(error) => {
                let code = worker_error_code(&error);
                if code == "worker_execution_failed" {
                    eprintln!("bundle {bundle_hash:x}: {error:#}");
                }
                ChildMessage::Failure {
                    capability: Some(capability),
                    code: code.into(),
                }
            }
        };
        send(&response)?;
    }
    Ok(())
}

fn send(message: &ChildMessage) -> io::Result<()> {
    protocol::write(&mut io::stdout(), message)
}

fn worker_error_code(error: &anyhow::Error) -> &str {
    error
        .downcast_ref::<WorkerError>()
        .map(WorkerError::code)
        .unwrap_or("worker_execution_failed")
}

#[cfg(not(feature = "starling-runner"))]
use runtime::store_limits;

#[cfg(feature = "starling-runner")]
fn store_limits(heap: usize) -> wasmtime::StoreLimits {
    wasmtime::StoreLimitsBuilder::new()
        .memory_size(heap)
        .trap_on_grow_failure(true)
        .instances(8)
        .memories(4)
        .tables(8)
        .build()
}
