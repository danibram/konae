use crate::bridge::Bridge;
use anyhow::{bail, ensure, Context, Result};
use konae_functions_executor::{
    protocol::{Input, Output, WorkerLimits, BODY_CHUNK_BYTES, MAX_OPERATION_BYTES},
    WorkerError,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    os::{
        fd::FromRawFd,
        unix::fs::{FileExt, MetadataExt},
    },
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, UNIX_EPOCH},
};
use wasmtime::{
    component::{
        Component, Instance, InstancePre, Linker, Resource, ResourceAny, ResourceType, TypedFunc,
        Val,
    },
    Engine, Store, UpdateDeadline,
};

const MAX_COMPONENT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RANDOM_BYTES: u64 = 1 << 20;
const ALLOWED_COMPONENT_IMPORTS: [&str; 16] = [
    "konae:worker/host",
    "konae:worker/crypto",
    "wasi:cli/stderr@0.2.10",
    "wasi:cli/stderr@0.2.12",
    "wasi:clocks/monotonic-clock@0.2.10",
    "wasi:clocks/monotonic-clock@0.2.12",
    "wasi:clocks/wall-clock@0.2.10",
    "wasi:clocks/wall-clock@0.2.12",
    "wasi:io/error@0.2.10",
    "wasi:io/error@0.2.12",
    "wasi:io/poll@0.2.10",
    "wasi:io/poll@0.2.12",
    "wasi:io/streams@0.2.10",
    "wasi:io/streams@0.2.12",
    "wasi:random/random@0.2.10",
    "wasi:random/random@0.2.12",
];
static TEST_RANDOM_SEED_INDEX: AtomicU64 = AtomicU64::new(0);

pub struct Compiler {
    pub engine: Engine,
}

impl Compiler {
    pub fn new() -> Result<Self> {
        let engine = Engine::new(&crate::engine_config::starling())?;
        Ok(Self { engine })
    }
}

fn link_worker_imports(linker: &mut Linker<Bridge>) -> Result<()> {
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
                "function.wasm-init-effect" => "function.wasm-init-effect",
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
        Ok((store.data().wall_ms()?,))
    })?;
    host.func_wrap("random-seed", |_store, ()| -> Result<(u64,)> {
        if let Some(seed) = std::env::var_os("KONAE_STARLING_TEST_SEED") {
            let seed = seed.to_string_lossy().parse::<u64>()?;
            return Ok(
                (seed.wrapping_add(TEST_RANDOM_SEED_INDEX.fetch_add(1, Ordering::Relaxed)),),
            );
        }
        let mut bytes = [0u8; 8];
        let result = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok((u64::from_le_bytes(bytes),))
    })?;
    link_request_web_imports(linker)?;
    crate::crypto::link(linker)
}

pub struct PreparedComponents {
    descriptors: BTreeMap<String, ComponentDescriptor>,
    loaded: Option<((u64, u64), InstancePre<Bridge>)>,
}

struct ComponentDescriptor {
    file: File,
    identity: (u64, u64),
    size: u64,
}

impl ComponentDescriptor {
    fn open(fd: u32) -> Result<Self> {
        let raw = i32::try_from(fd)?;
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        ensure!(
            flags >= 0 && flags & libc::O_ACCMODE == libc::O_RDONLY,
            "component descriptor is not read-only"
        );
        let file = unsafe { File::from_raw_fd(raw) };
        let metadata = file.metadata()?;
        let size = metadata.len();
        ensure!(
            metadata.is_file() && size > 0 && size <= MAX_COMPONENT_BYTES,
            "invalid prepared component descriptor"
        );
        Ok(Self {
            file,
            identity: (metadata.dev(), metadata.ino()),
            size,
        })
    }

    fn read(&self) -> Result<Vec<u8>> {
        let mut bytes = vec![0; self.size as usize];
        self.file.read_exact_at(&mut bytes, 0)?;
        ensure!(
            self.file.metadata()?.len() == self.size,
            "prepared component size changed"
        );
        Ok(bytes)
    }
}

impl PreparedComponents {
    pub fn open(components: BTreeMap<String, u32>) -> Result<Self> {
        let descriptors = components
            .into_iter()
            .map(|(name, fd)| Ok((name, ComponentDescriptor::open(fd)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            descriptors,
            loaded: None,
        })
    }

    pub fn load(&mut self, compiler: &Compiler, name: Option<&str>) -> Result<&InstancePre<Bridge>> {
        let descriptor = match name {
            Some(name) => self
                .descriptors
                .get(name)
                .ok_or_else(|| WorkerError::new("invalid_function_name"))?,
            None => self
                .descriptors
                .values()
                .next()
                .ok_or_else(|| WorkerError::new("invalid_init"))?,
        };
        if self
            .loaded
            .as_ref()
            .is_none_or(|(identity, _)| *identity != descriptor.identity)
        {
            // Drop the old image before allocating space to read its replacement.
            self.loaded = None;
            let bytes = descriptor.read()?;
            let component = Invocation::pack_prepared(compiler, &bytes)?;
            self.loaded = Some((descriptor.identity, component));
        }
        Ok(&self.loaded.as_ref().unwrap().1)
    }
}

pub struct Invocation {
    instance: Instance,
    store: Store<Bridge>,
}

impl Invocation {
    fn pack_prepared(compiler: &Compiler, bytes: &[u8]) -> Result<InstancePre<Bridge>> {
        // The Kernel verifies the native file in the operator's cache before passing its descriptor.
        let component = unsafe { Component::deserialize(&compiler.engine, bytes) }
            .context("Load the prepared StarlingMonkey component")?;
        let imports = component
            .component_type()
            .imports(&compiler.engine)
            .map(|(name, _)| name.to_owned())
            .collect::<BTreeSet<_>>();
        let unsupported = imports
            .iter()
            .filter(|name| !ALLOWED_COMPONENT_IMPORTS.contains(&name.as_str()))
            .collect::<Vec<_>>();
        ensure!(
            unsupported.is_empty(),
            "Prepared component imports unsupported host interfaces: {}",
            unsupported
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        ensure!(
            ["0.2.10", "0.2.12"].iter().any(|version| {
                imports.contains(&format!("wasi:clocks/monotonic-clock@{version}"))
            }) && ["0.2.10", "0.2.12"]
                .iter()
                .any(|version| { imports.contains(&format!("wasi:io/poll@{version}")) })
                && ["0.2.10", "0.2.12"]
                    .iter()
                    .any(|version| { imports.contains(&format!("wasi:random/random@{version}")) }),
            "Prepared component is missing request Web API imports"
        );
        let mut linker = Linker::<Bridge>::new(&compiler.engine);
        linker.allow_shadowing(true);
        linker.define_unknown_imports_as_traps(&component)?;
        link_worker_imports(&mut linker)?;
        linker.instantiate_pre(&component)
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
        self.store.data_mut().crypto.clear();
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
        self.store.data_mut().clear_pollables();
        self.store.data_mut().request = None;
        Ok(response)
    }

    pub fn shutdown(&mut self, grace_ms: u64) -> (bool, Option<String>) {
        self.store.data_mut().clear_pollables();
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

fn link_request_web_imports(linker: &mut Linker<Bridge>) -> Result<()> {
    link_request_web_imports_version(linker, "0.2.10")?;
    link_request_web_imports_version(linker, "0.2.12")
}

fn link_request_web_imports_version(linker: &mut Linker<Bridge>, version: &str) -> Result<()> {
    let mut clocks = linker.instance(&format!("wasi:clocks/monotonic-clock@{version}"))?;
    clocks.func_new("now", |store, _, results| {
        results[0] = Val::U64(store.data().monotonic_ns());
        Ok(())
    })?;
    clocks.func_new("resolution", |_store, _, results| {
        results[0] = Val::U64(1);
        Ok(())
    })?;
    clocks.func_new("subscribe-duration", |mut store, params, results| {
        let Val::U64(nanos) = params[0] else {
            bail!("invalid clock duration")
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_nanos(nanos))
            .ok_or_else(|| anyhow::anyhow!("timer duration overflow"))?;
        let id = store.data_mut().add_pollable(deadline)?;
        results[0] = Val::Resource(ResourceAny::try_from_resource(
            Resource::<()>::new_own(id),
            &mut store,
        )?);
        Ok(())
    })?;
    clocks.func_new("subscribe-instant", |mut store, params, results| {
        let Val::U64(when) = params[0] else {
            bail!("invalid clock instant")
        };
        let now = store.data().monotonic_ns();
        let wait = when.saturating_sub(now);
        let deadline = Instant::now()
            .checked_add(Duration::from_nanos(wait))
            .ok_or_else(|| anyhow::anyhow!("timer instant overflow"))?;
        let id = store.data_mut().add_pollable(deadline)?;
        results[0] = Val::Resource(ResourceAny::try_from_resource(
            Resource::<()>::new_own(id),
            &mut store,
        )?);
        Ok(())
    })?;

    let mut wall = linker.instance(&format!("wasi:clocks/wall-clock@{version}"))?;
    wall.func_new("now", |store, _, results| {
        let since_epoch = store.data().wall_time()?.duration_since(UNIX_EPOCH)?;
        results[0] = Val::Record(vec![
            ("seconds".into(), Val::U64(since_epoch.as_secs())),
            ("nanoseconds".into(), Val::U32(since_epoch.subsec_nanos())),
        ]);
        Ok(())
    })?;
    wall.func_new("resolution", |_store, _, results| {
        results[0] = Val::Record(vec![
            ("seconds".into(), Val::U64(0)),
            ("nanoseconds".into(), Val::U32(1)),
        ]);
        Ok(())
    })?;

    let mut random = linker.instance(&format!("wasi:random/random@{version}"))?;
    random.func_new("get-random-u64", |mut store, _, results| {
        require_live_request(&mut store)?;
        let mut bytes = [0u8; 8];
        fill_entropy(&mut bytes)?;
        results[0] = Val::U64(u64::from_le_bytes(bytes));
        Ok(())
    })?;
    random.func_new("get-random-bytes", |mut store, params, results| {
        require_live_request(&mut store)?;
        let Val::U64(length) = params[0] else {
            bail!("invalid random byte count")
        };
        if length > MAX_RANDOM_BYTES {
            bail!("random request exceeds its limit: {length}");
        }
        let mut bytes = vec![0; length as usize];
        fill_entropy(&mut bytes)?;
        results[0] = Val::List(bytes.into_iter().map(Val::U8).collect());
        Ok(())
    })?;

    let mut poll = linker.instance(&format!("wasi:io/poll@{version}"))?;
    poll.resource("pollable", ResourceType::host::<()>(), |mut store, rep| {
        store.data_mut().remove_pollable(rep);
        Ok(())
    })?;
    poll.func_wrap(
        "poll",
        |store, (resources,): (Vec<Resource<()>>,)| -> Result<(Vec<u32>,)> {
            ensure!(!resources.is_empty(), "cannot poll an empty list");
            loop {
                store.data().check()?;
                let now = Instant::now();
                let mut ready = Vec::new();
                let mut next_deadline = None;
                for (index, resource) in resources.iter().enumerate() {
                    let deadline = store.data().pollable_deadline(resource.rep())?;
                    if deadline <= now {
                        ready.push(index as u32);
                    } else {
                        next_deadline =
                            Some(next_deadline.map_or(deadline, |old: Instant| old.min(deadline)));
                    }
                }
                if !ready.is_empty() {
                    return Ok((ready,));
                }
                let delay = next_deadline
                    .map(|at| at.saturating_duration_since(Instant::now()))
                    .unwrap_or(Duration::from_millis(1))
                    .min(Duration::from_millis(1));
                std::thread::sleep(delay.max(Duration::from_micros(100)));
            }
        },
    )?;
    poll.func_wrap(
        "[method]pollable.ready",
        |store, (resource,): (Resource<()>,)| -> Result<(bool,)> {
            Ok((store.data().pollable_deadline(resource.rep())? <= Instant::now(),))
        },
    )?;
    poll.func_wrap(
        "[method]pollable.block",
        |store, (resource,): (Resource<()>,)| -> Result<()> {
            let deadline = store.data().pollable_deadline(resource.rep())?;
            loop {
                store.data().check()?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(());
                }
                std::thread::sleep(remaining.min(Duration::from_millis(1)));
            }
        },
    )?;
    Ok(())
}

fn require_live_request(store: &mut wasmtime::StoreContextMut<'_, Bridge>) -> Result<()> {
    store.data().check()?;
    ensure!(store.data().request.is_some(), "request capability expired");
    Ok(())
}

pub(crate) fn fill_entropy(output: &mut [u8]) -> Result<()> {
    for chunk in output.chunks_mut(256) {
        let result = unsafe { libc::getentropy(chunk.as_mut_ptr().cast(), chunk.len()) };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(())
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
