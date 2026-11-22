use crate::bridge::{self, Bridge};
use anyhow::{Context, Result, anyhow};
use konae_functions_executor::{
    WorkerError,
    protocol::{Input, Output, WorkerLimits},
};
use std::time::{Duration, Instant};
use wasmtime::{
    Engine, Instance, InstancePre, Linker, Module, Store, StoreLimitsBuilder, Trap, UpdateDeadline,
};

pub const COMPILED_ENGINE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quickjs.cwasm"));
const BOOTSTRAP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bootstrap.qbc"));

pub struct Compiler {
    pub engine: Engine,
    pub instance: InstancePre<Bridge>,
}

impl Compiler {
    pub fn new() -> Result<Self> {
        let engine = Engine::new(&crate::engine_config::config())?;
        // build.rs compiles the pinned engine for this binary. The app supplies only JavaScript.
        let module = unsafe { Module::deserialize(&engine, COMPILED_ENGINE)? };
        let mut linker = Linker::new(&engine);
        bridge::link(&mut linker)?;
        let instance = linker.instantiate_pre(&module)?;
        Ok(Self { engine, instance })
    }
}

pub struct Invocation {
    instance: Instance,
    store: Store<Bridge>,
}

impl Invocation {
    fn start(compiler: &Compiler, bridge: Bridge, limits: &WorkerLimits) -> Result<Self> {
        let mut store = Store::new(&compiler.engine, bridge);
        store.limiter(|bridge| &mut bridge.limits);
        store.set_epoch_deadline(1);
        store.epoch_deadline_callback(|context| {
            context.data().check()?;
            Ok(UpdateDeadline::Continue(1))
        });
        store.data().check()?;
        let instance = compiler.instance.instantiate(&mut store).map_err(|error| {
            if limits.heap_bytes < 2 * 1024 * 1024 {
                WorkerError::new("worker_memory_limit").into()
            } else {
                error
            }
        })?;
        let mut result = Self { instance, store };
        result.call::<(), ()>("_initialize", ())?;
        Ok(result)
    }

    pub fn pack(
        compiler: &Compiler,
        bridge: Bridge,
        limits: &WorkerLimits,
        source: &str,
    ) -> Result<Vec<u8>> {
        let mut guest = Self::start(compiler, bridge, limits)?;
        let ptr = guest.copy_into_guest(source.as_bytes())?;
        let code = guest.call(
            "guest_pack",
            (ptr, source.len() as u32, heap_limit(limits), 1_u32),
        )?;
        check_guest_code(code).context("Compile ESM")?;
        guest.reply(|bytes| Ok(bytes.to_vec()))
    }

    pub fn new(
        compiler: &Compiler,
        bridge: Bridge,
        limits: &WorkerLimits,
        bytecode: &[u8],
    ) -> Result<Self> {
        let mut result = Self::start(compiler, bridge, limits)?;
        let ptr = result.copy_into_guest(BOOTSTRAP)?;
        let code = result.call(
            "guest_init",
            (ptr, BOOTSTRAP.len() as u32, heap_limit(limits)),
        )?;
        check_guest_code(code).context("Web bootstrap")?;
        let ptr = result.copy_into_guest(bytecode)?;
        let code = result.call("guest_load", (ptr, bytecode.len() as u32))?;
        check_guest_code(code).context("ESM load")?;
        Ok(result)
    }

    pub fn invoke(&mut self, input: Input, limit: usize) -> Result<Output> {
        let json = serde_json::to_vec(&input)?;
        let ptr = self.copy_into_guest(&json)?;
        let code = self.call("guest_invoke", (ptr, json.len() as u32, limit as u32))?;
        if let Some(code) = self.store.data().failure.as_ref() {
            return Err(WorkerError::new(code).into());
        }
        check_guest_code(code).context("handler")?;
        self.store.data().check()?;
        let response = self.reply(|json| {
            if json.len() > limit * 6 + 128 * 1024 {
                return Err(WorkerError::new("response_too_large").into());
            }
            serde_json::from_slice(json).map_err(|_| WorkerError::new("invalid_response").into())
        })?;
        self.call::<(), ()>("guest_end_request", ())?;
        self.store.data_mut().request = None;
        Ok(response)
    }

    fn reply<T>(&mut self, read: impl FnOnce(&[u8]) -> Result<T>) -> Result<T> {
        let ptr: u32 = self.call("guest_reply_ptr", ())?;
        let len: u32 = self.call("guest_reply_len", ())?;
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| anyhow!("missing memory"))?;
        let bytes = memory
            .data(&self.store)
            .get(ptr as usize..ptr as usize + len as usize)
            .ok_or_else(|| WorkerError::new("invalid_response"))?;
        read(bytes)
    }

    pub fn shutdown(&mut self, grace_ms: u64) -> (bool, Option<String>) {
        self.store.data_mut().deadline =
            Instant::now() + Duration::from_millis(grace_ms.min(1000).saturating_sub(15));
        self.store.set_epoch_deadline(1);
        match self.call::<(), u32>("guest_shutdown", ()) {
            Ok(6) => (false, None),
            Ok(0) => (true, None),
            _ => (true, Some("shutdown_failed".into())),
        }
    }

    pub fn spin(&mut self) -> Result<()> {
        self.call("guest_spin", ())
    }

    fn call<P: wasmtime::WasmParams, R: wasmtime::WasmResults>(
        &mut self,
        export: &str,
        params: P,
    ) -> Result<R> {
        let function = self
            .instance
            .get_typed_func::<P, R>(&mut self.store, export)?;
        function.call(&mut self.store, params).map_err(|error| {
            if error.downcast_ref::<Trap>() == Some(&Trap::StackOverflow) {
                WorkerError::new("worker_execution_failed").into()
            } else {
                error
            }
        })
    }

    fn copy_into_guest(&mut self, bytes: &[u8]) -> Result<u32> {
        let ptr: u32 = self.call("guest_alloc", u32::try_from(bytes.len())?)?;
        if ptr == 0 {
            return Err(WorkerError::new("worker_memory_limit").into());
        }
        let memory = self
            .instance
            .get_memory(&mut self.store, "memory")
            .ok_or_else(|| anyhow!("missing memory"))?;
        memory.write(&mut self.store, ptr as usize, bytes)?;
        Ok(ptr)
    }
}

fn heap_limit(limits: &WorkerLimits) -> u32 {
    limits.heap_bytes.min(u32::MAX as usize) as u32
}

pub fn store_limits(heap: usize) -> wasmtime::StoreLimits {
    StoreLimitsBuilder::new()
        .memory_size(heap)
        .instances(1)
        .memories(1)
        .tables(1)
        .build()
}

fn check_guest_code(code: u32) -> Result<()> {
    let reason = match code {
        0 => return Ok(()),
        2 => "function_import_forbidden",
        3 => "worker_memory_limit",
        4 => "function_handle_missing",
        5 => "invalid_response",
        _ => "worker_execution_failed",
    };
    Err(WorkerError::new(reason).into())
}
