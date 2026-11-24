mod streaming;

use anyhow::{Context, Result, bail, ensure};
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use serde::Serialize;
use sha2::{Digest, Sha256};
use spidermonkey_embedding_splicer::{
    splice::splice_bindings,
    stub_wasi::stub_wasi,
    wit::exports::local::spidermonkey_embedding_splicer::splicer::{CoreTy, Feature},
};
use std::{
    env, fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use wasmtime::{Caller, Engine, Linker, Module, Store};
use wit_component::ComponentEncoder;

const ABI: &str = "konae-functions-wasm/1";
const INPUT_LIMIT: u64 = 64 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 64 * 1024 * 1024;
const SCRATCH_LIMIT: u64 = 256 * 1024 * 1024;
const WIZER_MEMORY_LIMIT: u64 = 1536 * 1024 * 1024;
const STDOUT_LIMIT: usize = 8 * 1024;
const STDERR_LIMIT: usize = 4 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);
const ENGINE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/engine.wasm"));
const ADAPTER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wasi-reactor.wasm"));
const WIT: &str = include_str!(concat!(env!("OUT_DIR"), "/starling.wit"));
const WIZER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wizer"));

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Identity<'a> {
    abi: &'a str,
    engine_sha256: &'a str,
    builder_fingerprint: &'a str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest<'a> {
    abi: &'a str,
    input_sha256: String,
    component_sha256: String,
}
#[derive(Serialize)]
#[serde(untagged)]
enum Reply<'a> {
    Ok {
        status: &'a str,
        manifest: Manifest<'a>,
    },
    Refusal {
        status: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic: Option<String>,
    },
}

fn main() {
    let result = run(env::args().skip(1).collect());
    if let Err(error) = result {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<()> {
    match args.as_slice() {
        [identity] if identity == "identity" => {
            println!(
                "{}",
                serde_json::to_string(&Identity {
                    abi: ABI,
                    engine_sha256: env!("KONAE_ENGINE_SHA256"),
                    builder_fingerprint: env!("KONAE_BUILDER_FINGERPRINT")
                })?
            );
        }
        [prepare, input_flag, input, output_flag, output]
            if prepare == "prepare" && input_flag == "--input" && output_flag == "--output" =>
        {
            let (status, manifest, diagnostic) =
                prepare_bundle(Path::new(input), Path::new(output))?;
            if let Some(manifest) = manifest {
                println!(
                    "{}",
                    serde_json::to_string(&Reply::Ok { status, manifest })?
                );
            } else {
                println!(
                    "{}",
                    serde_json::to_string(&Reply::Refusal { status, diagnostic })?
                );
            }
        }
        _ => bail!(
            "Usage: konae-functions-prepare identity | prepare --input <path> --output <path>"
        ),
    }
    Ok(())
}

fn prepare_bundle(
    input: &Path,
    output: &Path,
) -> Result<(&'static str, Option<Manifest<'static>>, Option<String>)> {
    macro_rules! write_scratch {
        ($root:expr, $path:expr, $bytes:expr) => {
            if !write_scratch_file($root, $path, $bytes, SCRATCH_LIMIT)? {
                return Ok((
                    "build_limit",
                    None,
                    Some("preparation scratch space exceeds the configured byte limit".into()),
                ));
            }
        };
    }
    if output.exists() {
        fs::remove_file(output).context("remove stale output")?;
    }
    if !input.is_absolute() || !output.is_absolute() {
        bail!("input and output paths must be absolute");
    }
    let file = fs::File::open(input).with_context(|| format!("read input {}", input.display()))?;
    if file.metadata()?.len() > INPUT_LIMIT {
        return Ok((
            "build_limit",
            None,
            Some("input exceeds the configured byte limit".into()),
        ));
    }
    let mut bytes = Vec::new();
    file.take(INPUT_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > INPUT_LIMIT {
        return Ok((
            "build_limit",
            None,
            Some("input exceeds the configured byte limit".into()),
        ));
    }
    let input_sha256 = hex(Sha256::digest(&bytes).as_slice());
    let scratch = tempfile::Builder::new()
        .prefix("konae-prepare-")
        .tempdir()?;
    let dir = scratch.path();
    let Some(source) = std::str::from_utf8(&bytes).ok().and_then(guard_imports) else {
        return Ok(("bundle_contract", None, None));
    };
    let app = dir.join("app.mjs");
    let adapter_file = dir.join("adapter.mjs");
    let entry = dir.join("source.js");
    let initializer = dir.join("initializer.js");
    let module = dir.join("input.wasm");
    let snapshot = dir.join("snapshot.wasm");
    write_scratch!(dir, &app, source.as_bytes());
    write_scratch!(
        dir,
        &adapter_file,
        include_bytes!(concat!(env!("OUT_DIR"), "/adapter.mjs"))
    );
    write_scratch!(
        dir,
        &dir.join("web.mjs"),
        include_bytes!(concat!(env!("OUT_DIR"), "/web.mjs"))
    );
    write_scratch!(
        dir,
        &entry,
        b"import { hostRuntime, initializationFailed } from './adapter.mjs';\nimport * as app from './app.mjs';\nif (typeof app.handle !== 'function') throw new Error('KONAE_BUNDLE_CONTRACT');\nif (initializationFailed()) throw new Error('function.wasm-init-effect: A function cannot start timers or fetch during top-level initialization.');\nexport const { handle, hasShutdown, shutdown, spin } = hostRuntime(app);\n"
    );

    let spliced = splice_bindings(
        ENGINE.to_vec(),
        vec![],
        Some(WIT.to_owned()),
        None,
        None,
        false,
    )
    .map_err(|error| anyhow::anyhow!("splicing failed: {error}"))?;
    write_scratch!(dir, &module, &spliced.wasm);
    write_scratch!(dir, &initializer, spliced.js_bindings.as_bytes());
    let wizer_path = dir.join("wizer");
    write_scratch!(dir, &wizer_path, WIZER);
    if scratch_exceeds(dir, SCRATCH_LIMIT)? {
        return Ok((
            "build_limit",
            None,
            Some("preparation scratch space exceeds the configured byte limit".into()),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wizer_path, fs::Permissions::from_mode(0o700))?;
    }
    let mut command = Command::new(&wizer_path);
    command
        .env_clear()
        .env("SOURCE_NAME", "source.js")
        .env("DEBUG", "")
        .env("FEATURE_CLOCKS", "1")
        .env("EXPORT_CNT", spliced.exports.len().to_string())
        .env("IMPORT_CNT", spliced.imports.len().to_string());
    for (i, (name, signature)) in spliced.exports.iter().enumerate() {
        let args = signature
            .params
            .iter()
            .map(core_ty)
            .collect::<Vec<_>>()
            .join(",");
        let ret = signature.ret.as_ref().map(core_ty).unwrap_or("");
        command
            .env(format!("EXPORT{i}_NAME"), name)
            .env(
                format!("EXPORT{i}_ARGS"),
                format!("{}{args}", if signature.paramptr { "*" } else { "" }),
            )
            .env(
                format!("EXPORT{i}_RET"),
                format!("{}{ret}", if signature.retptr { "*" } else { "" }),
            )
            .env(format!("EXPORT{i}_RETSIZE"), signature.retsize.to_string());
    }
    for (i, (_, name, argc)) in spliced.imports.iter().enumerate() {
        command
            .env(format!("IMPORT{i}_NAME"), name)
            .env(format!("IMPORT{i}_ARGCNT"), argc.to_string());
    }
    command
        .args([
            "--allow-wasi",
            "--init-func",
            "componentize.wizer",
            "--wasm-bulk-memory=true",
            "--inherit-env=true",
        ])
        .arg(format!("--dir={}", dir.display()))
        .arg(format!("--mapdir=/::{}", dir.display()))
        .arg("-o")
        .arg(&snapshot)
        .arg(&module)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(apply_child_limits);
        }
    }
    let start = Instant::now();
    let mut child = command.spawn().context("start Wizer")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out_reader = thread::spawn(move || read_bounded(stdout, STDOUT_LIMIT));
    let err_reader = thread::spawn(move || read_bounded(stderr, STDERR_LIMIT));
    use std::io::Write;
    write!(
        child.stdin.take().unwrap(),
        "--initializer-script-path {} --strip-path-prefix {}/ source.js",
        initializer.display(),
        dir.display()
    )?;
    loop {
        if let Some(limit) = exceeded_limit(child.id(), dir, SCRATCH_LIMIT, WIZER_MEMORY_LIMIT)? {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Ok(("build_limit", None, Some(limit.to_owned())));
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_reader.join();
            let _ = err_reader.join();
            let status = if source.contains("await") {
                "init_pending"
            } else {
                "build_limit"
            };
            return Ok((
                status,
                None,
                Some("Wizer exceeded its preparation deadline".into()),
            ));
        }
        if let Some(status) = child.try_wait()? {
            let (_stdout, stdout_overflow) = out_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stdout reader failed"))??;
            let (stderr, stderr_overflow) = err_reader
                .join()
                .map_err(|_| anyhow::anyhow!("stderr reader failed"))??;
            if stdout_overflow || stderr_overflow {
                return Ok((
                    "build_limit",
                    None,
                    Some("Wizer diagnostics exceeded the configured byte limit".into()),
                ));
            }
            if !status.success() {
                let detail = String::from_utf8_lossy(&stderr)
                    .chars()
                    .take(1024)
                    .collect::<String>();
                let refusal = if detail.contains("KONAE_INIT_EFFECT")
                    || detail.contains("function.wasm-init-effect:")
                    || detail.contains("function.initialization-cannot-snapshot:")
                    || [
                        "fetch",
                        "setTimeout",
                        "clearTimeout",
                        "setInterval",
                        "clearInterval",
                    ]
                    .iter()
                    .any(|name| {
                        detail.contains(&format!("{name} is not available in function handlers"))
                    }) {
                    "init_effect"
                } else if detail.contains("WebAssembly is not defined")
                    || detail.contains("KONAE_UNSUPPORTED_API")
                {
                    "unsupported_api"
                } else if detail.contains("pending") || detail.contains("unsettled") {
                    "init_pending"
                } else if detail.contains("KONAE_BUNDLE_CONTRACT") {
                    "bundle_contract"
                } else {
                    "init_failed"
                };
                return Ok((refusal, None, Some(detail)));
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if scratch_exceeds(dir, SCRATCH_LIMIT)? {
        return Ok((
            "build_limit",
            None,
            Some("preparation scratch space exceeds the configured byte limit".into()),
        ));
    }
    let snapshot_bytes = fs::read(&snapshot).context("Wizer produced no snapshot")?;
    let check_status = check_init(&snapshot_bytes)?;
    match check_status {
        1 => {
            return Ok((
                "init_pending",
                None,
                Some("Initialization did not settle".into()),
            ));
        }
        _ => (),
    }
    let core = stub_wasi(
        snapshot_bytes,
        vec![Feature::Clocks, Feature::Random],
        Some(WIT.to_owned()),
        None,
        None,
    )
    .map_err(|error| anyhow::anyhow!("WASI adapter failed: {error}"))?;
    let core = streaming::forward_random(&core)?;
    let core = streaming::bound_allocations(&core)?;
    let component = ComponentEncoder::default()
        .merge_imports_based_on_semver(false)
        .validate(true)
        .module(&core)?
        .adapter("wasi_snapshot_preview1", ADAPTER)?
        .encode()?;
    ensure!(
        component.len() as u64 <= OUTPUT_LIMIT,
        "component exceeds 64 MiB"
    );
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &component)?;
    let manifest = Manifest {
        abi: ABI,
        input_sha256,
        component_sha256: hex(Sha256::digest(&component).as_slice()),
    };
    Ok(("ok", Some(manifest), None))
}

fn guard_imports(source: &str) -> Option<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();
    if !parsed.errors.is_empty() || !parsed.module_record.requested_modules.is_empty() {
        return None;
    }

    // Preserve evaluation of the specifier while refusing to load a module.
    let mut offsets = parsed
        .module_record
        .dynamic_imports
        .iter()
        .map(|import| import.span.start as usize)
        .collect::<Vec<_>>();
    offsets.sort_unstable();
    let mut guarded = source.to_owned();
    for offset in offsets.into_iter().rev() {
        guarded.replace_range(
            offset..offset + "import".len(),
            "((..._) => Promise.reject(new TypeError('function_import_forbidden')))",
        );
    }
    Some(guarded)
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let available = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..count.min(available)]);
        overflow |= count > available;
    }
    Ok((bytes, overflow))
}

fn scratch_usage(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() {
            total = total.saturating_add(scratch_usage(&entry.path())?);
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn scratch_exceeds(path: &Path, limit: u64) -> Result<bool> {
    Ok(scratch_usage(path)? > limit)
}

fn write_scratch_file(root: &Path, path: &Path, bytes: &[u8], limit: u64) -> Result<bool> {
    let current = scratch_usage(root)?;
    if current.saturating_add(bytes.len() as u64) > limit {
        return Ok(false);
    }
    fs::write(path, bytes)?;
    Ok(true)
}

fn exceeded_limit(
    pid: u32,
    scratch: &Path,
    scratch_limit: u64,
    memory_limit: u64,
) -> Result<Option<&'static str>> {
    if scratch_exceeds(scratch, scratch_limit)? {
        return Ok(Some(
            "preparation scratch space exceeds the configured byte limit",
        ));
    }
    #[cfg(target_os = "macos")]
    if process_memory_usage(pid)?.is_some_and(|usage| usage > memory_limit) {
        return Ok(Some("Wizer exceeded the configured memory limit"));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (pid, memory_limit);
    Ok(None)
}

#[cfg(unix)]
fn apply_child_limits() -> std::io::Result<()> {
    let output = libc::rlimit {
        rlim_cur: OUTPUT_LIMIT,
        rlim_max: OUTPUT_LIMIT,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &output) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[cfg(target_os = "linux")]
    {
        let memory = libc::rlimit {
            rlim_cur: WIZER_MEMORY_LIMIT,
            rlim_max: WIZER_MEMORY_LIMIT,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_AS, &memory) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn process_memory_usage(pid: u32) -> Result<Option<u64>> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    let result = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error.into());
    }
    Ok(Some(unsafe { info.assume_init() }.ri_phys_footprint))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn core_ty(ty: &CoreTy) -> &'static str {
    match ty {
        CoreTy::I32 => "i32",
        CoreTy::I64 => "i64",
        CoreTy::F32 => "f32",
        CoreTy::F64 => "f64",
    }
}

fn check_init(snapshot: &[u8]) -> Result<i32> {
    let mut config = wasmtime::Config::new();
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let timeout_engine = engine.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(5));
        timeout_engine.increment_epoch();
    });
    let module = Module::new(&engine, snapshot)?;
    let mut linker = Linker::new(&engine);
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "fd_write",
        |mut caller: Caller<'_, ()>,
         fd: i32,
         _iovs: u32,
         _count: u32,
         written: u32|
         -> Result<i32> {
            let memory = caller
                .get_export("memory")
                .and_then(wasmtime::Extern::into_memory)
                .context("Wizer memory")?;
            memory.write(&mut caller, written as usize, &0u32.to_le_bytes())?;
            Ok(if fd == 2 { 1 } else { 0 })
        },
    )?;
    linker.define_unknown_imports_as_traps(&module)?;
    let mut store = Store::new(&engine, ());
    store.set_epoch_deadline(1);
    let guest = linker.instantiate(&mut store, &module)?;
    let status = guest
        .get_typed_func::<(), i32>(&mut store, "check_init")?
        .call(&mut store, ())?;
    Ok(if status == 0 { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    #[test]
    fn scratch_limit_counts_files_across_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("one"), [0u8; 7]).unwrap();
        fs::write(dir.path().join("two"), [0u8; 6]).unwrap();
        assert_eq!(scratch_usage(dir.path()).unwrap(), 13);
        assert!(scratch_exceeds(dir.path(), 12).unwrap());
        let third = dir.path().join("three");
        assert!(!write_scratch_file(dir.path(), &third, &[0u8; 1], 13).unwrap());
        assert!(!third.exists());
    }

    #[test]
    fn child_address_space_limit_is_applied() {
        #[cfg(target_os = "macos")]
        {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", "sleep 5"]);
            unsafe {
                command.pre_exec(apply_child_limits);
            }
            let mut child = command.spawn().unwrap();
            let scratch = tempfile::tempdir().unwrap();
            assert_eq!(
                exceeded_limit(child.id(), scratch.path(), u64::MAX, 1).unwrap(),
                Some("Wizer exceeded the configured memory limit")
            );
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(target_os = "linux")]
        {
            let mut command = std::process::Command::new("/bin/sh");
            command.args(["-c", "ulimit -v"]);
            unsafe {
                command.pre_exec(apply_child_limits);
            }
            let output = command.output().unwrap();
            assert!(output.status.success());
            let kibibytes: u64 = String::from_utf8(output.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert_eq!(kibibytes, WIZER_MEMORY_LIMIT / 1024);
        }
    }
}
