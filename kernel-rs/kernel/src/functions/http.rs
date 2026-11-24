use super::invocation::{Admission, InvocationConfig};
use super::trace::{self, TraceBuffer, TraceContext, TraceRecord};
use super::{Function, invocation as function_data, runtime::RunProgress};
use crate::{
    AppState, Deployment, Operation,
    usage::{FunctionOutcome, FunctionTiming},
};
use axum::{
    Json,
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use konae_functions_executor::{Budget, Input, Output, RequestBody};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_FUNCTION_BODY_BYTES: usize = 100_000_000;
const MAX_AGGREGATE_PENDING_BODY_BYTES: usize = 8 * 1024 * 1024;
static FUNCTION_BODY_BUDGET: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();

fn function_body_budget() -> Arc<tokio::sync::Semaphore> {
    FUNCTION_BODY_BUDGET
        .get_or_init(|| {
            Arc::new(tokio::sync::Semaphore::new(
                MAX_AGGREGATE_PENDING_BODY_BYTES,
            ))
        })
        .clone()
}

enum BodyFrame {
    Data(Vec<u8>, tokio::sync::OwnedSemaphorePermit),
}
struct BodyState {
    length: usize,
    digest: Sha256,
    complete: bool,
    error: Option<&'static str>,
}
struct IngressBody {
    receiver: Mutex<tokio::sync::mpsc::Receiver<BodyFrame>>,
    state: Mutex<BodyState>,
}
impl konae_functions_executor::RequestBody for IngressBody {
    fn read_chunk(
        &self,
        maximum: usize,
    ) -> Result<konae_functions_executor::RequestBodyChunk, String> {
        match self.receiver.lock().unwrap().blocking_recv() {
            Some(BodyFrame::Data(bytes, permit)) if bytes.len() <= maximum => {
                Ok(konae_functions_executor::RequestBodyChunk::retaining(
                    bytes,
                    false,
                    Box::new(permit),
                ))
            }
            Some(BodyFrame::Data(_, _)) => Err("worker_protocol_error".into()),
            None => {
                let state = self.state.lock().unwrap();
                match state.error {
                    Some(code) => Err(code.into()),
                    None if state.complete => Ok(konae_functions_executor::RequestBodyChunk::new(
                        Vec::new(),
                        true,
                    )),
                    None => Err("worker_protocol_error".into()),
                }
            }
        }
    }
    fn complete(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.complete && state.error.is_none()
    }
}
impl IngressBody {
    fn drain(&self) -> Result<(), String> {
        loop {
            match self.read_chunk(konae_functions_executor::protocol::BODY_CHUNK_BYTES) {
                Ok(chunk) if chunk.done => return Ok(()),
                Ok(_) => {}
                Err(code) => return Err(code),
            }
        }
    }
    fn summary(&self) -> Option<(usize, [u8; 32])> {
        let state = self.state.lock().unwrap();
        (state.complete && state.error.is_none())
            .then(|| (state.length, state.digest.clone().finalize().into()))
    }
}
struct BodyPump(tokio::task::JoinHandle<()>);
impl BodyPump {
    #[cfg(test)]
    async fn cancel(&mut self) {
        self.0.abort();
        let _ = (&mut self.0).await;
    }
}
impl Drop for BodyPump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn start_body_pump(body: Body, budget: Budget) -> (Arc<IngressBody>, BodyPump) {
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let ingress = Arc::new(IngressBody {
        receiver: Mutex::new(receiver),
        state: Mutex::new(BodyState {
            length: 0,
            digest: Sha256::new(),
            complete: false,
            error: None,
        }),
    });
    let state = ingress.clone();
    let task = tokio::spawn(async move {
        let mut stream = body.into_data_stream();
        loop {
            let item = tokio::time::timeout_at(
                tokio::time::Instant::from_std(budget.deadline()),
                stream.next(),
            )
            .await;
            let chunk = match item {
                Err(_) => {
                    state.state.lock().unwrap().error = Some("deadline_exceeded");
                    return;
                }
                Ok(None) => {
                    state.state.lock().unwrap().complete = true;
                    return;
                }
                Ok(Some(Err(_))) => {
                    state.state.lock().unwrap().error = Some("invalid_body");
                    return;
                }
                Ok(Some(Ok(chunk))) => chunk,
            };
            for bytes in chunk.chunks(konae_functions_executor::protocol::BODY_CHUNK_BYTES) {
                {
                    let mut state = state.state.lock().unwrap();
                    if state.length.saturating_add(bytes.len()) > MAX_FUNCTION_BODY_BYTES {
                        state.error = Some("body_too_large");
                        return;
                    }
                    state.length += bytes.len();
                    state.digest.update(bytes);
                }
                let permit = match function_body_budget().try_acquire_many_owned(bytes.len() as u32)
                {
                    Ok(permit) => permit,
                    Err(_) => {
                        state.state.lock().unwrap().error = Some("function.capacity");
                        return;
                    }
                };
                let frame = BodyFrame::Data(bytes.to_vec(), permit);
                if !matches!(
                    tokio::time::timeout_at(
                        tokio::time::Instant::from_std(budget.deadline()),
                        sender.send(frame),
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    state.state.lock().unwrap().error = Some("deadline_exceeded");
                    return;
                }
            }
        }
    });
    (ingress, BodyPump(task))
}

struct Cancel(Budget);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
#[derive(Clone)]
struct ResponseCode(String);
struct NativeTraceCompletion(TraceBuffer);
impl Drop for NativeTraceCompletion {
    fn drop(&mut self) {
        self.0.native_finished();
    }
}
struct Timed {
    response: Response,
    queue: Duration,
    initialization: Duration,
    execution: Duration,
    cold: bool,
    outcome: FunctionOutcome,
    error_code: Option<String>,
    error_detail: Option<String>,
}
struct PreparedRun {
    lease: super::runtime::Lease,
    config: InvocationConfig,
    input: Input,
    body: Arc<IngressBody>,
    body_pump: BodyPump,
}

fn acquire_action_resources(
    state: &AppState,
    deployment: &str,
    method: &str,
    budget: &Budget,
) -> Result<(tokio::sync::OwnedSemaphorePermit, crate::ActionLock), &'static str> {
    let deadline = tokio::time::Instant::from_std(budget.deadline());
    tokio::runtime::Handle::current().block_on(async {
        tokio::time::timeout_at(deadline, async {
            let slot = state
                .guards
                .statement_permit(deployment)
                .await
                .map_err(|_| "function.capacity")?;
            let lock = crate::action_lock(
                state,
                deployment,
                if method == "GET" { "query" } else { "mutation" },
            )
            .await;
            Ok((slot, lock))
        })
        .await
        .map_err(|_| "deadline_exceeded")?
    })
}
enum Prepared {
    Replay(Timed),
    Run(PreparedRun),
}
enum ReceiptPrelookup {
    Continue(Body),
    Replay(Timed),
}
async fn read_replay_body(body: Body, budget: &Budget) -> Result<(usize, [u8; 32]), Response> {
    let mut incoming = body.into_data_stream();
    let mut length = 0usize;
    let mut digest = Sha256::new();
    while let Some(chunk) = tokio::time::timeout_at(
        tokio::time::Instant::from_std(budget.deadline()),
        incoming.next(),
    )
    .await
    .map_err(|_| executor_failure("deadline_exceeded"))?
    {
        let chunk = chunk.map_err(|_| {
            failure(
                400,
                "invalid_body",
                "The function request body could not be read.",
            )
        })?;
        if length.saturating_add(chunk.len()) > MAX_FUNCTION_BODY_BYTES {
            return Err(failure(
                413,
                "body_too_large",
                "The function request exceeds 100000000 bytes.",
            ));
        }
        length += chunk.len();
        digest.update(chunk);
    }
    Ok((length, digest.finalize().into()))
}
impl Timed {
    fn refusal(response: Response, elapsed: Duration) -> Self {
        let error_code = response
            .extensions()
            .get::<ResponseCode>()
            .map(|value| value.0.clone());
        Self {
            response,
            queue: elapsed,
            initialization: Duration::ZERO,
            execution: Duration::ZERO,
            cold: false,
            outcome: FunctionOutcome::Refused,
            error_code,
            error_detail: None,
        }
    }
}

async fn prelookup_receipt(
    database: &PathBuf,
    actor: Option<&str>,
    function_name: &str,
    key: &str,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: Body,
    budget: &Budget,
    started: Instant,
) -> Result<ReceiptPrelookup, Response> {
    let candidate = function_data::lookup_receipt(database, actor, function_name, key, budget)
        .map_err(|error| failure(error.status, error.code, &error.message))?;
    let Some(stored_fingerprint) = candidate else {
        return Ok(ReceiptPrelookup::Continue(body));
    };
    let (body_len, body_digest) = read_replay_body(body, budget).await?;
    let fingerprint =
        function_data::request_fingerprint_v3(method, path, body_len, &body_digest, headers)
            .map_err(|error| failure(error.status, error.code, &error.message))?;
    if !stored_fingerprint.starts_with("v3:") {
        return Err(failure(
            409,
            "idempotency_conflict",
            "Idempotency-Key was already used with a different request.",
        ));
    }
    if fingerprint != stored_fingerprint {
        return Err(failure(
            409,
            "idempotency_conflict",
            "Idempotency-Key was already used with a different request.",
        ));
    }
    let replay = function_data::Failure::replay();
    Ok(ReceiptPrelookup::Replay(Timed {
        response: failure(replay.status, replay.code, &replay.message),
        queue: started.elapsed(),
        initialization: Duration::ZERO,
        execution: Duration::ZERO,
        cold: false,
        outcome: FunctionOutcome::Replayed,
        error_code: None,
        error_detail: None,
    }))
}

fn failure(status: u16, code: &str, message: &str) -> Response {
    let mut response = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({"error":message,"code":code})),
    )
        .into_response();
    response
        .extensions_mut()
        .insert(ResponseCode(code.to_owned()));
    response
}
fn executor_failure(code: &str) -> Response {
    let status = match code {
        "deadline_exceeded" | "cancelled" => 504,
        "mutation_outcome_unknown" => 409,
        "function.runner-unavailable"
        | "sandbox_unavailable"
        | "containment_unavailable"
        | "function.capacity"
        | "worker_protocol_mismatch"
        | "queue_full"
        | "cache_busy" => 503,
        "worker_failed"
        | "worker_initialization_failed"
        | "worker_execution_failed"
        | "worker_protocol_error"
        | "invalid_response"
        | "worker_memory_limit"
        | "response_too_large"
        | "pending_work" => 502,
        _ => 500,
    };
    failure(
        status,
        code,
        match code {
            "sandbox_unavailable" => {
                "The host cannot apply the required OS isolation to function workers."
            }
            "worker_protocol_mismatch" => {
                "The Function runner protocol is incompatible. Restart the service with a matching Kernel and runner pair."
            }
            _ => match status {
                504 => "The function exceeded its execution budget.",
                503 => "The function executor is unavailable.",
                409 => {
                    "The mutation outcome is unknown. Repeat this request with the same Idempotency-Key."
                }
                _ => "The function could not complete.",
            },
        },
    )
}
fn outcome(code: &str) -> FunctionOutcome {
    match code {
        "deadline_exceeded" | "cancelled" => FunctionOutcome::DeadlineExceeded,
        "mutation_outcome_unknown" => FunctionOutcome::MutationOutcomeUnknown,
        function_data::IDEMPOTENCY_REPLAY_CODE => FunctionOutcome::Replayed,
        _ => FunctionOutcome::Failed,
    }
}

/// After the deadline the executor gets this long to bring the worker back cooperatively
/// (its own cancel handshake) before the Kernel kills the worker process from outside.
const ABORT_GRACE: Duration = Duration::from_millis(200);

/// Kills the request's worker if its invocation is still running once the grace has passed.
/// An invocation that ended in time cleared its abort handler, so a warm worker survives.
fn schedule_abort(budget: Budget) {
    tokio::spawn(async move {
        tokio::time::sleep(ABORT_GRACE).await;
        budget.abort();
    });
}

fn interrupted(budget: &Budget, code: &str, started: Instant, progress: &RunProgress) -> Timed {
    let mutation = budget.cancel();
    let code = if mutation.may_have_committed() {
        "mutation_outcome_unknown"
    } else {
        code
    };
    let now = Instant::now();
    let queue = progress
        .initialization_started
        .or(progress.execution_started)
        .unwrap_or(now)
        .saturating_duration_since(started);
    let initialization = progress
        .initialization_started
        .map(|at| {
            progress
                .execution_started
                .or(progress.finished)
                .unwrap_or(now)
                .saturating_duration_since(at)
        })
        .unwrap_or_default();
    let execution = progress
        .execution_started
        .map(|at| now.saturating_duration_since(at))
        .unwrap_or_default();
    Timed {
        response: executor_failure(code),
        queue,
        initialization,
        execution,
        cold: progress.cold,
        outcome: outcome(code),
        error_code: Some(code.to_owned()),
        error_detail: None,
    }
}

fn record(
    state: &AppState,
    deployment: &Deployment,
    name: String,
    budget: &Budget,
    started: Instant,
    timed: Timed,
    trace: &TraceBuffer,
) -> Response {
    let now = Instant::now();
    let status = timed.response.status().as_u16();
    trace.set_root_error(timed.error_code.as_deref());
    let timing = FunctionTiming {
        name,
        version: deployment.artifact_version.clone(),
        budget: budget.deadline().duration_since(started),
        total: now.duration_since(started),
        queue: timed.queue,
        initialization: timed.initialization,
        execution: timed.execution,
        cold: timed.cold,
        timed_out: now >= budget.deadline(),
        outcome: timed.outcome,
        error_code: timed.error_code,
        error_detail: timed.error_detail,
    };
    if timed.outcome == FunctionOutcome::Replayed {
        let _ = trace.push(TraceRecord {
            parent: Some(0),
            name: "konae.function.receipt_replay",
            start_us: 0,
            duration_us: timed.queue.as_micros().min(u64::MAX as u128) as u64,
            action_name: None,
            origin: None,
            sequence: None,
            rows_read: None,
            rows_written: None,
            reply_bytes: None,
            sqlite_us: None,
            error_code: Some(function_data::IDEMPOTENCY_REPLAY_CODE),
        });
    }
    let outcome = match timed.outcome {
        FunctionOutcome::Ok => "ok",
        FunctionOutcome::Refused => "refused",
        FunctionOutcome::Failed => "failed",
        FunctionOutcome::DeadlineExceeded => "deadline_exceeded",
        FunctionOutcome::MutationOutcomeUnknown => "mutation_outcome_unknown",
        FunctionOutcome::Replayed => "replayed",
    };
    let data_root = state.data_root.clone();
    let deployment_id = deployment.id.clone();
    let local = state.execution_mode.is_test();
    crate::usage::function_capture(data_root, deployment_id, timing, trace.clone(), local);
    trace.finish(status, outcome);
    let mut response = timed.response;
    trace::attach_response(&mut response, trace);
    response
}
fn declaration(deployment: &Deployment, path: &str) -> Option<(String, Function)> {
    deployment
        .server
        .as_ref()?
        .manifest
        .functions
        .iter()
        .find(|(_, f)| {
            path == f.mount
                || path
                    .strip_prefix(&f.mount)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
        .map(|(name, function)| (name.clone(), function.clone()))
}

/// A function request holds the publication lease until native work and its receipt finish.
pub(crate) async fn dispatch(state: &AppState, request: Request) -> Result<Response, Request> {
    let Some(observed) = crate::deployment_for(state, request.headers()).await else {
        return Err(request);
    };
    let Some((observed_name, _)) = declaration(&observed, request.uri().path()) else {
        return Err(request);
    };
    let started = Instant::now();
    let runner_kind = if std::env::var("KONAE_FUNCTIONS_RUNNER_KIND").as_deref() == Ok("wasm") {
        "wasm"
    } else {
        "deno"
    };
    let trace = TraceBuffer::new(
        TraceContext::from_headers(request.headers(), state.execution_mode.is_test()),
        started,
        runner_kind,
    );
    let budget = Budget::until(started + state.functions.deadline);
    let _cancel = Cancel(budget.clone());
    let deadline = tokio::time::Instant::from_std(budget.deadline());
    let (generation, held, deployment) = match tokio::time::timeout_at(deadline, async {
        let generation = state.copies.generations.clone().read_owned().await;
        let held = crate::named_lock(state, &format!("publish:{}", observed.id), false).await;
        let deployment = crate::deployment_by_id(state, &observed.id).await;
        (generation, held, deployment)
    })
    .await
    {
        Ok(held) => held,
        Err(_) => {
            let timed = interrupted(
                &budget,
                "deadline_exceeded",
                started,
                &RunProgress::default(),
            );
            return Ok(record(
                state,
                &observed,
                observed_name,
                &budget,
                started,
                timed,
                &trace,
            ));
        }
    };
    let Some(deployment) = deployment else {
        let mut response = failure(404, "function.not-found", "Function not found.");
        trace::attach_response(&mut response, &trace);
        return Ok(response);
    };
    let Some((name, function)) = declaration(&deployment, request.uri().path()) else {
        return Err(request);
    };
    let server_digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(deployment.server.as_ref().unwrap())
                .expect("published metadata serializes")
        )
    );
    let operation = request
        .extensions()
        .get::<Operation>()
        .map(|operation| operation.0.clone());
    let prepare = async {
        if let Some(message) = crate::maintenance::pause(&state.data_root, &deployment.id) {
            return Err(crate::maintenance::refusal(&message));
        }
        crate::modules::require_on(&deployment.modules, "functions")
            .map_err(|refused| crate::refusal_error(&refused))?;
        if !function
            .methods
            .iter()
            .any(|method| method == request.method().as_str())
        {
            let mut response = failure(
                405,
                "function.method-not-allowed",
                "The function does not declare this HTTP method.",
            );
            response.headers_mut().insert(
                header::ALLOW,
                HeaderValue::from_str(&function.methods.join(", ")).unwrap(),
            );
            return Err(response);
        }
        if !crate::same_origin(request.headers())
            || crate::path::foreign_referer(state, request.headers(), &deployment.id)
        {
            return Err(failure(
                403,
                "function.origin-refused",
                "Cross-origin function request rejected.",
            ));
        }
        if crate::runtime::bearer(request.headers()).is_some_and(|token| token.starts_with("lbk_"))
        {
            return Err(failure(
                403,
                "function.key-refused",
                "Functions do not accept API keys.",
            ));
        }
        let actor = if crate::modules::is_on(&deployment.modules, "auth") {
            crate::authenticate_deployment(state, &deployment, request.headers()).await?
        } else {
            None
        };
        if (crate::is_private(&deployment) || function.access == "authenticated") && actor.is_none()
        {
            return Err(failure(
                401,
                "authentication-required",
                "authentication-required",
            ));
        }
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|value| value.0);
        let client = state.guards.client_ip(peer, request.headers());
        let limited = match &actor {
            Some(user) => state
                .guards
                .signed_in_actions
                .check(&format!("runtime:{}:{}", deployment.id, user.user_id)),
            None => state.guards.anonymous_actions.check(&format!(
                "{}|{}",
                deployment.id,
                crate::limits::client_key(client)
            )),
        };
        limited.map_err(crate::http::rate_limited)?;
        if request
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > MAX_FUNCTION_BODY_BYTES)
        {
            return Err(failure(
                413,
                "body_too_large",
                "The function request exceeds 100000000 bytes.",
            ));
        }
        let admission = state
            .functions
            .admit(&budget)
            .map_err(|code| executor_failure(&code))?;
        let actor_id = actor.map(|user| user.user_id);
        let headers = [header::ACCEPT, header::CONTENT_TYPE]
            .into_iter()
            .filter_map(|name| {
                request
                    .headers()
                    .get(&name)
                    .and_then(|value| value.to_str().ok())
                    .map(|value| (name.as_str().to_owned(), value.trim().to_owned()))
            })
            .collect::<Vec<_>>();
        let key = request
            .headers()
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let path = request
            .uri()
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/")
            .to_owned();
        let method = request.method().as_str().to_owned();
        let has_data = !function.actions.is_empty();
        let mut request_body = Some(request.into_body());
        if has_data {
            crate::modules::require_on(&deployment.modules, "actions")
                .map_err(|refused| crate::refusal_error(&refused))?;
        }
        if has_data && method == "POST" {
            let key = key
                .as_deref()
                .filter(|key| !key.is_empty())
                .ok_or_else(|| {
                    failure(
                        400,
                        "idempotency_key_required",
                        "Idempotency-Key is required for this function.",
                    )
                })?;
            if key.len() > 128 || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
                return Err(failure(
                    400,
                    "invalid_input",
                    "Idempotency-Key must contain 1 to 128 visible ASCII bytes.",
                ));
            }
            let database = crate::layout::database(&state.data_root, &deployment.id);
            match prelookup_receipt(
                &database,
                actor_id.as_deref(),
                &name,
                key,
                &method,
                &path,
                &headers,
                request_body.take().unwrap(),
                &budget,
                started,
            )
            .await?
            {
                ReceiptPrelookup::Continue(body) => request_body = Some(body),
                ReceiptPrelookup::Replay(timed) => return Ok(Prepared::Replay(timed)),
            }
        }
        let cache_key = format!(
            "{}:{}:{server_digest}",
            deployment.id, deployment.artifact_version
        );
        let lease = state
            .functions
            .lease_admitted(cache_key, &budget, admission)
            .await
            .map_err(|code| executor_failure(&code))?;
        let (body, body_pump) = start_body_pump(request_body.unwrap(), budget.clone());
        let program = Arc::new(crate::action_shape::read(&deployment.actions).map_err(|_| {
            failure(
                500,
                "function.program-invalid",
                "The published actions cannot be read.",
            )
        })?);
        let config = InvocationConfig {
            database: has_data.then(|| crate::layout::database(&state.data_root, &deployment.id)),
            program,
            function_name: name.clone(),
            function,
            actor: actor_id.clone(),
            method: method.clone(),
            path: path.clone(),
            body_len: 0,
            fingerprint: String::new(),
            headers: headers.clone(),
            idempotency_key: key,
        };
        let input = Input {
            url: format!("https://function.invalid{path}"),
            method,
            headers,
            body: Vec::new(),
            body_len: 0,
            actor: actor_id.unwrap_or_default(),
        };
        Ok(Prepared::Run(PreparedRun {
            lease,
            config,
            input,
            body,
            body_pump,
        }))
    };
    let prepared = tokio::time::timeout_at(deadline, prepare).await;
    let timed = match prepared {
        Err(_) => interrupted(
            &budget,
            "deadline_exceeded",
            started,
            &RunProgress::default(),
        ),
        Ok(Err(_)) if Instant::now() >= budget.deadline() => interrupted(
            &budget,
            "deadline_exceeded",
            started,
            &RunProgress::default(),
        ),
        Ok(Err(response)) => Timed::refusal(response, started.elapsed()),
        Ok(Ok(Prepared::Replay(timed))) => timed,
        Ok(Ok(Prepared::Run(PreparedRun {
            mut lease,
            config,
            input,
            body,
            body_pump,
        }))) => {
            let _body_pump = body_pump;
            let request_method = input.method.clone();
            let progress = lease.progress();
            let state = state.clone();
            let deployment = deployment.clone();
            let worker_budget = budget.clone();
            let name = name.clone();
            let worker_trace = trace.clone();
            trace.retain_native();
            let task = tokio::task::spawn_blocking(move || {
                let _native_trace_completion = NativeTraceCompletion(worker_trace.clone());
                let budget = worker_budget;
                let _operation = operation;
                let _generation = generation;
                let _held = held;
                let mut sql_slot = None;
                let mut sql_guard = None;
                let prepare_started = Instant::now();
                let bundles_result = pinned_bundle(&state, &deployment, &name);
                let prepare_us = prepare_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                let _ = worker_trace.push(TraceRecord {
                    parent: Some(0),
                    name: "konae.function.prepare",
                    start_us: worker_trace.elapsed_us().saturating_sub(prepare_us),
                    duration_us: prepare_us,
                    action_name: None,
                    origin: None,
                    sequence: None,
                    rows_read: None,
                    rows_written: None,
                    reply_bytes: None,
                    sqlite_us: None,
                    error_code: bundles_result
                        .as_ref()
                        .err()
                        .map(|_| "function.artifact-changed"),
                });
                let bundles = match bundles_result {
                    Ok(bundles) => bundles,
                    Err(_) => {
                        return Timed::refusal(
                            failure(
                                503,
                                "function.artifact-changed",
                                "The function bundle does not match its published version.",
                            ),
                            started.elapsed(),
                        );
                    }
                };
                let began = Instant::now();
                let queue = began.duration_since(started);
                let mut native_failure = None;
                let mut invocation = None;
                let mut replayed = None;
                let mut config = Some(config);
                let mut ipc_sequence = 0u16;
                let request_body: Arc<dyn konae_functions_executor::RequestBody> = body.clone();
                let action_body = body.clone();
                let action_trace = worker_trace.clone();
                let handler_started = Instant::now();
                let run = lease.run_stream(&name, &bundles, input, request_body, &budget, &mut |operation, args| {
                    ipc_sequence = ipc_sequence.saturating_add(1);
                    if operation != "action" {
                        let _ = action_trace.push(TraceRecord { parent: Some(0), name: "konae.action", start_us: action_trace.elapsed_us(), duration_us: 0,
                            action_name: Some("<unknown>"), origin: Some("function"), sequence: Some(ipc_sequence), rows_read: None, rows_written: None, reply_bytes: None, sqlite_us: None, error_code: Some("function.operation-forbidden".into()) });
                        return Err("function.operation-forbidden".into());
                    }
                    if request_method == "POST" && !action_body.complete() {
                        let error = function_data::Failure {
                            code: "function.request-body-incomplete",
                            message: "Consume the complete request body before calling a Server Action.".into(),
                            status: 400,
                        };
                        let code = error.code.to_owned();
                        native_failure = Some(error);
                        return Err(code);
                    }
                    if invocation.is_none() {
                        let (body_len, body_digest) = if request_method == "POST" {
                            let Some(summary) = action_body.summary() else {
                                return Err("worker_protocol_error".into());
                            };
                            summary
                        } else {
                            (0, Sha256::digest([]).into())
                        };
                        let mut config = config.take().ok_or("worker_protocol_error")?;
                        config.body_len = body_len;
                        config.fingerprint = match function_data::request_fingerprint_v3(
                            &config.method,
                            &config.path,
                            body_len,
                            &body_digest,
                            &config.headers,
                        ) {
                            Ok(value) => value,
                            Err(error) => {
                                let code = error.code.to_owned();
                                native_failure = Some(error);
                                return Err(code);
                            }
                        };
                        if config.database.is_some() {
                            match acquire_action_resources(&state, &deployment.id, &config.method, &budget) {
                                Ok((slot, lock)) => {
                                    sql_slot = Some(slot);
                                    sql_guard = Some(lock);
                                }
                                Err(code) => {
                                    let error = function_data::Failure {
                                        code,
                                        message: if code == "function.capacity" {
                                            "The function could not acquire a data slot."
                                        } else {
                                            "The function exceeded its execution budget."
                                        }.into(),
                                        status: if code == "function.capacity" { 503 } else { 504 },
                                    };
                                    native_failure = Some(error);
                                    return Err(code.into());
                                }
                            }
                        }
                        match function_data::admit(config, budget.clone()) {
                            Ok(Admission::Replay(error)) => {
                                let code = error.code.to_owned();
                                replayed = Some(error);
                                return Err(code);
                            }
                            Ok(Admission::Run(value)) => invocation = Some(value),
                            Err(error) => {
                                let code = error.code.to_owned();
                                native_failure = Some(error);
                                return Err(code);
                            }
                        }
                    }
                    let Some(action) = args["name"].as_str() else {
                        let _ = action_trace.push(TraceRecord { parent: Some(0), name: "konae.action", start_us: action_trace.elapsed_us(), duration_us: 0,
                            action_name: Some("<unknown>"), origin: Some("function"), sequence: Some(ipc_sequence), rows_read: None, rows_written: None, reply_bytes: None, sqlite_us: None, error_code: Some("function.input-invalid".into()) });
                        return Err("function.input-invalid".into());
                    };
                    match invocation.as_mut().unwrap().action_traced(action, args.get("input").unwrap_or(&Value::Null), &action_trace, ipc_sequence) {
                        Ok(answer) => Ok(answer),
                        Err(error) => {
                            let code = error.code.to_owned();
                            native_failure = Some(error);
                            Err(code)
                        }
                    }
                });
                let worker_us = handler_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
                let init_us = run.initialization.as_micros().min(u64::MAX as u128) as u64;
                let handler_us = worker_us.saturating_sub(init_us);
                let _ = action_trace.push(TraceRecord {
                    parent: Some(0),
                    name: "konae.function.worker",
                    start_us: action_trace.elapsed_us().saturating_sub(worker_us),
                    duration_us: worker_us,
                    action_name: Some(if run.cold { "cold" } else { "warm" }),
                    origin: None,
                    sequence: None,
                    rows_read: None,
                    rows_written: None,
                    reply_bytes: None,
                    sqlite_us: None,
                    error_code: None,
                });
                let _ = action_trace.push(TraceRecord {
                    parent: Some(0),
                    name: "konae.function.handler",
                    start_us: action_trace.elapsed_us().saturating_sub(handler_us),
                    duration_us: handler_us,
                    action_name: None,
                    origin: None,
                    sequence: None,
                    rows_read: None,
                    rows_written: None,
                    reply_bytes: None,
                    sqlite_us: None,
                    error_code: run
                        .output
                        .as_ref()
                        .err()
                        .map(|error| konae_functions_executor::error_code(error)),
                });
                if run.cold {
                    let _ = action_trace.push(TraceRecord {
                        parent: Some(0),
                        name: "konae.function.init",
                        start_us: action_trace
                            .elapsed_us()
                            .saturating_sub(handler_us)
                            .saturating_sub(init_us),
                        duration_us: init_us,
                        action_name: None,
                        origin: None,
                        sequence: None,
                        rows_read: None,
                        rows_written: None,
                        reply_bytes: None,
                        sqlite_us: None,
                        error_code: None,
                    });
                }
                let _admission = lease.take_admission();
                drop(lease);
                let ingress = body.drain();
                if let Err(code) = ingress {
                    let (status, message) = match code.as_str() {
                        "body_too_large" => (413, "The function request exceeds 100000000 bytes."),
                        "function.capacity" => (503, "The function request capacity is full."),
                        "invalid_body" => (400, "The function request body could not be read."),
                        _ => (504, "The function exceeded its execution budget."),
                    };
                    if let Some(mut invocation) = invocation.take() {
                        let _ = invocation.abort();
                    }
                    return Timed {
                        response: failure(status, &code, message),
                        queue,
                        initialization: run.initialization,
                        execution: began.elapsed().saturating_sub(run.initialization),
                        cold: run.cold,
                        outcome: outcome(&code),
                        error_code: Some(code.clone()),
                        error_detail: None,
                    };
                }
                if run.output.is_err() && invocation.is_none() && replayed.is_none() {
                    let error_detail = native_failure
                        .as_ref()
                        .map(|error| error.message.clone())
                        .or_else(|| {
                            run.output
                                .as_ref()
                                .err()
                                .and_then(konae_functions_executor::error_detail)
                                .map(str::to_owned)
                        });
                    let error_code = native_failure
                        .as_ref()
                        .map(|error| error.code.to_owned())
                        .or_else(|| {
                            Some(budget.failure_code().map(str::to_owned).unwrap_or_else(|| {
                                konae_functions_executor::error_code(
                                    run.output.as_ref().unwrap_err(),
                                )
                                .to_owned()
                            }))
                        });
                    let (response, result_outcome) = if let Some(error) = native_failure {
                        (
                            failure(error.status, error.code, &error.message),
                            outcome(error.code),
                        )
                    } else {
                        let code = budget.failure_code().map(str::to_owned).unwrap_or_else(|| {
                            konae_functions_executor::error_code(run.output.as_ref().unwrap_err())
                                .to_owned()
                        });
                        (executor_failure(&code), outcome(&code))
                    };
                    return Timed {
                        response,
                        queue,
                        initialization: run.initialization,
                        execution: began.elapsed().saturating_sub(run.initialization),
                        cold: run.cold,
                        outcome: result_outcome,
                        error_code,
                        error_detail,
                    };
                }
                if replayed.is_none() && invocation.is_none() {
                    let (body_len, body_digest) = if request_method == "POST" {
                        let Some(summary) = body.summary() else {
                            return Timed::refusal(
                                executor_failure("worker_protocol_error"),
                                started.elapsed(),
                            );
                        };
                        summary
                    } else {
                        (0, Sha256::digest([]).into())
                    };
                    let mut config = config.take().unwrap();
                    config.body_len = body_len;
                    config.fingerprint = match function_data::request_fingerprint_v3(
                        &config.method,
                        &config.path,
                        body_len,
                        &body_digest,
                        &config.headers,
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            return Timed::refusal(
                                failure(error.status, error.code, &error.message),
                                started.elapsed(),
                            );
                        }
                    };
                    if config.database.is_some() {
                        match acquire_action_resources(
                            &state,
                            &deployment.id,
                            &config.method,
                            &budget,
                        ) {
                            Ok((slot, lock)) => {
                                sql_slot = Some(slot);
                                sql_guard = Some(lock);
                            }
                            Err(code) => {
                                return Timed::refusal(executor_failure(code), started.elapsed());
                            }
                        }
                    }
                    match function_data::admit(config, budget.clone()) {
                        Ok(Admission::Replay(error)) => replayed = Some(error),
                        Ok(Admission::Run(value)) => invocation = Some(value),
                        Err(error) => {
                            return Timed::refusal(
                                failure(error.status, error.code, &error.message),
                                started.elapsed(),
                            );
                        }
                    }
                }
                if let Some(error) = replayed {
                    return Timed {
                        response: failure(error.status, error.code, &error.message),
                        queue,
                        initialization: run.initialization,
                        execution: began.elapsed().saturating_sub(run.initialization),
                        cold: run.cold,
                        outcome: FunctionOutcome::Replayed,
                        error_code: None,
                        error_detail: None,
                    };
                }
                let mut invocation = invocation.unwrap();
                let mut failure_code = None;
                let mut failure_detail = None;
                let (response, result_outcome) = match run.output {
                    Ok(output) => match invocation.finish(output) {
                        Ok(output) => {
                            let outcome = if output.status >= 500 {
                                FunctionOutcome::Failed
                            } else if output.status >= 400 {
                                FunctionOutcome::Refused
                            } else {
                                FunctionOutcome::Ok
                            };
                            (respond(output), outcome)
                        }
                        Err(error) => {
                            failure_code = Some(error.code.to_owned());
                            failure_detail = Some(error.message.clone());
                            (
                                failure(error.status, error.code, &error.message),
                                outcome(error.code),
                            )
                        }
                    },
                    Err(error) => {
                        let _ = invocation.abort();
                        if let Some(error) = native_failure {
                            failure_code = Some(error.code.to_owned());
                            failure_detail = Some(error.message.clone());
                            (
                                failure(error.status, error.code, &error.message),
                                outcome(error.code),
                            )
                        } else {
                            let code =
                                budget.failure_code().map(str::to_owned).unwrap_or_else(|| {
                                    konae_functions_executor::error_code(&error).to_owned()
                                });
                            failure_code = Some(code.clone());
                            failure_detail =
                                konae_functions_executor::error_detail(&error).map(str::to_owned);
                            (executor_failure(&code), outcome(&code))
                        }
                    }
                };
                drop((sql_slot, sql_guard));
                Timed {
                    response,
                    queue,
                    initialization: run.initialization,
                    execution: began.elapsed().saturating_sub(run.initialization),
                    cold: run.cold,
                    outcome: result_outcome,
                    error_code: failure_code,
                    error_detail: failure_detail,
                }
            });
            match tokio::time::timeout_at(deadline, task).await {
                Ok(Ok(timed)) if Instant::now() < budget.deadline() => timed,
                Ok(Err(_)) if Instant::now() < budget.deadline() => {
                    let progress = progress.lock().unwrap_or_else(|error| error.into_inner());
                    interrupted(&budget, "function.executor-failed", started, &progress)
                }
                _ => {
                    let progress = progress.lock().unwrap_or_else(|error| error.into_inner());
                    schedule_abort(budget.clone());
                    interrupted(&budget, "deadline_exceeded", started, &progress)
                }
            }
        }
    };
    Ok(record(
        state,
        &deployment,
        name,
        &budget,
        started,
        timed,
        &trace,
    ))
}

static BUNDLE_CACHE: OnceLock<
    Mutex<std::collections::HashMap<String, Arc<std::collections::BTreeMap<String, String>>>>,
> = OnceLock::new();

fn pinned_bundle(
    state: &AppState,
    deployment: &Deployment,
    name: &str,
) -> anyhow::Result<Arc<std::collections::BTreeMap<String, String>>> {
    let published = deployment
        .server
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing published metadata"))?;
    anyhow::ensure!(
        published.hashes.contains_key(name),
        "Missing published bundle hash"
    );
    let server_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(published)?));
    let version_key = format!("{}:{}:", deployment.id, deployment.artifact_version);
    let cache_key = format!("{version_key}{server_digest}");
    let server = crate::layout::server(&state.data_root, &deployment.id);
    let disk_metadata: crate::functions::Published =
        serde_json::from_slice(&std::fs::read(server.join("manifest.json"))?)?;
    let disk_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&disk_metadata)?));
    anyhow::ensure!(
        disk_digest == server_digest,
        "Published server artifact changed"
    );
    let cache = BUNDLE_CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(bundles) = cache.lock().unwrap().get(&cache_key).cloned() {
        anyhow::ensure!(bundles.contains_key(name), "Missing function bundle");
        return Ok(bundles);
    }
    let (artifact, checked) = crate::functions::read_artifact_with_metadata(&server)?
        .ok_or_else(|| anyhow::anyhow!("Missing server artifact"))?;
    anyhow::ensure!(&checked == published, "Published server artifact changed");
    let source = Arc::new(artifact.bundles);
    let mut cache = cache.lock().unwrap();
    cache.retain(|key, _| {
        !key.starts_with(&format!("{}:", deployment.id)) || key.starts_with(&version_key)
    });
    cache.insert(cache_key, source.clone());
    anyhow::ensure!(source.contains_key(name), "Missing function bundle");
    Ok(source)
}
fn respond(output: Output) -> Response {
    let mut response = Response::new(Body::from(output.body));
    *response.status_mut() =
        StatusCode::from_u16(output.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in output.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            response.headers_mut().append(name, value);
        }
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
