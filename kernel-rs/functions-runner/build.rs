use deno_core::{JsRuntimeForSnapshot, OpState, op2, v8};
use deno_error::JsErrorBox;
use konae_functions_executor::protocol::{BodyChunk, Capability};
use std::{env, fs, path::PathBuf};

#[op2]
#[serde]
fn op_worker_call(
    _state: &mut OpState,
    #[serde] _capability: Capability,
    #[string] _operation: String,
    #[serde] _args: serde_json::Value,
) -> Result<serde_json::Value, JsErrorBox> {
    Err(JsErrorBox::generic("snapshot_op_unavailable"))
}

#[op2(fast)]
fn op_worker_failure(_state: &mut OpState, #[string] _code: String) {}

#[op2]
#[serde]
fn op_worker_body_read(
    _state: &mut OpState,
    #[serde] _capability: Capability,
    #[number] _sequence: u64,
    #[number] _maximum: usize,
) -> Result<BodyChunk, JsErrorBox> {
    Err(JsErrorBox::generic("snapshot_op_unavailable"))
}

deno_core::extension!(
    functions_bridge,
    ops = [op_worker_call, op_worker_failure, op_worker_body_read]
);

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set");
    let extensions = vec![
        deno_webidl::deno_webidl::init(),
        deno_web::deno_web::init(
            std::sync::Arc::new(deno_web::BlobStore::default()),
            None,
            false,
            Default::default(),
        ),
        deno_crypto::deno_crypto::init(None),
        deno_net::deno_net::init(None, None),
        deno_fetch::deno_fetch::init(Default::default()),
        functions_bridge::init(),
    ];
    let snapshot = deno_core::snapshot::create_snapshot(
        deno_core::snapshot::CreateSnapshotOptions {
            cargo_manifest_dir: Box::leak(manifest_dir.into_boxed_str()),
            startup_snapshot: None,
            skip_op_registration: false,
            extensions,
            extension_transpiler: None,
            with_runtime_cb: Some(Box::new(|runtime: &mut JsRuntimeForSnapshot| {
                let bootstrap_source = include_str!("src/bootstrap.js")
                    .replace("delete globalThis.Deno;", "")
                    .replace("delete globalThis.__bootstrap;", "");
                let bootstrap = runtime
                    .execute_script("worker:bootstrap", bootstrap_source)
                    .expect("execute runner bootstrap");
                deno_core::scope!(scope, runtime);
                let global = scope.get_current_context().global(scope);
                let key = v8::String::new(scope, "__functions_bootstrap")
                    .expect("allocate bootstrap global name");
                global
                    .set(scope, key.into(), v8::Local::new(scope, bootstrap))
                    .expect("save runner bootstrap callbacks");
            })),
        },
        None,
    )
    .expect("create runner startup snapshot");
    for path in snapshot.files_loaded_during_snapshot {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    fs::write(out.join("startup_snapshot.bin"), snapshot.output)
        .expect("write runner startup snapshot");
    println!("cargo:rerun-if-changed=src/bootstrap.js");
}
