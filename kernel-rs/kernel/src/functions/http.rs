use crate::{
    AppState, Deployment, Operation,
    usage::{FunctionOutcome, FunctionTiming},
};
use super::{Function, invocation as function_data, runtime::RunProgress};
use super::invocation::{Admission, InvocationConfig};
use axum::{
    Json,
    body::Body,
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
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
const MAX_LEGACY_FUNCTION_BODY_BYTES: usize = 16 * 1024;
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
    fn channel(complete: bool) -> (tokio::sync::mpsc::Sender<BodyFrame>, Arc<Self>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let ingress = Arc::new(Self {
            receiver: Mutex::new(receiver),
            state: Mutex::new(BodyState {
                length: 0,
                digest: Sha256::new(),
                complete,
                error: None,
            }),
        });
        (sender, ingress)
    }

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
    let (sender, ingress) = IngressBody::channel(false);
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

async fn read_replay_body(
    body: Body,
    legacy: bool,
    budget: &Budget,
) -> Result<
    (
        usize,
        [u8; 32],
        Option<Vec<u8>>,
        Vec<tokio::sync::OwnedSemaphorePermit>,
    ),
    Response,
> {
    let mut incoming = body.into_data_stream();
    let mut length = 0usize;
    let mut digest = Sha256::new();
    let mut legacy_bytes = legacy.then(Vec::new);
    let mut permits = Vec::new();
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
        for bytes in chunk.chunks(konae_functions_executor::protocol::BODY_CHUNK_BYTES) {
            if length.saturating_add(bytes.len()) > MAX_FUNCTION_BODY_BYTES {
                return Err(failure(
                    413,
                    "body_too_large",
                    "The function request exceeds 100000000 bytes.",
                ));
            }
            let next_length = length + bytes.len();
            if legacy_bytes.is_some() && next_length > MAX_LEGACY_FUNCTION_BODY_BYTES {
                legacy_bytes = None;
                permits.clear();
            }
            length = next_length;
            digest.update(bytes);
            let permit = function_body_budget()
                .try_acquire_many_owned(bytes.len() as u32)
                .map_err(|_| executor_failure("function.capacity"))?;
            if let Some(legacy_bytes) = &mut legacy_bytes {
                permits.push(permit);
                legacy_bytes.extend_from_slice(bytes);
            }
        }
    }
    Ok((length, digest.finalize().into(), legacy_bytes, permits))
}

struct Cancel(Budget);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct Timed {
    response: Response,
    queue: Duration,
    initialization: Duration,
    execution: Duration,
    cold: bool,
    outcome: FunctionOutcome,
}
struct PreparedRun {
    lease: super::runtime::Lease,
    config: InvocationConfig,
    input: Input,
    body: Arc<IngressBody>,
    body_pump: Option<BodyPump>,
}

fn acquire_action_resources(
    state: &AppState,
    deployment: &str,
    method: &str,
    budget: &Budget,
) -> Result<(crate::limits::StatementPermit, crate::ActionLock), &'static str> {
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
impl Timed {
    fn refusal(response: Response, elapsed: Duration) -> Self {
        Self {
            response,
            queue: elapsed,
            initialization: Duration::ZERO,
            execution: Duration::ZERO,
            cold: false,
            outcome: FunctionOutcome::Refused,
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
    let legacy = !stored_fingerprint.starts_with("v3:");
    let (body_len, body_digest, legacy_body, _permits) =
        read_replay_body(body, legacy, budget).await?;
    if legacy && legacy_body.is_none() {
        return Err(failure(
            409,
            "idempotency_conflict",
            "Idempotency-Key was already used with a different request.",
        ));
    }
    let fingerprint = if legacy {
        function_data::request_fingerprint(
            method,
            path,
            legacy_body.as_deref().unwrap_or_default(),
            headers,
        )
    } else {
        function_data::request_fingerprint_v3(method, path, body_len, &body_digest, headers)
    }
    .map_err(|error| failure(error.status, error.code, &error.message))?;
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
    }))
}

#[derive(Clone)]
pub(super) struct FunctionFailureCode(pub String);

fn failure(status: u16, code: &str, message: &str) -> Response {
    let mut response = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(json!({"error":message,"code":code})),
    )
        .into_response();
    response.extensions_mut().insert(FunctionFailureCode(code.to_owned()));
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

fn interrupted(budget: &Budget, code: &str, started: Instant, progress: &RunProgress) -> Timed {
    let code = if budget.cancel().may_have_committed() {
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
    }
}

fn record(
    state: &AppState,
    deployment: &Deployment,
    name: String,
    budget: &Budget,
    started: Instant,
    timed: Timed,
) -> Response {
    if timed.response.extensions().get::<CronSkipped>().is_some() {
        return timed.response;
    }
    let now = Instant::now();
    crate::usage::function_invocation(
        &state.data_root,
        &deployment.id,
        FunctionTiming {
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
        },
    );
    timed.response
}
fn declaration(deployment: &Deployment, path: &str) -> Option<(String, Function)> {
    deployment
        .server
        .as_ref()?
        .manifest
        .functions
        .iter()
        .find(|(_, f)| {
            f.access != "scheduled" && (path == f.mount
                || path
                    .strip_prefix(&f.mount)
                    .is_some_and(|rest| rest.starts_with('/')))
        })
        .map(|(name, function)| (name.clone(), function.clone()))
}

async fn http_actor(
    state: &AppState,
    deployment: &Deployment,
    function: &Function,
    method: &Method,
    headers: &HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> Result<Option<String>, Response> {
    if !function
        .methods
        .iter()
        .any(|allowed| allowed == method.as_str())
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
    if !crate::same_origin(headers)
        || crate::path::foreign_referer(state, headers, &deployment.id)
    {
        return Err(failure(
            403,
            "function.origin-refused",
            "Cross-origin function request rejected.",
        ));
    }
    if crate::runtime::bearer(headers).is_some_and(|token| token.starts_with("lbk_"))
    {
        return Err(failure(
            403,
            "function.key-refused",
            "Functions do not accept API keys.",
        ));
    }
    let actor = if crate::modules::is_on(&deployment.modules, "auth") {
        crate::authenticate_deployment(state, deployment, headers).await?
    } else {
        None
    };
    if (crate::is_private(deployment) || function.access == "authenticated") && actor.is_none()
    {
        return Err(failure(
            401,
            "authentication-required",
            "authentication-required",
        ));
    }
    let client = state.guards.client_ip(peer, headers);
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
    Ok(actor.map(|user| user.user_id))
}

#[derive(Clone, Copy)]
pub(super) enum Trigger<'a> {
    Http,
    Cron { id: &'a str, at: u64, manual: bool },
}

#[derive(Clone)]
pub(super) struct CronSkipped;

/// A function request holds the publication lease until native work and its receipt finish.
pub(crate) async fn dispatch(state: &AppState, request: Request) -> Result<Response, Request> {
    let Some(observed) = crate::deployment_for(state, request.headers()).await else {
        return Err(request);
    };
    let Some((observed_name, _)) = declaration(&observed, request.uri().path()) else {
        return Err(request);
    };
    invoke(state, &observed, observed_name, request, Trigger::Http).await
}

pub(super) async fn invoke(
    state: &AppState,
    observed: &Deployment,
    observed_name: String,
    request: Request,
    trigger: Trigger<'_>,
) -> Result<Response, Request> {
    let started = Instant::now();
    let budget = Budget::until(started + state.functions.deadline);
    let _cancel = Cancel(budget.clone());
    let deadline = tokio::time::Instant::from_std(budget.deadline());
    let admission = if matches!(trigger, Trigger::Http) {
        match state.functions.admit(&budget) {
            Ok(admission) => Some(admission),
            Err(code) => {
                let timed = Timed::refusal(executor_failure(&code), started.elapsed());
                return Ok(record(state, observed, observed_name, &budget, started, timed));
            }
        }
    } else {
        None
    };
    let skip_cron = || (StatusCode::NO_CONTENT, axum::Extension(CronSkipped)).into_response();
    let (generation, held, deployment) = match tokio::time::timeout_at(deadline, async {
        let generation = state.copies.generations.clone().read_owned().await;
        let held = crate::named_lock(state, &format!("publish:{}", observed.id), false).await;
        let deployment = crate::deployment_by_id(state, &observed.id).await;
        (generation, held, deployment)
    })
    .await
    {
        Ok(held) => held,
        Err(_) if matches!(trigger, Trigger::Cron { manual: false, .. }) => return Ok(skip_cron()),
        Err(_) => {
            let timed = interrupted(
                &budget,
                "deadline_exceeded",
                started,
                &RunProgress::default(),
            );
            return Ok(record(
                state,
                observed,
                observed_name,
                &budget,
                started,
                timed,
            ));
        }
    };
    let Some(deployment) = deployment else {
        return Ok(match trigger {
            Trigger::Http => failure(404, "function.not-found", "Function not found."),
            Trigger::Cron { .. } => skip_cron(),
        });
    };
    let (name, function) = match trigger {
        Trigger::Http => match declaration(&deployment, request.uri().path()) {
            Some(function) => function,
            None => return Err(request),
        },
        Trigger::Cron { id, at, manual } => {
            let Some(server) = &deployment.server else { return Ok(skip_cron()) };
            let Some(cron) = server.manifest.crons.get(id) else { return Ok(skip_cron()) };
            if deployment.artifact_version != observed.artifact_version
                || state.execution_mode.is_test() != manual
                || (!manual && !super::cron::released_before_slot(&deployment, at))
                || crate::maintenance::pause(&state.data_root, &deployment.id).is_some()
                || !crate::modules::is_on(&deployment.modules, "functions")
            {
                return Ok(skip_cron());
            }
            let Some(function) = server.manifest.functions.get(&cron.function).filter(|fn_def| fn_def.access == "scheduled") else {
                return Ok(skip_cron());
            };
            (cron.function.clone(), function.clone())
        }
    };
    let bundle_hash = deployment.server.as_ref().unwrap().hashes[&name].clone();
    let operation = request
        .extensions()
        .get::<Operation>()
        .map(|operation| operation.0.clone());
    if let Trigger::Cron { id, at, manual: false } = trigger {
        let folder = crate::layout::data(&state.data_root, &deployment.id);
        let name = super::cron::run_name(id);
        let operation = operation.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _operation = operation;
            crate::usage::claim_cron(&folder, &name, super::cron::slot(at))
        });
        match task.await {
            Ok(Ok(true)) => {}
            Ok(Ok(false)) => return Ok(skip_cron()),
            _ => return Ok(failure(503, "cron.claim-failed", "The Kernel could not record the cron attempt.")),
        }
    }
    let prepare = async {
        if let Some(message) = crate::maintenance::pause(&state.data_root, &deployment.id) {
            return Err(crate::maintenance::refusal(&message));
        }
        crate::modules::require_on(&deployment.modules, "functions")
            .map_err(|refused| crate::refusal_error(&refused))?;
        let actor_id = match trigger {
            Trigger::Http => {
                let peer = request.extensions()
                    .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                    .map(|value| value.0);
                http_actor(state, &deployment, &function, request.method(), request.headers(), peer).await?
            }
            Trigger::Cron { .. } => None,
        };
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
            "{}:{}:{}:{bundle_hash}:{}",
            deployment.id,
            name,
            deployment.artifact_version,
            konae_functions_executor::protocol::ABI
        );
        let admission = match admission {
            Some(admission) => admission,
            None => state.functions.admit(&budget).map_err(|code| executor_failure(&code))?,
        };
        let lease = state
            .functions
            .lease_admitted(cache_key, &budget, admission)
            .await
            .map_err(|code| executor_failure(&code))?;
        let (body, body_pump) = match trigger {
            Trigger::Http => {
                let (body, pump) = start_body_pump(request_body.unwrap(), budget.clone());
                (body, Some(pump))
            }
            Trigger::Cron { .. } => (IngressBody::channel(true).1, None),
        };
        let program = Arc::new(crate::action_shape::read(&deployment.actions).map_err(|_| {
            failure(
                500,
                "function.program-invalid",
                "The published actions cannot be read.",
            )
        })?);
        let config = InvocationConfig {
            app_id: deployment.id.clone(),
            release_id: deployment.artifact_version.clone(),
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
            let task = tokio::task::spawn_blocking(move || {
                let budget = worker_budget;
                let _operation = operation;
                let _generation = generation;
                let _held = held;
                let mut sql_slot = None;
                let mut sql_guard = None;
                let bundle = match pinned_bundle(&state, &deployment, &name) {
                    Ok(directory) => directory,
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
                let request_body: Arc<dyn konae_functions_executor::RequestBody> = body.clone();
                let action_body = body.clone();
                let run = lease.run_stream(&name, &bundle.0, input, request_body, &budget, &mut |operation, args| {
                    if operation != "action" {
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
                        return Err("function.input-invalid".into());
                    };
                    match invocation.as_mut().unwrap().action(action, args.get("input").unwrap_or(&Value::Null)) {
                        Ok(answer) => Ok(answer),
                        Err(error) => {
                            let code = error.code.to_owned();
                            native_failure = Some(error);
                            Err(code)
                        }
                    }
                });
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
                    };
                }
                if run.output.is_err() && invocation.is_none() && replayed.is_none() {
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
                    };
                }
                let mut invocation = invocation.unwrap();
                let (response, result_outcome) = match run.output {
                    Ok(mut output) => {
                        output.headers.retain(|(name, _)| {
                            ["content-type", "location", "retry-after"]
                                .contains(&name.to_ascii_lowercase().as_str())
                        });
                        match invocation.finish(output) {
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
                            Err(error) => (
                                failure(error.status, error.code, &error.message),
                                outcome(error.code),
                            ),
                        }
                    }
                    Err(error) => {
                        let _ = invocation.abort();
                        if let Some(error) = native_failure {
                            (
                                failure(error.status, error.code, &error.message),
                                outcome(error.code),
                            )
                        } else {
                            let code =
                                budget.failure_code().map(str::to_owned).unwrap_or_else(|| {
                                    konae_functions_executor::error_code(&error).to_owned()
                                });
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
                    interrupted(&budget, "deadline_exceeded", started, &progress)
                }
            }
        }
    };
    Ok(record(state, &deployment, name, &budget, started, timed))
}

struct Bundle(PathBuf);
impl Drop for Bundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn pinned_bundle(state: &AppState, deployment: &Deployment, name: &str) -> anyhow::Result<Bundle> {
    let (artifact, metadata) = crate::functions::read_artifact_with_metadata(
        &crate::layout::server(&state.data_root, &deployment.id),
    )?
    .ok_or_else(|| anyhow::anyhow!("Missing server artifact"))?;
    anyhow::ensure!(
        Some(metadata) == deployment.server,
        "Published server artifact changed"
    );
    let directory = crate::layout::private(&state.data_root, &deployment.id)
        .join(format!("function-input-{}", uuid::Uuid::new_v4().simple()));
    let bundle = Bundle(directory);
    std::fs::create_dir_all(&bundle.0)?;
    std::fs::write(
        bundle.0.join(format!("{name}.mjs")),
        &artifact.bundles[name],
    )?;
    Ok(bundle)
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
