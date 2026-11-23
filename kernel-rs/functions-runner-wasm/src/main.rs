mod bridge;
mod engine_config;
#[cfg(any(
    all(feature = "quickjs-runner", feature = "starling-runner"),
    not(any(feature = "quickjs-runner", feature = "starling-runner"))
))]
compile_error!("select exactly one runner feature");
#[cfg(feature = "quickjs-runner")]
mod runtime;
#[cfg(feature = "starling-runner")]
mod starling;

use anyhow::{Result, bail};
use bridge::{Bridge, Request};
use konae_functions_executor::{
    self as executor, WorkerError,
    protocol::{self, ChildMessage, ParentMessage},
};
#[cfg(feature = "quickjs-runner")]
use runtime::{Compiler, Invocation};
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

// IPC v3 has no deadline field. The executor sends Cancel at its deadline.
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
        #[cfg(feature = "quickjs-runner")]
        Some("--engine-size") => {
            println!("{}", runtime::COMPILED_ENGINE.len());
            Ok(())
        }
        _ => bail!("expected --function-worker"),
    }
}

enum Command {
    Invoke(Arc<Request>, String, protocol::Input),
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
        bundles,
        generation,
        limits,
    } = protocol::read_parent(&mut input)?
    else {
        bail!("init_required")
    };
    let validation = if abi != protocol::ABI {
        Some("worker_protocol_mismatch")
    } else if !valid_bundles(&bundles) || !limits.validate() {
        Some("invalid_init")
    } else {
        None
    };
    if let Some(code) = validation {
        send(&ChildMessage::Failure {
            capability: None,
            code: code.into(),
            detail: None,
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
                ParentMessage::OperationResult {
                    ref result,
                    ref capability,
                    ..
                } if matches!(result, Err(code) if code == "cancelled" || code == "deadline_exceeded") =>
                {
                    let active = waiting.lock().unwrap();
                    if let Some(request) = active
                        .as_ref()
                        .filter(|request| request.capability == *capability)
                    {
                        request.cancelled.store(true, Ordering::Release);
                        interrupt.increment_epoch();
                    }
                    if replies_tx.try_send(message).is_err() {
                        break;
                    }
                }
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
                        ParentMessage::Invoke {
                            function, input, ..
                        } => Command::Invoke(request, function, input),
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
    let mut prepared = std::collections::BTreeMap::new();
    for (name, source) in &bundles {
        let compile_bridge = Bridge::new(
            store_limits(limits.heap_bytes),
            Instant::now() + PARENT_FAILURE_LIMIT,
            None,
            replies.clone(),
            0,
            limits.response_bytes,
        );
        let component = match Invocation::pack(&compiler, compile_bridge, &limits, source) {
            Ok(component) => component,
            Err(error) => {
                let code = worker_error_code(&error);
                send(&ChildMessage::Failure {
                    capability: None,
                    code: code.into(),
                    detail: Some(error.to_string()),
                })?;
                bail!("{code}");
            }
        };
        prepared.insert(name.clone(), component);
    }
    send(&ChildMessage::Ready { abi, generation })?;

    let mut last = None;
    while let Ok(command) = command_rx.recv() {
        let (request, input) = match command {
            Command::Invoke(request, function, input) => (request, Some((function, input))),
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
            limits.response_bytes,
        );
        let result = match input {
            Some((function, input)) => match prepared.get(&function) {
                Some(component) => Invocation::new(&compiler, bridge, &limits, component)
                    .and_then(|mut guest| {
                        let output = guest.invoke(input, limits.response_bytes)?;
                        last = Some(guest);
                        Ok(output)
                    }),
                None => Err(WorkerError::new("invalid_function_name").into()),
            },
            None => match prepared.iter().next() {
                Some((_, component)) => Invocation::new(&compiler, bridge, &limits, component)
                    .and_then(|mut guest| {
                        guest.spin()?;
                        bail!("unexpected_spin_result");
                    }),
                None => Err(WorkerError::new("invalid_init").into()),
            },
        };
        let capability = request.capability.clone();
        let response = match result {
            Ok(output) => ChildMessage::Reply { capability, output },
            Err(error) => {
                let code = worker_error_code(&error);
                ChildMessage::Failure {
                    capability: Some(capability),
                    code: code.into(),
                    detail: executor::error_detail(&error).map(str::to_owned),
                }
            }
        };
        send(&response)?;
    }
    Ok(())
}

fn valid_bundles(bundles: &std::collections::BTreeMap<String, String>) -> bool {
    let total = bundles.values().map(String::len).sum::<usize>();
    !bundles.is_empty()
        && total <= protocol::MAX_SOURCE_BYTES
        && bundles.iter().all(|(name, source)| {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && !source.is_empty()
                && source.len() <= 1024 * 1024
        })
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

#[cfg(feature = "quickjs-runner")]
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
