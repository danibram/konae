use axum::http::{HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

pub(crate) const TRACEPARENT: &str = "traceparent";
pub(super) const MAX_RECORDS: usize = 160;
const MAX_INTERNED_NAMES: usize = MAX_RECORDS;
const MAX_NAME_BYTES: usize = 100;
const NO_NAME: u8 = u8::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TraceContext {
    pub trace_id: [u8; 16],
    pub parent_id: [u8; 8],
    pub sampled: bool,
    response_seed: Option<[u8; 8]>,
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
        let parent_id = trace_id[..8].try_into().unwrap();
        let response_seed = Some(trace_id[8..].try_into().unwrap());
        Self {
            trace_id,
            parent_id,
            sampled: false,
            response_seed,
        }
    }

    pub(crate) fn child(&self) -> [u8; 8] {
        self.response_seed
            .unwrap_or_else(|| uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap())
    }

    pub(crate) fn header(&self, span_id: [u8; 8]) -> HeaderValue {
        let mut bytes = [0; 55];
        bytes[..3].copy_from_slice(b"00-");
        hex_into(&mut bytes[3..35], &self.trace_id);
        bytes[35] = b'-';
        hex_into(&mut bytes[36..52], &span_id);
        bytes[52] = b'-';
        bytes[53] = b'0';
        bytes[54] = if self.sampled { b'1' } else { b'0' };
        HeaderValue::from_bytes(&bytes).expect("traceparent contains only lowercase hex")
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
        response_seed: None,
    })
}

pub(crate) fn child_traceparent(value: &str) -> Option<String> {
    let context = parse(value)?;
    Some(context.header(context.child()).to_str().ok()?.to_owned())
}

pub(crate) fn traceparent_id(value: &str) -> Option<[u8; 16]> {
    Some(parse(value)?.trace_id)
}

pub(crate) fn traceparent_sampled(value: &str) -> Option<bool> {
    Some(parse(value)?.sampled)
}

pub(crate) fn normalize_traceparent(value: &str, sampled: bool) -> Option<String> {
    let mut context = parse(value)?;
    context.sampled = sampled;
    Some(context.header(context.parent_id).to_str().ok()?.to_owned())
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

fn hex_into(output: &mut [u8], bytes: &[u8]) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    debug_assert_eq!(output.len(), bytes.len() * 2);
    for (index, byte) in bytes.iter().enumerate() {
        output[index * 2] = DIGITS[(byte >> 4) as usize];
        output[index * 2 + 1] = DIGITS[(byte & 15) as usize];
    }
}

#[derive(Clone, Copy)]
pub(super) struct TraceRecord<'a> {
    pub parent: Option<u8>,
    pub name: &'static str,
    pub start_us: u64,
    pub duration_us: u64,
    pub action_name: Option<&'a str>,
    pub origin: Option<&'static str>,
    pub sequence: Option<u16>,
    pub rows_read: Option<u32>,
    pub rows_written: Option<u32>,
    pub reply_bytes: Option<u32>,
    pub sqlite_us: Option<u64>,
    pub error_code: Option<&'a str>,
}

#[derive(Clone, Copy)]
struct InternedName {
    bytes: [u8; MAX_NAME_BYTES],
    len: u8,
}

impl InternedName {
    const EMPTY: Self = Self {
        bytes: [0; MAX_NAME_BYTES],
        len: 0,
    };

    fn new(value: &str) -> Self {
        let mut len = value.len().min(MAX_NAME_BYTES);
        while !value.is_char_boundary(len) {
            len -= 1;
        }
        let mut name = Self::EMPTY;
        name.bytes[..len].copy_from_slice(&value.as_bytes()[..len]);
        name.len = len as u8;
        name
    }

    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len as usize]).expect("interned UTF-8 name")
    }
}

#[derive(Clone, Copy)]
struct StoredRecord {
    parent: Option<u8>,
    name: &'static str,
    start_us: u64,
    duration_us: u64,
    action_name: u8,
    origin: Option<&'static str>,
    sequence: Option<u16>,
    rows_read: Option<u32>,
    rows_written: Option<u32>,
    reply_bytes: Option<u32>,
    sqlite_us: Option<u64>,
    error_code: u8,
}

impl StoredRecord {
    const EMPTY: Self = Self {
        parent: None,
        name: "",
        start_us: 0,
        duration_us: 0,
        action_name: NO_NAME,
        origin: None,
        sequence: None,
        rows_read: None,
        rows_written: None,
        reply_bytes: None,
        sqlite_us: None,
        error_code: NO_NAME,
    };

    fn new(record: TraceRecord<'_>, action_name: u8, error_code: u8) -> Self {
        Self {
            parent: record.parent,
            name: record.name,
            start_us: record.start_us,
            duration_us: record.duration_us,
            action_name,
            origin: record.origin,
            sequence: record.sequence,
            rows_read: record.rows_read,
            rows_written: record.rows_written,
            reply_bytes: record.reply_bytes,
            sqlite_us: record.sqlite_us,
            error_code,
        }
    }
}

pub(super) struct Recorder {
    records: [StoredRecord; MAX_RECORDS],
    record_count: usize,
    names: [InternedName; MAX_INTERNED_NAMES],
    name_count: usize,
    truncated: bool,
}

impl Recorder {
    fn new() -> Self {
        Self {
            records: [StoredRecord::EMPTY; MAX_RECORDS],
            record_count: 0,
            names: [InternedName::EMPTY; MAX_INTERNED_NAMES],
            name_count: 0,
            truncated: false,
        }
    }

    fn intern(&mut self, value: Option<&str>) -> u8 {
        let Some(value) = value else { return NO_NAME };
        for (index, name) in self.names[..self.name_count].iter().enumerate() {
            if name.as_str() == value {
                return index as u8;
            }
        }
        if self.name_count == MAX_INTERNED_NAMES {
            self.truncated = true;
            return NO_NAME;
        }
        let id = self.name_count as u8;
        self.names[self.name_count] = InternedName::new(value);
        self.name_count += 1;
        id
    }

    fn push(&mut self, record: TraceRecord<'_>) -> Option<u8> {
        if self.record_count >= MAX_RECORDS - 2 {
            self.truncated = true;
            return None;
        }
        let action_name = self.intern(record.action_name);
        let error_code = self.intern(record.error_code);
        let index = self.record_count as u8;
        self.records[self.record_count] = StoredRecord::new(record, action_name, error_code);
        self.record_count += 1;
        Some(index)
    }

    fn update(&mut self, index: u8, record: TraceRecord<'_>) {
        if index as usize >= self.record_count {
            return;
        }
        let action_name = self.intern(record.action_name);
        let error_code = self.intern(record.error_code);
        self.records[index as usize] = StoredRecord::new(record, action_name, error_code);
    }

    fn name(&self, id: u8) -> Option<&str> {
        (id != NO_NAME)
            .then(|| self.names.get(id as usize).map(InternedName::as_str))
            .flatten()
    }
}

#[derive(Clone)]
pub(crate) struct TraceBuffer(Arc<Inner>);

struct Inner {
    started: Instant,
    context: TraceContext,
    runner_kind: &'static str,
    response_span: [u8; 8],
    records: Mutex<Recorder>,
    completion: Mutex<CompletionState>,
    exporter: Option<super::otlp::Exporter>,
    #[cfg(test)]
    hook: Mutex<Option<Box<dyn FnOnce(CompletedTrace) + Send>>>,
}

#[derive(Default)]
struct CompletionState {
    pending_native: usize,
    http: Option<(u16, &'static str)>,
    completed: bool,
}

impl TraceBuffer {
    #[cfg(test)]
    pub fn new(context: TraceContext, started: Instant, runner_kind: &'static str) -> Self {
        Self::new_with_exporter(context, started, runner_kind, None)
    }

    pub(crate) fn new_with_exporter(
        context: TraceContext,
        started: Instant,
        runner_kind: &'static str,
        exporter: Option<super::otlp::Exporter>,
    ) -> Self {
        Self(Arc::new(Inner {
            started,
            context,
            runner_kind,
            response_span: context.child(),
            records: Mutex::new({
                let mut recorder = Recorder::new();
                recorder.push(TraceRecord {
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
                });
                recorder
            }),
            completion: Mutex::new(CompletionState::default()),
            exporter,
            #[cfg(test)]
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
        let mut records = self
            .0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if records.record_count > 0 {
            records.records[0].error_code = records.intern(code);
        }
    }

    pub(super) fn push(&self, record: TraceRecord<'_>) -> Option<u8> {
        self.0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(record)
    }

    pub(super) fn update(&self, index: u8, record: TraceRecord<'_>) {
        self.0
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .update(index, record);
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
    #[cfg(test)]
    pub fn set_completion_hook(&self, hook: Box<dyn FnOnce(CompletedTrace) + Send>) {
        *self
            .0
            .hook
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(hook);
        self.try_complete();
    }
    pub fn finish(&self, status: u16, outcome: &'static str) {
        {
            let mut records = self
                .0
                .records
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if records.record_count > 0 {
                records.records[0].duration_us = self.elapsed_us();
            }
        }
        self.0
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .http = Some((status, outcome));
        self.try_complete();
    }
    fn try_complete(&self) {
        let mut state = self
            .0
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.completed || state.pending_native != 0 {
            return;
        }
        if state.http.is_none() {
            return;
        }
        let exporter = self.0.exporter.clone();
        #[cfg(test)]
        let test_hook = self
            .0
            .hook
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        #[cfg(not(test))]
        let test_hook: Option<Box<dyn FnOnce(CompletedTrace) + Send>> = None;
        if exporter.is_none() && test_hook.is_none() {
            #[cfg(not(test))]
            {
                state.completed = true;
            }
            return;
        }
        let (status, outcome) = state.http.expect("completed HTTP status");
        state.completed = true;
        drop(state);
        let completed = CompletedTrace {
            inner: Arc::clone(&self.0),
            status,
            outcome,
        };
        if let Some(exporter) = exporter {
            exporter.enqueue(completed.clone());
        }
        #[cfg(test)]
        if let Some(hook) = test_hook {
            hook(completed);
        }
    }
}

impl TraceBuffer {
    pub(crate) fn completed(&self) -> Option<CompletedTrace> {
        let state = self
            .0
            .completion
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !state.completed {
            return None;
        }
        let (status, outcome) = state.http?;
        Some(CompletedTrace {
            inner: Arc::clone(&self.0),
            status,
            outcome,
        })
    }
}

pub(crate) struct CompletedTrace {
    inner: Arc<Inner>,
    pub(super) status: u16,
    pub(super) outcome: &'static str,
}

impl CompletedTrace {
    pub(crate) fn span_count(&self) -> usize {
        self.inner
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .record_count
    }

    pub(crate) fn queued_bytes(&self) -> usize {
        std::mem::size_of::<Inner>() + std::mem::size_of::<Self>()
    }

    pub(crate) fn otlp_spans(&self) -> Vec<Value> {
        use sha2::{Digest, Sha256};
        let records = self
            .inner
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let trace_id = hex(&self.inner.context.trace_id);
        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .saturating_sub(self.inner.started.elapsed());
        let base_ns = start.as_nanos();
        let span_ids = (0..records.record_count)
            .map(|index| {
                let mut hash = Sha256::new();
                hash.update(self.inner.context.trace_id);
                hash.update((index as u64).to_be_bytes());
                let digest = hash.finalize();
                let id: [u8; 8] = if index == 0 {
                    self.inner.response_span
                } else {
                    digest[..8].try_into().unwrap()
                };
                hex(&id)
            })
            .collect::<Vec<_>>();
        records.records[..records.record_count].iter().enumerate().map(|(index, row)| {
            let parent = row.parent.map(|parent| span_ids[parent as usize].clone()).or_else(|| {
                self.inner.context.response_seed.is_none().then(|| hex(&self.inner.context.parent_id))
            });
            let mut attributes = vec![
                json!({"key":"konae.runner.kind","value":{"stringValue":self.inner.runner_kind}}),
                json!({"key":"http.response.status_code","value":{"intValue":self.status.to_string()}}),
            ];
            if let Some(action) = records.name(row.action_name) { attributes.push(json!({"key":"konae.action.name","value":{"stringValue":action}})); }
            if let Some(origin) = row.origin { attributes.push(json!({"key":"konae.action.origin","value":{"stringValue":origin}})); }
            if let Some(code) = records.name(row.error_code) { attributes.push(json!({"key":"konae.error.code","value":{"stringValue":code}})); }
            for (key, value) in [("konae.db.rows_read", row.rows_read), ("konae.db.rows_written", row.rows_written), ("konae.action.reply_bytes", row.reply_bytes)] {
                if let Some(value) = value { attributes.push(json!({"key":key,"value":{"intValue":value.to_string()}})); }
            }
            if let Some(value) = row.sqlite_us { attributes.push(json!({"key":"konae.db.duration_us","value":{"intValue":value.to_string()}})); }
            let begin = base_ns.saturating_add(row.start_us as u128 * 1_000);
            let end = begin.saturating_add(row.duration_us as u128 * 1_000);
            let mut span = json!({"traceId":trace_id,"spanId":span_ids[index],"name":row.name,"kind":1,"flags":if self.inner.context.sampled {1} else {0},"startTimeUnixNano":begin.to_string(),"endTimeUnixNano":end.to_string(),"attributes":attributes,"status":{"code":if self.outcome == "ok" {1} else {2}}});
            if let Some(parent) = parent { span["parentSpanId"] = json!(parent); }
            span
        }).collect()
    }

    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            status: self.status,
            outcome: self.outcome,
        }
    }

    pub(crate) fn local_json(&self, at: u64) -> Value {
        let records = self
            .inner
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let spans = records.records[..records.record_count]
            .iter()
            .map(|row| json!({
                "parent": row.parent, "name": row.name, "startUs": row.start_us, "durationUs": row.duration_us,
                "action": records.name(row.action_name), "origin": row.origin, "sequence": row.sequence,
                "rowsRead": row.rows_read, "rowsWritten": row.rows_written, "replyBytes": row.reply_bytes,
                "sqliteUs": row.sqlite_us, "errorCode": records.name(row.error_code),
            }))
            .collect::<Vec<_>>();
        json!({"at":at,"traceId":hex(&self.inner.context.trace_id),"spanId":hex(&self.inner.response_span),"runnerKind":self.inner.runner_kind,"status":self.status,"outcome":self.outcome,"truncated":records.truncated,"spans":spans})
    }

    pub(crate) fn action_timings(&self, version: &str) -> Vec<crate::usage::FunctionActionTiming> {
        let storage = self
            .inner
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let records = &storage.records[..storage.record_count];
        let mut timings = Vec::new();
        for record in records
            .iter()
            .filter(|record| record.name == "konae.action")
        {
            let Some(action) = storage.name(record.action_name) else {
                continue;
            };
            if action == "<unknown>" {
                continue;
            }
            let sqlite = records
                .iter()
                .find(|child| child.name == "konae.sqlite" && child.sequence == record.sequence);
            timings.push(crate::usage::FunctionActionTiming {
                action: action.to_owned(),
                version: version.to_owned(),
                origin: record.origin.unwrap_or("function").to_owned(),
                duration: std::time::Duration::from_micros(record.duration_us),
                sqlite: std::time::Duration::from_micros(
                    sqlite.and_then(|child| child.sqlite_us).unwrap_or(0),
                ),
                rows_read: record.rows_read,
                rows_written: record.rows_written,
                reply_bytes: record.reply_bytes,
                error_code: storage.name(record.error_code).map(str::to_owned),
            });
        }
        timings
    }

    #[cfg(test)]
    fn is_truncated(&self) -> bool {
        self.inner
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .truncated
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
        trace.set_completion_hook(Box::new(move |completed| {
            let _ = sender.send(completed);
        }));
        trace.finish(403, "refused");
        let completed = receiver.recv().unwrap();
        let value = completed.local_json(1_800_000_000_000);
        assert_eq!(value["spans"].as_array().unwrap().len(), MAX_RECORDS - 2);
        assert!(completed.is_truncated());
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
        trace.set_completion_hook(Box::new(move |completed| {
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
            completed.local_json(0)["spans"]
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["name"] == "konae.transaction")
        );
    }

    #[test]
    fn action_summaries_keep_resolved_names_and_match_sqlite_children() {
        let trace = TraceBuffer::new(TraceContext::new(), Instant::now(), "wasm");
        let long_action = format!("sample.{}", "a".repeat(70));
        for (sequence, action, duration, rows) in [
            (1, long_action.as_str(), 28, Some(2)),
            (2, "<unknown>", 9, None),
        ] {
            let _ = trace.push(TraceRecord {
                parent: Some(0),
                name: "konae.action",
                start_us: 10,
                duration_us: duration,
                action_name: Some(action),
                origin: Some("function"),
                sequence: Some(sequence),
                rows_read: rows,
                rows_written: None,
                reply_bytes: Some(16),
                sqlite_us: None,
                error_code: None,
            });
        }
        let _ = trace.push(TraceRecord {
            parent: Some(1),
            name: "konae.sqlite",
            start_us: 12,
            duration_us: 7,
            action_name: None,
            origin: None,
            sequence: Some(1),
            rows_read: None,
            rows_written: None,
            reply_bytes: None,
            sqlite_us: Some(7),
            error_code: None,
        });
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        trace.set_completion_hook(Box::new(move |completed| {
            let _ = sender.send(completed);
        }));
        trace.finish(200, "ok");
        let completed = receiver.recv().unwrap();
        let timings = completed.action_timings("fixture-version");
        assert_eq!(timings.len(), 1);
        assert_eq!(timings[0].action, long_action);
        assert_eq!(timings[0].duration.as_micros(), 28);
        assert_eq!(timings[0].sqlite.as_micros(), 7);
        assert_eq!(timings[0].rows_read, Some(2));
    }

    #[test]
    fn otlp_spans_keep_parentage_and_only_export_recorded_fields() {
        let mut context = parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
        context.sampled = true;
        let trace = TraceBuffer::new(context, Instant::now(), "wasm");
        let _ = trace.push(TraceRecord {
            parent: Some(0),
            name: "konae.action",
            start_us: 5,
            duration_us: 12,
            action_name: Some("probe.read"),
            origin: Some("function"),
            sequence: Some(1),
            rows_read: Some(1),
            rows_written: None,
            reply_bytes: Some(18),
            sqlite_us: None,
            error_code: None,
        });
        let _ = trace.push(TraceRecord {
            parent: Some(1),
            name: "konae.sqlite",
            start_us: 8,
            duration_us: 2,
            action_name: None,
            origin: None,
            sequence: Some(1),
            rows_read: Some(1),
            rows_written: None,
            reply_bytes: None,
            sqlite_us: Some(2),
            error_code: None,
        });
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        trace.set_completion_hook(Box::new(move |completed| {
            let _ = sender.send(completed);
        }));
        trace.finish(200, "ok");
        let completed = receiver.recv().unwrap();
        let spans = completed.otlp_spans();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0]["parentSpanId"], hex(&context.parent_id));
        assert_eq!(spans[1]["parentSpanId"], spans[0]["spanId"]);
        assert_eq!(spans[2]["parentSpanId"], spans[1]["spanId"]);
        assert!(
            spans
                .iter()
                .all(|span| span["traceId"] == hex(&context.trace_id))
        );
        let json = serde_json::to_string(&spans).unwrap();
        for field in [
            "probe.read",
            "konae.sqlite",
            "konae.db.rows_read",
            "startTimeUnixNano",
        ] {
            assert!(json.contains(field));
        }
        for private in [
            "fixture-input",
            "row-value",
            "authorization-value",
            "exception message",
        ] {
            assert!(!json.contains(private));
        }
    }

    #[test]
    fn trace_context_call_attempts_keep_trace_id_and_make_fresh_span_ids() {
        let parent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let first = child_traceparent(parent).unwrap();
        let retry = child_traceparent(parent).unwrap();
        let unsampled = normalize_traceparent(parent, false).unwrap();
        assert_eq!(traceparent_sampled(parent), Some(true));
        assert_eq!(traceparent_sampled(&unsampled), Some(false));
        assert!(first.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-"));
        assert_ne!(&first[36..52], &retry[36..52]);
        assert!(first.ends_with("-01"));
        assert_eq!(
            unsampled,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00"
        );
    }
}
