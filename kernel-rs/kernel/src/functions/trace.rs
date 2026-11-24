use axum::http::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        mpsc::{SyncSender, TrySendError},
    },
    time::Instant,
};

pub(super) const TRACEPARENT: &str = "traceparent";
pub(super) const MAX_RECORDS: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TraceContext {
    pub trace_id: [u8; 16],
    pub parent_id: [u8; 8],
    pub sampled: bool,
}

impl TraceContext {
    pub(crate) fn from_headers(headers: &HeaderMap, sampled: bool) -> Self {
        let parsed = if headers.get_all(TRACEPARENT).iter().count() == 1 {
            headers
                .get(TRACEPARENT)
                .and_then(|value| value.to_str().ok())
                .and_then(parse)
        } else {
            None
        };
        let mut context = parsed.unwrap_or_else(Self::new);
        context.sampled = sampled;
        context
    }

    pub fn new() -> Self {
        let trace_id = *uuid::Uuid::new_v4().as_bytes();
        let parent_id: [u8; 8] = uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap();
        Self {
            trace_id,
            parent_id,
            sampled: false,
        }
    }

    pub(crate) fn child(&self) -> [u8; 8] {
        uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap()
    }

    pub(crate) fn header(&self, span_id: [u8; 8]) -> HeaderValue {
        let text = format!(
            "00-{}-{}-{:02x}",
            hex(&self.trace_id),
            hex(&span_id),
            u8::from(self.sampled)
        );
        HeaderValue::from_str(&text).expect("traceparent contains only lowercase hex")
    }
}

fn parse(value: &str) -> Option<TraceContext> {
    let bytes = value.as_bytes();
    if bytes.len() != 55 || &bytes[..3] != b"00-" || bytes[35] != b'-' || bytes[52] != b'-' {
        return None;
    }
    let trace_id = decode::<16>(&bytes[3..35])?;
    let parent_id = decode::<8>(&bytes[36..52])?;
    let flags = decode::<1>(&bytes[53..55])?[0];
    if trace_id == [0; 16] || parent_id == [0; 8] {
        return None;
    }
    Some(TraceContext {
        trace_id,
        parent_id,
        sampled: flags & 1 != 0,
    })
}

fn decode<const N: usize>(bytes: &[u8]) -> Option<[u8; N]> {
    if bytes.len() != N * 2
        || !bytes.iter().all(u8::is_ascii_hexdigit)
        || bytes.iter().any(u8::is_ascii_uppercase)
    {
        return None;
    }
    let mut result = [0; N];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        result[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(result)
}

fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

#[derive(Clone, Debug)]
pub(super) struct TraceRecord {
    pub parent: Option<u8>,
    pub name: &'static str,
    pub start_us: u64,
    pub duration_us: u64,
    pub action_name: Option<String>,
    pub origin: Option<&'static str>,
    pub sequence: Option<u16>,
    pub rows_read: Option<u32>,
    pub rows_written: Option<u32>,
    pub reply_bytes: Option<u32>,
    pub sqlite_us: Option<u64>,
    pub error_code: Option<String>,
}

#[derive(Clone)]
pub(super) struct TraceBuffer(Arc<Inner>);

struct Inner {
    started: Instant,
    context: TraceContext,
    runner_kind: &'static str,
    response_span: [u8; 8],
    records: Mutex<Vec<TraceRecord>>,
    truncated: Mutex<bool>,
    completion: Mutex<CompletionState>,
    hook: Mutex<Option<Arc<dyn Fn(CompletedTrace) + Send + Sync>>>,
}

#[derive(Default)]
struct CompletionState {
    pending_native: usize,
    http: Option<(u16, String)>,
    completed: bool,
}

impl TraceBuffer {
    pub fn new(context: TraceContext, started: Instant, runner_kind: &'static str) -> Self {
        Self(Arc::new(Inner {
            started,
            context,
            runner_kind,
            response_span: context.child(),
            records: Mutex::new(vec![TraceRecord {
                parent: None,
                name: "konae.http.server",
                start_us: 0,
                duration_us: 0,
                action_name: None,
                origin: None,
                sequence: None,
                rows_read: None,
                rows_written: None,
                reply_bytes: None,
                sqlite_us: None,
                error_code: None,
            }]),
            truncated: Mutex::new(false),
            completion: Mutex::new(CompletionState::default()),
            hook: Mutex::new(None),
        }))
    }

    pub fn response_header(&self) -> HeaderValue {
        self.0.context.header(self.0.response_span)
    }
    pub fn elapsed_us(&self) -> u64 {
        self.0.started.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
    pub fn set_root_error(&self, code: Option<&str>) {
        if let Some(root) = self
            .0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .first_mut()
        {
            root.error_code = code.map(str::to_owned);
        }
    }

    pub fn push(&self, record: TraceRecord) -> Option<u8> {
        let mut records = self
            .0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if records.len() < MAX_RECORDS - 2 {
            let index = records.len() as u8;
            records.push(record);
            Some(index)
        } else {
            *self
                .0
                .truncated
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = true;
            None
        }
    }

    pub fn update(&self, index: u8, record: TraceRecord) {
        if let Some(slot) = self
            .0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(index as usize)
        {
            *slot = record;
        }
    }

    pub fn retain_native(&self) {
        self.0
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending_native += 1;
    }
    pub fn native_finished(&self) {
        let mut state = self
            .0
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.pending_native = state.pending_native.saturating_sub(1);
        drop(state);
        self.try_complete();
    }
    pub fn set_completion_hook(&self, hook: Arc<dyn Fn(CompletedTrace) + Send + Sync>) {
        *self
            .0
            .hook
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(hook);
        self.try_complete();
    }
    pub fn finish(&self, status: u16, outcome: &'static str) {
        if let Some(root) = self
            .0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .first_mut()
        {
            root.duration_us = self.elapsed_us();
        }
        self.0
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .http = Some((status, outcome.to_owned()));
        self.try_complete();
    }
    fn try_complete(&self) {
        let (status, outcome) = {
            let mut state = self
                .0
                .completion
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.completed || state.pending_native != 0 {
                return;
            }
            let Some(http) = state.http.clone() else {
                return;
            };
            let hook_ready = self
                .0
                .hook
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some();
            if !hook_ready {
                return;
            }
            state.completed = true;
            http
        };
        let mut records = self
            .0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let completed = CompletedTrace {
            context: self.0.context,
            runner_kind: self.0.runner_kind,
            response_span: self.0.response_span,
            records: std::mem::take(&mut *records),
            truncated: *self
                .0
                .truncated
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            status,
            outcome,
        };
        drop(records);
        if let Some(hook) = self
            .0
            .hook
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            hook(completed);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompletedTrace {
    pub(super) context: TraceContext,
    pub(super) runner_kind: &'static str,
    pub(super) response_span: [u8; 8],
    pub(super) records: Vec<TraceRecord>,
    pub(super) truncated: bool,
    pub(super) status: u16,
    pub(super) outcome: String,
}

impl CompletedTrace {
    pub(crate) fn local_json(&self, at: u64) -> Value {
        let spans = self.records.iter().map(|row| json!({
            "parent": row.parent, "name": row.name, "startUs": row.start_us, "durationUs": row.duration_us,
            "action": row.action_name, "origin": row.origin, "sequence": row.sequence,
            "rowsRead": row.rows_read, "rowsWritten": row.rows_written, "replyBytes": row.reply_bytes,
            "sqliteUs": row.sqlite_us, "errorCode": row.error_code,
        })).collect::<Vec<_>>();
        json!({"at":at,"traceId":hex(&self.context.trace_id),"spanId":hex(&self.response_span),"runnerKind":self.runner_kind,"status":self.status,"outcome":self.outcome,"truncated":self.truncated,"spans":spans})
    }
}

static COMPLETED_SINK: OnceLock<RwLock<Option<SyncSender<CompletedTrace>>>> = OnceLock::new();

#[allow(dead_code)]
pub(super) fn set_completed_sink(sink: SyncSender<CompletedTrace>) {
    *COMPLETED_SINK
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|error| error.into_inner()) = Some(sink);
}

pub(super) fn submit_completed(trace: CompletedTrace) {
    if let Some(sink) = COMPLETED_SINK
        .get()
        .and_then(|value| value.read().ok()?.clone())
    {
        match sink.try_send(trace) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

pub(super) fn attach_response(response: &mut axum::response::Response, trace: &TraceBuffer) {
    response
        .headers_mut()
        .insert(TRACEPARENT, trace.response_header());
}

pub(crate) fn attach_request_response(
    headers: &HeaderMap,
    response: &mut axum::response::Response,
) {
    let context = TraceContext::from_headers(headers, false);
    response
        .headers_mut()
        .insert(TRACEPARENT, context.header(context.child()));
}

#[cfg(test)]
mod function_trace_tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn traceparent_accepts_one_valid_version_zero_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TRACEPARENT,
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        let context = TraceContext::from_headers(&headers, true);
        assert_eq!(hex(&context.trace_id), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(hex(&context.parent_id), "00f067aa0ba902b7");
        assert!(context.sampled);
    }

    #[test]
    fn incoming_sampling_is_cleared_without_local_capture() {
        let mut headers = HeaderMap::new();
        headers.insert(
            TRACEPARENT,
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        assert!(!TraceContext::from_headers(&headers, false).sampled);
    }

    #[test]
    fn malformed_and_duplicate_headers_start_a_new_trace() {
        for value in [
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(TRACEPARENT, HeaderValue::from_str(value).unwrap());
            assert_ne!(
                TraceContext::from_headers(&headers, false).trace_id,
                decode::<16>(b"4bf92f3577b34da6a3ce929d0e0e4736").unwrap()
            );
        }
        let mut headers = HeaderMap::new();
        headers.append(
            TRACEPARENT,
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        headers.append(
            TRACEPARENT,
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        assert_ne!(
            TraceContext::from_headers(&headers, false).trace_id,
            decode::<16>(b"4bf92f3577b34da6a3ce929d0e0e4736").unwrap()
        );
    }

    #[test]
    fn response_gets_a_distinct_server_span() {
        let context = TraceContext::new();
        let trace = TraceBuffer::new(context, Instant::now(), "deno");
        assert_ne!(trace.0.response_span, context.parent_id);
        assert!(trace.response_header().to_str().unwrap().ends_with("-00"));
    }

    #[test]
    fn function_trace_caps_records_and_serializes_only_allowlisted_values() {
        let trace = TraceBuffer::new(TraceContext::new(), Instant::now(), "deno");
        for sequence in 0..MAX_RECORDS + 20 {
            let _ = trace.push(TraceRecord {
                parent: Some(0),
                name: "konae.action",
                start_us: sequence as u64,
                duration_us: 5,
                action_name: Some("<unknown>".into()),
                origin: Some("function"),
                sequence: Some(sequence as u16),
                rows_read: None,
                rows_written: None,
                reply_bytes: None,
                sqlite_us: None,
                error_code: Some("function.action-refused".into()),
            });
        }
        let trace_id = trace.0.context.trace_id;
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        trace.set_completion_hook(Arc::new(move |completed| {
            let _ = sender.send(completed);
        }));
        trace.finish(403, "refused");
        let completed = receiver.recv().unwrap();
        let value = completed.local_json(1_800_000_000_000);
        assert_eq!(value["spans"].as_array().unwrap().len(), MAX_RECORDS - 2);
        assert!(completed.truncated);
        assert_eq!(value["status"], 403);
        assert_eq!(value["runnerKind"], "deno");
        let text = value.to_string();
        assert!(!text.contains("fixture-data"));
        assert!(!text.contains("SQL text"));
        assert!(!text.contains("exception detail"));
        assert_eq!(value["traceId"], hex(&trace_id));
    }

    #[test]
    fn function_trace_waits_for_late_native_completion_after_http_response() {
        let trace = TraceBuffer::new(TraceContext::new(), Instant::now(), "deno");
        trace.retain_native();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        trace.set_completion_hook(Arc::new(move |completed| {
            let _ = sender.send(completed);
        }));
        trace.finish(409, "mutation_outcome_unknown");
        assert!(receiver.try_recv().is_err());
        let _ = trace.push(TraceRecord {
            parent: Some(0),
            name: "konae.transaction",
            start_us: 20,
            duration_us: 5,
            action_name: Some("committed".into()),
            origin: None,
            sequence: None,
            rows_read: None,
            rows_written: None,
            reply_bytes: None,
            sqlite_us: None,
            error_code: None,
        });
        trace.native_finished();
        let completed = receiver.recv().unwrap();
        assert_eq!(completed.status, 409);
        assert!(
            completed
                .records
                .iter()
                .any(|record| record.name == "konae.transaction")
        );
    }
}
