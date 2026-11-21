use crate::worker::{ChildBridge, IsolateRegistration};
use anyhow::{Result, anyhow};
use deno_core::{
    Extension, ExtensionFileSource, JsRuntime, ModuleLoadOptions, ModuleLoadReferrer,
    ModuleLoadResponse, ModuleLoader, ModuleSpecifier, OpState, ResolutionKind, RuntimeOptions,
    op2, v8,
};
use deno_error::JsErrorBox;
pub use konae_functions_executor::{Input, Output};
use konae_functions_executor::{
    WorkerError,
    protocol::{Capability, WorkerLimits},
};
use std::{
    borrow::Cow,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod embedded_extensions {
    include!(concat!(env!("OUT_DIR"), "/embedded_extensions.rs"));
}

fn embed_extension(mut extension: Extension) -> Result<Extension> {
    fn embed(
        extension_name: &str,
        sources: &mut Cow<'static, [ExtensionFileSource]>,
    ) -> Result<()> {
        let embedded = sources
            .iter()
            .map(|source| {
                let file = source
                    .specifier
                    .rsplit('/')
                    .next()
                    .ok_or_else(|| anyhow!("invalid_extension_specifier"))?;
                let code = embedded_extensions::source(extension_name, file)
                    .ok_or_else(|| anyhow!("extension_source_missing"))?;
                Ok(ExtensionFileSource::new_computed(
                    source.specifier,
                    Arc::<str>::from(code),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        *sources = Cow::Owned(embedded);
        Ok(())
    }

    embed(extension.name, &mut extension.js_files)?;
    embed(extension.name, &mut extension.esm_files)?;
    embed(extension.name, &mut extension.lazy_loaded_esm_files)?;
    embed(extension.name, &mut extension.lazy_loaded_js_files)?;
    Ok(extension)
}

#[cfg(test)]
mod embedded_extension_tests {
    use super::*;

    #[test]
    fn runtime_extensions_no_longer_reference_build_paths() {
        let extensions = [
            embed_extension(deno_webidl::deno_webidl::init()).unwrap(),
            embed_extension(deno_web::deno_web::init(
                Arc::new(deno_web::BlobStore::default()),
                None,
                false,
                Default::default(),
            ))
            .unwrap(),
            embed_extension(deno_net::deno_net::init(None, None)).unwrap(),
            embed_extension(deno_fetch::deno_fetch::init(Default::default())).unwrap(),
        ];
        for extension in extensions {
            for source in extension
                .js_files
                .iter()
                .chain(extension.esm_files.iter())
                .chain(extension.lazy_loaded_esm_files.iter())
                .chain(extension.lazy_loaded_js_files.iter())
            {
                assert!(source.is_runtime_loadable(), "{}", source.specifier);
            }
        }
    }
}

#[op2]
#[serde]
fn op_worker_call(
    state: &mut OpState,
    #[serde] capability: Capability,
    #[string] operation: String,
    #[serde] args: serde_json::Value,
) -> Result<serde_json::Value, JsErrorBox> {
    state
        .borrow::<Arc<ChildBridge>>()
        .call(capability, operation, args)
        .map_err(JsErrorBox::generic)
}
#[op2(fast)]
fn op_worker_failure(state: &mut OpState, #[string] code: String) {
    state.borrow::<Arc<ChildBridge>>().record_error(&code);
}
#[op2]
#[serde]
fn op_worker_body_read(
    state: &mut OpState,
    #[serde] capability: Capability,
    #[number] sequence: u64,
    #[number] maximum: usize,
) -> Result<konae_functions_executor::protocol::BodyChunk, JsErrorBox> {
    state
        .borrow::<Arc<ChildBridge>>()
        .read_body(capability, sequence, maximum)
        .map_err(JsErrorBox::generic)
}
deno_core::extension!(
    functions_bridge,
    ops = [op_worker_call, op_worker_failure, op_worker_body_read]
);

struct RejectUserModules {
    denied: Arc<AtomicBool>,
    entry: ModuleSpecifier,
}

impl ModuleLoader for RejectUserModules {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        kind: ResolutionKind,
    ) -> std::result::Result<ModuleSpecifier, JsErrorBox> {
        if kind == ResolutionKind::MainModule && referrer == "." && specifier == self.entry.as_str()
        {
            return Ok(self.entry.clone());
        }
        self.denied.store(true, Ordering::SeqCst);
        Err(JsErrorBox::generic("function_import_forbidden"))
    }

    fn load(
        &self,
        _module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        self.denied.store(true, Ordering::SeqCst);
        ModuleLoadResponse::Sync(Err(JsErrorBox::generic("function_import_forbidden")))
    }
}

pub struct Runtime {
    // The registration stops and joins the watchdog before Rust drops the isolate.
    _isolate_registration: IsolateRegistration,
    js: JsRuntime,
    invoke: v8::Global<v8::Function>,
    cancel: v8::Global<v8::Function>,
    shutdown: v8::Global<v8::Function>,
    handler: v8::Global<v8::Value>,
    shutdown_handler: v8::Global<v8::Value>,
    pub has_shutdown: bool,
    bridge: Arc<ChildBridge>,
    response_bytes: usize,
}
impl Runtime {
    pub async fn new(
        function: &str,
        source: String,
        limits: &WorkerLimits,
        bridge: Arc<ChildBridge>,
    ) -> Result<Self> {
        let extensions = vec![
            embed_extension(deno_webidl::deno_webidl::init())?,
            embed_extension(deno_web::deno_web::init(
                Arc::new(deno_web::BlobStore::default()),
                None,
                false,
                Default::default(),
            ))?,
            embed_extension(deno_net::deno_net::init(None, None))?,
            embed_extension(deno_fetch::deno_fetch::init(Default::default()))?,
            functions_bridge::init(),
        ];
        let specifier =
            deno_core::ModuleSpecifier::parse(&format!("file:///function/{function}.mjs"))?;
        let rejected_import = Arc::new(AtomicBool::new(false));
        let mut js = JsRuntime::new(RuntimeOptions {
            create_params: Some(v8::CreateParams::default().heap_limits(0, limits.heap_bytes)),
            module_loader: Some(Rc::new(RejectUserModules {
                denied: rejected_import.clone(),
                entry: specifier.clone(),
            })),
            extensions,
            ..Default::default()
        });
        let isolate_registration = bridge.register_isolate(js.v8_isolate().thread_safe_handle());
        js.op_state()
            .borrow_mut()
            .put(deno_permissions::PermissionsContainer::new(
                Arc::new(deno_permissions::RuntimePermissionDescriptorParser::new(
                    sys_traits::impls::RealSys,
                )),
                deno_permissions::Permissions::none_without_prompt(),
            ));
        js.op_state().borrow_mut().put(bridge.clone());
        let bootstrap = js.execute_script("worker:bootstrap", include_str!("bootstrap.js"))?;
        let (invoke, cancel, shutdown) = {
            deno_core::scope!(scope, js);
            let object = v8::Local::new(scope, bootstrap)
                .to_object(scope)
                .ok_or_else(|| anyhow!("bootstrap_invalid"))?;
            let function = |name: &str| -> Result<v8::Global<v8::Function>> {
                let key = v8::String::new(scope, name).unwrap();
                let value = object
                    .get(scope, key.into())
                    .ok_or_else(|| anyhow!("bootstrap_invalid"))?;
                let callback = v8::Local::<v8::Function>::try_from(value)
                    .map_err(|_| anyhow!("bootstrap_invalid"))?;
                Ok(v8::Global::new(scope, callback))
            };
            (
                function("invoke")?,
                function("cancel")?,
                function("shutdown")?,
            )
        };
        let id = match js.load_main_es_module_from_code(&specifier, source).await {
            Ok(id) => id,
            Err(_error) if rejected_import.load(Ordering::SeqCst) => {
                return Err(WorkerError::new("function_import_forbidden").into());
            }
            Err(error) => return Err(error.into()),
        };
        let evaluation = js.mod_evaluate(id);
        js.run_event_loop(Default::default()).await?;
        evaluation.await?;
        let namespace = js.get_module_namespace(id)?;
        let (handler, shutdown_handler, has_shutdown) = {
            deno_core::scope!(scope, js);
            let object = v8::Local::new(scope, namespace);
            let key = v8::String::new(scope, "handle").unwrap();
            let handler = object
                .get(scope, key.into())
                .ok_or_else(|| anyhow!("handler_missing"))?;
            if !handler.is_function() {
                return Err(anyhow!("handler_missing"));
            }
            let key = v8::String::new(scope, "shutdown").unwrap();
            let shutdown = object
                .get(scope, key.into())
                .ok_or_else(|| anyhow!("shutdown_invalid"))?;
            let has_shutdown = shutdown.is_function();
            (
                v8::Global::new(scope, handler),
                v8::Global::new(scope, shutdown),
                has_shutdown,
            )
        };
        Ok(Self {
            _isolate_registration: isolate_registration,
            js,
            invoke,
            cancel,
            shutdown,
            handler,
            shutdown_handler,
            has_shutdown,
            bridge,
            response_bytes: limits.response_bytes,
        })
    }
    pub async fn invoke(&mut self, input: Input, capability: Capability) -> Result<Output> {
        let args = {
            deno_core::scope!(scope, self.js);
            let input = deno_core::serde_v8::to_v8(scope, input)?;
            let input = v8::Global::new(scope, input);
            let capability = deno_core::serde_v8::to_v8(scope, capability)?;
            let capability = v8::Global::new(scope, capability);
            let limit: v8::Local<v8::Value> =
                v8::Number::new(scope, self.response_bytes as f64).into();
            vec![
                self.handler.clone(),
                input,
                capability,
                v8::Global::new(scope, limit),
            ]
        };
        let invoke = self.invoke.clone();
        let result = self.run(&invoke, args, None).await;
        let pending = self
            .js
            .runtime_activity_stats_factory()
            .capture(&deno_core::stats::RuntimeActivityStatsFilter::all())
            .dump();
        if !pending.active.is_empty() {
            self.bridge.record_error("pending_work");
            return Err(anyhow!("pending_work"));
        }
        let text = result?;
        serde_json::from_str(&text).map_err(Into::into)
    }
    pub async fn spin(&mut self, _capability: Capability) -> Result<Output> {
        let result = self
            .js
            .execute_script("worker:loop", "while (true) {}".to_owned());
        result?;
        Err(anyhow!("loop_returned"))
    }
    pub async fn shutdown(&mut self, grace: Duration) -> Result<()> {
        if !self.has_shutdown {
            return Ok(());
        }
        let callback = self.shutdown.clone();
        let args = vec![self.shutdown_handler.clone()];
        self.run(
            &callback,
            args,
            Some(Instant::now() + grace.saturating_sub(Duration::from_millis(5))),
        )
        .await?;
        Ok(())
    }
    async fn run(
        &mut self,
        callback: &v8::Global<v8::Function>,
        args: Vec<v8::Global<v8::Value>>,
        abort_at: Option<Instant>,
    ) -> Result<String> {
        let pending = self.js.call_with_args(callback, &args);
        tokio::pin!(pending);
        let mut aborted = false;
        let value = loop {
            let step = tokio::select! {
                result = self.js.with_event_loop_future(&mut pending, Default::default()) => Some(result),
                _ = tokio::time::sleep(Duration::from_millis(2)) => None,
            };
            if let Some(result) = step {
                break result?;
            }
            if !aborted
                && (self.bridge.cancelled()
                    || abort_at.is_some_and(|deadline| Instant::now() >= deadline))
            {
                aborted = true;
                let abort = self.js.call_with_args(&self.cancel, &[]);
                self.js
                    .with_event_loop_future(abort, Default::default())
                    .await?;
            }
        };
        if let Some(error) = self.bridge.error() {
            return Err(anyhow!(error));
        }
        deno_core::scope!(scope, self.js);
        let value = v8::Local::new(scope, value);
        deno_core::serde_v8::from_v8(scope, value).map_err(Into::into)
    }
}
