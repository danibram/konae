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
    let os = env::var("CARGO_CFG_TARGET_OS")?;
    let arch = env::var("CARGO_CFG_TARGET_ARCH")?;
    let (wizer_package, wizer_sha256) = match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => (
            "wizer-darwin-arm64",
            "22e35af0887a351ef8a702fd10630abf056ce8387d9580461ea30d72adf50ee7",
        ),
        ("macos", "x86_64") => (
            "wizer-darwin-x64",
            "d7c6868e7318df271cc53fe0fde5706b7e346b346be10be6e453d5890e941dab",
        ),
        ("linux", "aarch64") => (
            "wizer-linux-arm64",
            "c0b1f9acc56d8536259cfc5398416c3947ddb572b3535e3dcc8ccd6703694d18",
        ),
        ("linux", "x86_64") => (
            "wizer-linux-x64",
            "bd74fcd8677c5c511f8a7b053965d774bf0534109638a8e1b015d6190d26cb6e",
        ),
        _ => bail!("Unsupported Wizer target {target}"),
    };
    let wizer = tools.join(format!(
        "node_modules/@bytecodealliance/{wizer_package}/wizer"
    ));
    ensure_sha(&wizer, wizer_sha256)?;
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
            "--external:konae:worker/crypto",
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
    adapter_js.push_str("\nrestrictAbortSignalTimeout(web.AbortSignal);\nObject.assign(globalThis, { AbortController: web.AbortController, AbortSignal: web.AbortSignal, Blob: web.Blob, DOMException: web.DOMException, File: web.File, FormData: web.FormData, Headers: web.Headers, ReadableStream: web.ReadableStream, Request: web.Request, Response: web.Response });\n");
    fs::write(&adapter_bundle, adapter_js)?;
    for name in [
        "starling-entry.js",
        "starling-runtime.js",
        "starling-crypto.js",
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
        fingerprint(
            &root,
            &guest,
            [&engine, &adapter_bundle, &web_bundle, &wizer, &adapter]
        )?
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
    assets: [&std::path::Path; 5],
) -> Result<String> {
    let mut hash = Sha256::new();
    let sources = [
        guest.join("starling.wit"),
        guest.join("starling-entry.js"),
        guest.join("starling-runtime.js"),
        guest.join("starling-crypto.js"),
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
    ];
    for path in assets
        .into_iter()
        .chain(sources.iter().map(|path| path.as_path()))
    {
        hash.update(path.file_name().unwrap().to_string_lossy().as_bytes());
        hash.update(fs::metadata(path)?.len().to_le_bytes());
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
