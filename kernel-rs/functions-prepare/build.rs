use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, process::Command};

fn main() -> Result<()> {
    println!("cargo:rerun-if-env-changed=KONAE_STARLING_TOOLS");
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let guest = root.join("../functions-runner-wasm/guest");
    let tools = env::var_os("KONAE_STARLING_TOOLS").map(PathBuf::from);
    let tools =
        tools.context("Set KONAE_STARLING_TOOLS to the pinned ComponentizeJS 0.21.0 assets")?;
    let package = tools.join("node_modules/@bytecodealliance/componentize-js");
    let package_json: serde_json::Value =
        serde_json::from_slice(&fs::read(package.join("package.json"))?)?;
    if package_json["version"].as_str() != Some("0.21.0") {
        bail!("KONAE_STARLING_TOOLS must contain ComponentizeJS 0.21.0");
    }
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let engine = package.join("lib/starlingmonkey_embedding.wasm");
    let adapter =
        tools.join("node_modules/@bytecodealliance/jco/lib/wasi_snapshot_preview1.reactor.wasm");
    ensure_sha(
        &engine,
        "b45eb4dda2fed0eaecd4e5581fb26a99e642d001dbf82f3698aba27564a78ea1",
    )?;
    ensure_sha(
        &adapter,
        "b857310c4753c8fafaa0f890e9ccf978dc3d9dbe8804b1125291a627485041d0",
    )?;
    check_package(
        &tools.join("node_modules/@bytecodealliance/jco/package.json"),
        "1.42.0",
    )?;
    check_package(&tools.join("node_modules/esbuild/package.json"), "0.25.12")?;
    copy(&engine, &out.join("engine.wasm"))?;
    copy(&adapter, &out.join("wasi-reactor.wasm"))?;
    copy(&guest.join("starling.wit"), &out.join("starling.wit"))?;
    let target = env::var("TARGET")?;
    let platform = if target.contains("apple-darwin") {
        "darwin"
    } else if target.contains("linux") {
        "linux"
    } else {
        bail!("Unsupported Wizer target {target}")
    };
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else if target.starts_with("x86_64") {
        "x64"
    } else {
        bail!("Unsupported Wizer target {target}")
    };
    let wizer = tools.join(format!(
        "node_modules/@bytecodealliance/wizer-{platform}-{arch}/wizer"
    ));
    let version = Command::new(&wizer).arg("--version").output()?;
    let version = String::from_utf8_lossy(&version.stdout);
    if !version.contains("10.0.0") {
        bail!("Expected Wizer 10.0.0, found {}", version.trim());
    }
    copy(&wizer, &out.join("wizer"))?;
    let adapter_source = guest.join("starling-runtime.js");
    let esbuild = tools.join("node_modules/.bin/esbuild");
    let outfile = format!("--outfile={}", out.join("adapter.mjs").display());
    let status = Command::new(&esbuild)
        .args([
            adapter_source.to_str().unwrap(),
            "--bundle",
            "--format=esm",
            "--platform=neutral",
            "--external:konae:worker/host",
            &outfile,
        ])
        .status()?;
    if !status.success() {
        bail!("Could not bundle the pinned StarlingMonkey adapter");
    }
    let adapter_bundle = out.join("adapter.mjs");
    let mut adapter_js = fs::read_to_string(&adapter_bundle)?;
    anyhow::ensure!(
        adapter_js.contains("__KONAE_BUILD_TIMESTAMP__"),
        "StarlingMonkey runtime adapter has no build clock"
    );
    adapter_js = adapter_js.replace("__KONAE_BUILD_TIMESTAMP__", "0");
    let web_bundle = out.join("web.mjs");
    let web_outfile = format!("--outfile={}", web_bundle.display());
    let web_status = Command::new(&esbuild)
        .env("NODE_PATH", tools.join("node_modules"))
        .args([
            guest.join("web.js").to_str().unwrap(),
            "--bundle",
            "--format=esm",
            "--platform=neutral",
            "--main-fields=module,main",
            &web_outfile,
        ])
        .status()?;
    if !web_status.success() {
        bail!("Could not bundle the pinned Web API adapter");
    }
    adapter_js.insert_str(0, "import * as web from './web.mjs';\n");
    adapter_js.push_str("\nObject.assign(globalThis, { AbortController: web.AbortController, AbortSignal: web.AbortSignal, Blob: web.Blob, DOMException: web.DOMException, File: web.File, FormData: web.FormData, Headers: web.Headers, ReadableStream: web.ReadableStream, Request: web.Request, Response: web.Response });\n");
    fs::write(&adapter_bundle, adapter_js)?;
    for name in [
        "starling-entry.js",
        "starling-runtime.js",
        "dispatch.js",
        "web.js",
        "text.js",
        "abort.js",
    ] {
        println!("cargo:rerun-if-changed={}", guest.join(name).display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        guest.join("starling.wit").display()
    );
    println!("cargo:rerun-if-changed={}", engine.display());
    println!("cargo:rerun-if-changed={}", wizer.display());
    println!("cargo:rustc-env=KONAE_ENGINE_SHA256={}", digest(&engine)?);
    println!(
        "cargo:rustc-env=KONAE_BUILDER_FINGERPRINT={}",
        fingerprint(&root, &guest, &engine, &adapter_bundle, &web_bundle)?
    );
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=src/streaming.rs");
    println!("cargo:rerun-if-changed={}", adapter.display());
    Ok(())
}

fn copy(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    fs::copy(from, to).with_context(|| format!("copy pinned asset {}", from.display()))?;
    Ok(())
}

fn digest(path: &std::path::Path) -> Result<String> {
    let hash = Sha256::digest(fs::read(path)?);
    Ok(hash.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn ensure_sha(path: &std::path::Path, expected: &str) -> Result<()> {
    let actual = digest(path)?;
    if actual != expected {
        bail!("Pinned asset digest mismatch for {}", path.display());
    }
    Ok(())
}

fn check_package(path: &std::path::Path, version: &str) -> Result<()> {
    let package: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    if package["version"].as_str() != Some(version) {
        bail!(
            "Expected pinned package version {version} at {}",
            path.display()
        );
    }
    Ok(())
}

fn fingerprint(
    root: &std::path::Path,
    guest: &std::path::Path,
    engine: &std::path::Path,
    adapter_bundle: &std::path::Path,
    web_bundle: &std::path::Path,
) -> Result<String> {
    let mut hash = Sha256::new();
    let target = env::var("TARGET").unwrap_or_default();
    let platform = if target.contains("apple-darwin") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if target.starts_with("aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let wizer = env::var_os("KONAE_STARLING_TOOLS")
        .map(PathBuf::from)
        .context("Set KONAE_STARLING_TOOLS")?
        .join(format!(
            "node_modules/@bytecodealliance/wizer-{platform}-{arch}/wizer"
        ));
    let adapter = env::var_os("KONAE_STARLING_TOOLS")
        .map(PathBuf::from)
        .context("Set KONAE_STARLING_TOOLS")?
        .join("node_modules/@bytecodealliance/jco/lib/wasi_snapshot_preview1.reactor.wasm");
    for path in [
        engine.to_path_buf(),
        adapter_bundle.to_path_buf(),
        web_bundle.to_path_buf(),
        wizer,
        adapter,
        guest.join("starling.wit"),
        guest.join("starling-entry.js"),
        guest.join("starling-runtime.js"),
        guest.join("dispatch.js"),
        guest.join("web.js"),
        guest.join("text.js"),
        guest.join("abort.js"),
        root.join("src/main.rs"),
        root.join("src/streaming.rs"),
        root.join("build.rs"),
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("../functions-runner-wasm/package-lock.json"),
        root.join("vendor/spidermonkey-embedding-splicer/src/splice.rs"),
        root.join("vendor/spidermonkey-embedding-splicer/src/stub_wasi.rs"),
        root.join("vendor/spidermonkey-embedding-splicer/src/bindgen.rs"),
        root.join("vendor/spidermonkey-embedding-splicer/src/wit.rs"),
        root.join("vendor/spidermonkey-embedding-splicer/src/lib.rs"),
    ] {
        hash.update(path.file_name().unwrap().to_string_lossy().as_bytes());
        hash.update((fs::metadata(&path)?.len() as u64).to_le_bytes());
        hash.update(fs::read(path)?);
    }
    hash.update(
        b"ComponentizeJS/0.21.0;Wizer/10.0.0;Wasmtime/38.0.4;init-clock/0;semver-merge/off",
    );
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
