use crate::bridge::Bridge;
use anyhow::{Context, Result, bail, ensure};
use konae_functions_executor::{
    WorkerError,
    protocol::{
        BODY_CHUNK_BYTES, Input, MAX_OPERATION_BYTES, MAX_SOURCE_BYTES, Output, WorkerLimits,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};
use wasmtime::{
    Engine, Store, UpdateDeadline,
    component::{
        Component, Instance, InstancePre, Linker, Resource, ResourceAny, ResourceType, TypedFunc,
        Val,
    },
};

const TARGET: &str = env!("KONAE_STARLING_TARGET");
const STARLING: &str = "0.21.0:b45eb4dda2fed0eaecd4e5581fb26a99e642d001dbf82f3698aba27564a78ea1";
const WASMTIME: &str = "38.0.4";
const BUILD_LIMIT: Duration = Duration::from_secs(60);
const MAX_RANDOM_BYTES: u64 = 1 << 20;
const ALLOWED_COMPONENT_IMPORTS: [&str; 8] = [
    "konae:worker/host",
    "wasi:cli/stderr@0.2.12",
    "wasi:clocks/monotonic-clock@0.2.12",
    "wasi:clocks/wall-clock@0.2.12",
    "wasi:io/error@0.2.12",
    "wasi:io/poll@0.2.12",
    "wasi:io/streams@0.2.12",
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
    Ok(())
}

pub struct Invocation {
    instance: Instance,
    store: Store<Bridge>,
}

impl Invocation {
    pub fn pack(
        compiler: &Compiler,
        _bridge: Bridge,
        _limits: &WorkerLimits,
        source: &str,
    ) -> Result<InstancePre<Bridge>> {
        let cache = Cache::new(source)?;
        let meta = fs::symlink_metadata(&cache.compiled)
            .context("Prepared StarlingMonkey component is missing")?;
        ensure!(
            meta.is_file() && trusted(&meta),
            "Unsafe component cache entry"
        );
        // Only accept native code from the operator-owned cache.
        let component = unsafe { Component::deserialize_file(&compiler.engine, &cache.compiled) }
            .context("Load the approved StarlingMonkey component")?;
        let imports = component
            .component_type()
            .imports(&compiler.engine)
            .map(|(name, _)| name.to_owned())
            .collect::<BTreeSet<_>>();
        ensure!(
            imports
                .iter()
                .all(|name| ALLOWED_COMPONENT_IMPORTS.contains(&name.as_str())),
            "Prepared component imports an unsupported host interface"
        );
        ensure!(
            imports.contains("wasi:clocks/monotonic-clock@0.2.12")
                && imports.contains("wasi:clocks/wall-clock@0.2.12")
                && imports.contains("wasi:io/poll@0.2.12")
                && imports.contains("wasi:random/random@0.2.12"),
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
    let mut clocks = linker.instance("wasi:clocks/monotonic-clock@0.2.12")?;
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

    let mut wall = linker.instance("wasi:clocks/wall-clock@0.2.12")?;
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

    let mut random = linker.instance("wasi:random/random@0.2.12")?;
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

    let mut poll = linker.instance("wasi:io/poll@0.2.12")?;
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

fn fill_entropy(output: &mut [u8]) -> Result<()> {
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

struct Cache {
    source_hash: String,
    portable: PathBuf,
    compiled: PathBuf,
}

impl Cache {
    fn new(source: &str) -> Result<Self> {
        let directory = PathBuf::from(
            std::env::var_os("KONAE_STARLING_CACHE")
                .context("Set KONAE_STARLING_CACHE to the operator-owned directory")?,
        );
        ensure!(
            directory.is_absolute(),
            "The component cache path must be absolute"
        );
        let meta = fs::symlink_metadata(&directory)?;
        ensure!(
            meta.is_dir() && trusted(&meta),
            "The component cache must be operator-owned"
        );
        let source_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
        let mut version = Sha256::new();
        for bytes in [
            STARLING.as_bytes(),
            WASMTIME.as_bytes(),
            include_bytes!("../guest/dispatch.js"),
            include_bytes!("../guest/starling-runtime.js"),
            include_bytes!("../guest/starling-entry.js"),
            include_bytes!("../guest/starling.wit"),
            include_bytes!("../guest/web.js"),
            include_bytes!("../guest/text.js"),
            include_bytes!("../guest/abort.js"),
            include_bytes!("../package-lock.json"),
            include_bytes!("../measure/starling-component.mjs"),
            include_bytes!("../measure/starling-component.sh"),
            include_bytes!("../measure/starling-wat.py"),
        ] {
            version.update(Sha256::digest(bytes));
        }
        let key = format!("{source_hash}.{:x}", version.finalize());
        Ok(Self {
            source_hash,
            portable: directory.join(format!("{key}.wasm")),
            compiled: directory.join(format!("{key}.{TARGET}.cwasm")),
        })
    }
}

fn trusted(meta: &fs::Metadata) -> bool {
    let uid = unsafe { libc::geteuid() };
    (meta.uid() == uid || meta.uid() == 0) && meta.mode() & 0o022 == 0
}

fn cached_component(source: &str, build_limit: Duration) -> Result<(Cache, Option<Duration>)> {
    let cache = Cache::new(source)?;
    if let Ok(meta) = fs::symlink_metadata(&cache.compiled) {
        ensure!(
            meta.is_file() && trusted(&meta),
            "Unsafe component cache entry"
        );
        return Ok((cache, None));
    }
    let builder = PathBuf::from(
        std::env::var_os("KONAE_STARLING_BUILDER")
            .context("Component cache miss. Prepare this bundle with the configured builder")?,
    );
    ensure!(
        builder.is_absolute(),
        "The component builder path must be absolute"
    );
    let start = Instant::now();
    let work = tempfile::tempdir_in(cache.compiled.parent().unwrap())?;
    let input = work.path().join("app.mjs");
    let portable = work.path().join("app.wasm");
    let compiled = work.path().join("app.cwasm");
    fs::write(&input, source)?;
    if let Ok(meta) = fs::symlink_metadata(&cache.portable) {
        ensure!(
            meta.is_file() && trusted(&meta),
            "Unsafe portable component cache entry"
        );
        fs::copy(&cache.portable, &portable)?;
    }
    let mut command = Command::new(builder);
    command.process_group(0);
    let mut child = command
        .args([
            input.as_os_str(),
            portable.as_os_str(),
            compiled.as_os_str(),
        ])
        .args([STARLING, TARGET])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= build_limit {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            child.wait()?;
            bail!("StarlingMonkey component build timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    if status.code() == Some(65) {
        return Err(WorkerError::new("function_import_forbidden").into());
    }
    if status.code() == Some(66) {
        return Err(WorkerError::new("function.wasm-init-effect").into());
    }
    ensure!(
        status.success(),
        "StarlingMonkey builder exited with {status}"
    );
    fs::rename(portable, &cache.portable)?;
    fs::rename(compiled, &cache.compiled)?;
    Ok((cache, Some(start.elapsed())))
}

pub fn prepare(path: &Path) -> Result<()> {
    prepare_with_limit(path, BUILD_LIMIT)
}

fn prepare_with_limit(path: &Path, build_limit: Duration) -> Result<()> {
    ensure!(
        path.metadata()?.len() <= MAX_SOURCE_BYTES as u64,
        "Bundle is too large"
    );
    let source = fs::read_to_string(path)?;
    ensure!(source.len() <= MAX_SOURCE_BYTES, "Bundle is too large");
    let (cache, built) = cached_component(&source, build_limit)?;
    println!(
        "{}",
        json!({
            "source_sha256": cache.source_hash,
            "build_ms": built.map(|duration| duration.as_secs_f64() * 1000.0),
            "portable": cache.portable,
            "portable_bytes": cache.portable.metadata()?.len(),
            "compiled": cache.compiled,
            "compiled_bytes": cache.compiled.metadata()?.len(),
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonterminating_module_is_stopped_during_preparation() {
        let bundle = tempfile::tempdir().unwrap();
        let cache = bundle.path().join("cache");
        fs::create_dir(&cache).unwrap();
        let previous_cache = std::env::var_os("KONAE_STARLING_CACHE");
        unsafe { std::env::set_var("KONAE_STARLING_CACHE", &cache) };
        let source = bundle.path().join("loop.mjs");
        fs::write(
            &source,
            "while (true) {} export function handle() { return new Response('unreachable'); }",
        )
        .unwrap();
        let result = prepare_with_limit(&source, Duration::from_secs(15));
        assert!(result.is_err());
        assert!(fs::read_dir(cache).unwrap().next().is_none());
        if let Some(previous_cache) = previous_cache {
            unsafe { std::env::set_var("KONAE_STARLING_CACHE", previous_cache) };
        } else {
            unsafe { std::env::remove_var("KONAE_STARLING_CACHE") };
        }
    }
}
