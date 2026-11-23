#[cfg(feature = "quickjs-runner")]
#[path = "src/engine_config.rs"]
mod engine_config;

#[cfg(feature = "quickjs-runner")]
use anyhow::{Context, Result, ensure};
#[cfg(feature = "quickjs-runner")]
use std::{fs, path::Path};
#[cfg(feature = "quickjs-runner")]
use wasmtime::{Caller, Linker, Module, Store};

#[cfg(feature = "starling-runner")]
fn main() -> anyhow::Result<()> {
    println!(
        "cargo:rustc-env=KONAE_STARLING_TARGET={}",
        std::env::var("TARGET")?
    );
    Ok(())
}

#[cfg(feature = "quickjs-runner")]
fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=guest/out/quickjs.wasm");
    println!("cargo:rerun-if-changed=guest/out/bootstrap.js");
    println!("cargo:rerun-if-changed=src/engine_config.rs");
    ensure!(
        std::env::var("HOST")? == std::env::var("TARGET")?,
        "Build the spike on the target architecture"
    );

    let engine = wasmtime::Engine::new(&engine_config::config())?;
    let wasm =
        fs::read("guest/out/quickjs.wasm").context("Build the guest with guest/build.sh first")?;
    let module = Module::new(&engine, wasm)?;
    let bytes = fs::read("guest/out/bootstrap.js")?;
    let bootstrap = pack_bootstrap(&module, &bytes)?;
    let out_dir = std::env::var("OUT_DIR")?;
    let out = Path::new(&out_dir);
    fs::write(out.join("quickjs.cwasm"), module.serialize()?)?;
    fs::write(out.join("bootstrap.qbc"), bootstrap)?;
    Ok(())
}

#[cfg(feature = "quickjs-runner")]
fn pack_bootstrap(module: &Module, source: &[u8]) -> Result<Vec<u8>> {
    let mut linker = Linker::new(module.engine());
    linker.func_wrap(
        "wasi_snapshot_preview1",
        "clock_time_get",
        |mut caller: Caller<'_, ()>, _clock: i32, _precision: i64, offset: u32| -> Result<i32> {
            let memory = caller
                .get_export("memory")
                .and_then(wasmtime::Extern::into_memory)
                .context("Missing guest memory")?;
            memory.write(&mut caller, offset as usize, &[0; 8])?;
            Ok(0)
        },
    )?;
    linker.define_unknown_imports_as_traps(module)?;

    let mut store = Store::new(module.engine(), ());
    store.set_epoch_deadline(1);
    let guest = linker.instantiate(&mut store, module)?;
    guest
        .get_typed_func::<(), ()>(&mut store, "_initialize")?
        .call(&mut store, ())?;
    let memory = guest
        .get_memory(&mut store, "memory")
        .context("Missing guest memory")?;
    let length = u32::try_from(source.len())?;
    let offset = guest
        .get_typed_func::<u32, u32>(&mut store, "guest_alloc")?
        .call(&mut store, length)?;
    ensure!(
        offset != 0,
        "Could not allocate the Web bootstrap in guest memory"
    );
    memory.write(&mut store, offset as usize, source)?;
    let code = guest
        .get_typed_func::<(u32, u32, u32, u32), u32>(&mut store, "guest_pack")?
        .call(&mut store, (offset, length, 64 << 20, 0))?;
    ensure!(code == 0, "Could not compile the Web bootstrap ({code})");
    let offset = guest
        .get_typed_func::<(), u32>(&mut store, "guest_reply_ptr")?
        .call(&mut store, ())?;
    let length = guest
        .get_typed_func::<(), u32>(&mut store, "guest_reply_len")?
        .call(&mut store, ())?;
    Ok(memory.data(&store)[offset as usize..offset as usize + length as usize].to_vec())
}
