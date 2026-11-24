use crate::bridge::Bridge;
use anyhow::{Context, Result, ensure};
use konae_functions_executor::{
    WorkerError,
    protocol::{BODY_CHUNK_BYTES, Input, MAX_OPERATION_BYTES, Output, WorkerLimits},
};
use serde_json::Value;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wasmtime::{
    Engine, Store, UpdateDeadline,
    component::{Component, Instance, InstancePre, Linker, TypedFunc},
};

static TEST_RANDOM_SEED_INDEX: AtomicU64 = AtomicU64::new(0);

pub struct Compiler {
    pub engine: Engine,
    linker: Linker<Bridge>,
}

impl Compiler {
    pub fn new() -> Result<Self> {
        let engine = Engine::new(&crate::engine_config::starling())?;
        let mut linker = Linker::<Bridge>::new(&engine);
        let mut host = linker.instance("konae:worker/host")?;
        host.func_wrap(
            "op-worker-call",
            |mut store, (args,): (String,)| -> Result<(String,)> {
                let bridge = store.data_mut();
                if args.len() > MAX_OPERATION_BYTES {
                    bridge.failure("operation_too_large");
                    return Ok((r#"{"Err":"operation_too_large"}"#.into(),));
                }
                let bytes = bridge.call(serde_json::from_str::<Value>(&args)?)?;
                Ok((String::from_utf8(bytes)?,))
            },
        )?;
        host.func_wrap("op-worker-body-read", |mut store, ()| {
            Ok((store.data_mut().body(BODY_CHUNK_BYTES)?,))
        })?;
        host.func_wrap(
            "op-worker-response-chunk",
            |mut store, (bytes,): (Vec<u8>,)| -> Result<(bool,)> {
                Ok((store.data_mut().response_chunk(bytes)?,))
            },
        )?;
        host.func_wrap(
            "op-worker-failure",
            |mut store, (code, detail): (String, Option<String>)| -> Result<()> {
                let code = match code.as_str() {
                    "unsupported_runtime_api" => "unsupported_runtime_api",
                    "function.initialization-cannot-snapshot" => {
                        "function.initialization-cannot-snapshot"
                    }
                    "response_too_large" => "response_too_large",
                    "function_handle_missing" => "function_handle_missing",
                    "handler_error" => "handler_error",
                    _ => "invalid_response",
                };
                if code == "handler_error" {
                    store.data_mut().failure_with_detail(code, detail);
                } else {
                    store.data_mut().failure(code);
                }
                Ok(())
            },
        )?;
        host.func_wrap("now-ms", |store, ()| -> Result<(f64,)> {
            store.data().check()?;
            Ok((SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as f64,))
        })?;
        host.func_wrap("random-seed", |_store, ()| -> Result<(u64,)> {
            if let Some(seed) = std::env::var_os("KONAE_STARLING_TEST_SEED") {
                let seed = seed.to_string_lossy().parse::<u64>()?;
                return Ok((
                    seed.wrapping_add(TEST_RANDOM_SEED_INDEX.fetch_add(1, Ordering::Relaxed)),
                ));
            }
            let mut bytes = [0u8; 8];
            let result = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok((u64::from_le_bytes(bytes),))
        })?;
        Ok(Self { linker, engine })
    }
}

pub struct Invocation {
    instance: Instance,
    store: Store<Bridge>,
}

impl Invocation {
    pub fn pack_prepared(compiler: &Compiler, bytes: &[u8]) -> Result<InstancePre<Bridge>> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= 512 * 1024 * 1024,
            "Invalid prepared component size"
        );
        let component = unsafe { Component::deserialize(&compiler.engine, bytes) }
            .context("Load the prepared StarlingMonkey component")?;
        compiler.linker.instantiate_pre(&component)
    }

    pub fn new(
        compiler: &Compiler,
        bridge: Bridge,
        _limits: &WorkerLimits,
        component: &InstancePre<Bridge>,
    ) -> Result<Self> {
        let mut store = Store::new(&compiler.engine, bridge);
        store.limiter(|bridge| &mut bridge.limits);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|context| {
            context.data().check()?;
            Ok(UpdateDeadline::Continue(1))
        });
        store.data().check()?;
        let instance = component.instantiate(&mut store).map_err(memory_error)?;
        Ok(Self { instance, store })
    }

    pub fn invoke(&mut self, input: Input, limit: usize) -> Result<Output> {
        let input = serde_json::to_string(&input)?;
        let function: TypedFunc<(&str, u32), (String,)> =
            self.instance.get_typed_func(&mut self.store, "handle")?;
        let result = function.call(&mut self.store, (&input, limit as u32));
        if let Some(code) = self.store.data().failure.as_ref() {
            return Err(WorkerError::with_detail(
                code.clone(),
                self.store.data().failure_detail.clone(),
            )
            .into());
        }
        let (output,) = result.map_err(|error| {
            if is_memory_error(&error) {
                memory_error(error)
            } else {
                self.store.data().check().err().unwrap_or_else(|| {
                    WorkerError::with_detail("handler_error", Some(format!("{error:#}"))).into()
                })
            }
        })?;
        function.post_return(&mut self.store)?;
        self.store.data().check()?;
        ensure!(
            output.len() <= limit * 6 + 128 * 1024,
            "Oversized component output"
        );
        let mut response: Output =
            serde_json::from_str(&output).map_err(|_| WorkerError::new("invalid_response"))?;
        let body = self.store.data_mut().take_response();
        let body = std::str::from_utf8(&body).map_err(|_| WorkerError::new("invalid_response"))?;
        response.body = body.strip_prefix('\u{feff}').unwrap_or(body).to_owned();
        self.store.data_mut().request = None;
        Ok(response)
    }

    pub fn shutdown(&mut self, grace_ms: u64) -> (bool, Option<String>) {
        self.store.data_mut().deadline =
            Instant::now() + Duration::from_millis(grace_ms.min(1000).saturating_sub(15));
        self.store.set_epoch_deadline(1);
        let result = self
            .instance
            .get_typed_func::<(), (bool,)>(&mut self.store, "has-shutdown")
            .and_then(|function| {
                let (invoked,) = function.call(&mut self.store, ())?;
                function.post_return(&mut self.store)?;
                Ok(invoked)
            });
        match result {
            Ok(false) => (false, None),
            Ok(true) => (
                true,
                self.call("shutdown")
                    .err()
                    .map(|_| "shutdown_failed".into()),
            ),
            Err(_) => (true, Some("shutdown_failed".into())),
        }
    }

    pub fn spin(&mut self) -> Result<()> {
        self.call("spin")
    }

    fn call(&mut self, name: &str) -> Result<()> {
        let function: TypedFunc<(), ()> = self.instance.get_typed_func(&mut self.store, name)?;
        function.call(&mut self.store, ())?;
        function.post_return(&mut self.store)
    }
}

fn memory_error(error: anyhow::Error) -> anyhow::Error {
    if is_memory_error(&error) {
        WorkerError::new("worker_memory_limit").into()
    } else {
        error
    }
}

fn is_memory_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.starts_with("forcing trap when growing memory")
            || message.starts_with("forcing a memory growth failure")
            || message.starts_with("resource limit exceeded")
    })
}
