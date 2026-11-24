use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, process::Command};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_konae-functions-prepare")
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn identity_has_pinned_protocol_fields() {
    let output = Command::new(binary()).arg("identity").output().unwrap();
    assert!(output.status.success());
    let identity: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(identity["abi"], "konae-functions-wasm/1");
    for key in ["engineSha256", "builderFingerprint"] {
        let value = identity[key].as_str().unwrap();
        assert_eq!(value.len(), 64);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
    }
}

#[test]
fn prepare_emits_a_component_and_hashes_the_exact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("entry.mjs");
    let output = dir.path().join("component.wasm");
    let source = b"export async function handle() { return new Response('ready'); }\n";
    fs::write(&input, source).unwrap();
    let result = Command::new(binary())
        .args([
            "prepare",
            "--input",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let reply: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(reply["status"], "ok", "{reply}");
    assert_eq!(reply["manifest"]["abi"], "konae-functions-wasm/1");
    assert_eq!(
        reply["manifest"]["inputSha256"],
        hex(&Sha256::digest(source))
    );
    let component = fs::read(output).unwrap();
    assert!(!component.is_empty());
    assert_eq!(
        reply["manifest"]["componentSha256"],
        hex(&Sha256::digest(&component))
    );
}

fn prepare_source(source: &str) -> Value {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("entry.mjs");
    let output = dir.path().join("component.wasm");
    fs::write(&input, source).unwrap();
    let result = Command::new(binary())
        .args([
            "prepare",
            "--input",
            input.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

#[test]
fn resolved_await_and_work_free_pending_promise_prepare() {
    for source in [
        "await Promise.resolve(); export async function handle() { return new Response('ok'); }",
        "void new Promise(() => {}); export async function handle() { return new Response('ok'); }",
    ] {
        let reply = prepare_source(source);
        assert_eq!(reply["status"], "ok", "{reply}");
    }
}

#[test]
fn initialization_refusals_have_typed_statuses() {
    let cases = [
        ("export const handle = 1;", "bundle_contract"),
        (
            "setTimeout(() => {}, 1); export function handle() {}",
            "init_effect",
        ),
        (
            "const timer = setTimeout; try { timer(() => {}, 1); } catch {} export function handle() {}",
            "init_effect",
        ),
        (
            "const request = fetch; try { request('https://example.test'); } catch {} export function handle() {}",
            "init_effect",
        ),
        (
            "throw new Error('initialization failed'); export function handle() {}",
            "init_failed",
        ),
        (
            "const module = new WebAssembly.Module(new Uint8Array()); export function handle() {}",
            "unsupported_api",
        ),
    ];
    for (source, expected) in cases {
        let reply = prepare_source(source);
        assert_eq!(
            reply["status"], expected,
            "source {source:?}, reply {reply}"
        );
    }
}

#[test]
fn a_synchronous_initialization_loop_hits_the_deadline() {
    let reply = prepare_source("while (true) {} export function handle() {}");
    assert_eq!(reply["status"], "build_limit", "{reply}");
}

#[test]
fn an_unfinished_top_level_await_is_pending() {
    let reply = prepare_source("await new Promise(() => {}); export function handle() {}");
    assert_eq!(reply["status"], "init_pending", "{reply}");
}
