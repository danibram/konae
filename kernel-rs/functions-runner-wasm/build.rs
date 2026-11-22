#[path = "src/engine_config.rs"]
mod engine_config;

fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=guest/out/quickjs.wasm");
    println!("cargo:rerun-if-changed=guest/out/bootstrap.js");
    println!("cargo:rerun-if-changed=src/engine_config.rs");
    anyhow::ensure!(
        std::env::var("HOST")? == std::env::var("TARGET")?,
        "Build the spike on the target architecture"
    );
    let engine = wasmtime::Engine::new(&engine_config::config())?;
    let wasm = std::fs::read("guest/out/quickjs.wasm")
        .map_err(|error| anyhow::anyhow!("Build the guest with guest/build.sh first: {error}"))?;
    let compiled = engine.precompile_module(&wasm)?;
    std::fs::write(
        std::path::Path::new(&std::env::var("OUT_DIR")?).join("quickjs.cwasm"),
        compiled,
    )?;
    Ok(())
}
