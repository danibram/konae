use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use super::trace::CompletedTrace;
use anyhow::{Context, Result, bail};
use reqwest::{
    Client,
    header::{CONTENT_TYPE, HeaderName, HeaderValue},
};
use ring::rand::SecureRandom;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

const QUEUE_TRACES: usize = 256;
const TRACE_BYTES: usize = 24 * 1024;
const QUEUE_BYTES: usize = 6 * 1024 * 1024;
const BATCH_SPANS: usize = 512;
const BATCH_BYTES: usize = 512 * 1024 - 2048;
const RESPONSE_BYTES: usize = 64 * 1024;
const BATCH_WAIT: Duration = Duration::from_secs(1);
const POST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct Losses {
    queue_full: AtomicU64,
    trace_too_large: AtomicU64,
    export_failed: AtomicU64,
    partial_success: AtomicU64,
    shutdown: AtomicU64,
    last_log: AtomicU64,
}

#[derive(Clone)]
pub struct Exporter {
    inner: Arc<Inner>,
}

struct Inner {
    tx: Mutex<Option<mpsc::Sender<QueuedTrace>>>,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    shutdown_deadline: Arc<Mutex<Option<tokio::time::Instant>>>,
    shutdown_requested: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    queued_bytes: Arc<AtomicU64>,
    client: Client,
    endpoint: reqwest::Url,
    headers: Vec<(HeaderName, HeaderValue)>,
    losses: Arc<Losses>,
    sample_rate: f64,
    sample_key: [u8; 32],
}

struct QueuedTrace {
    trace: CompletedTrace,
    bytes: usize,
}

struct WorkerConfig {
    queued_bytes: Arc<AtomicU64>,
    client: Client,
    endpoint: reqwest::Url,
    headers: Vec<(HeaderName, HeaderValue)>,
    losses: Arc<Losses>,
    shutdown_deadline: Arc<Mutex<Option<tokio::time::Instant>>>,
    shutdown_requested: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
}

struct Settings {
    endpoint: reqwest::Url,
    headers: Vec<(HeaderName, HeaderValue)>,
    sample_rate: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderConfig(std::collections::BTreeMap<String, String>);

impl Exporter {
    pub(crate) fn from_env(data_root: &Path) -> Result<Option<Self>> {
        let settings = parse_settings(
            std::env::var("KONAE_OTLP_TRACES_ENDPOINT").ok().as_deref(),
            std::env::var("KONAE_OTLP_TRACES_HEADERS").ok().as_deref(),
            std::env::var("KONAE_OTLP_TRACES_SAMPLE_RATE")
                .ok()
                .as_deref(),
        )?;
        let Some(settings) = settings else {
            return Ok(None);
        };
        let sample_key = sample_key(data_root)?;
        Ok(Some(Self::start(settings, sample_key)?))
    }

    fn start(settings: Settings, sample_key: [u8; 32]) -> Result<Self> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(POST_TIMEOUT)
            .build()?;
        let (tx, rx) = mpsc::channel(QUEUE_TRACES);
        let inner = Arc::new(Inner {
            tx: Mutex::new(Some(tx)),
            worker: Mutex::new(None),
            shutdown_deadline: Arc::new(Mutex::new(None)),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            queued_bytes: Arc::new(AtomicU64::new(0)),
            client,
            endpoint: settings.endpoint,
            headers: settings.headers,
            losses: Arc::new(Losses::default()),
            sample_rate: settings.sample_rate,
            sample_key,
        });
        let worker_config = WorkerConfig {
            queued_bytes: Arc::clone(&inner.queued_bytes),
            client: inner.client.clone(),
            endpoint: inner.endpoint.clone(),
            headers: inner.headers.clone(),
            losses: Arc::clone(&inner.losses),
            shutdown_deadline: Arc::clone(&inner.shutdown_deadline),
            shutdown_requested: Arc::clone(&inner.shutdown_requested),
            shutdown_notify: Arc::clone(&inner.shutdown_notify),
        };
        *inner
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(tokio::spawn(worker(worker_config, rx)));
        Ok(Self { inner })
    }

    pub(crate) fn should_sample(&self, trace_id: &[u8; 16]) -> bool {
        if self.inner.sample_rate >= 1.0 {
            return true;
        }
        if self.inner.sample_rate <= 0.0 {
            return false;
        }
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &self.inner.sample_key);
        let digest = ring::hmac::sign(&key, trace_id);
        let value = u64::from_be_bytes(digest.as_ref()[..8].try_into().unwrap());
        (value as f64) / (u64::MAX as f64) < self.inner.sample_rate
    }

    pub(crate) fn enqueue(&self, trace: CompletedTrace) {
        let span_count = trace.span_count();
        if span_count == 0 {
            return;
        }
        let bytes = trace.queued_bytes();
        let previous = self
            .inner
            .queued_bytes
            .fetch_add(bytes as u64, Ordering::AcqRel);
        if previous.saturating_add(bytes as u64) > QUEUE_BYTES as u64 {
            self.inner
                .queued_bytes
                .fetch_sub(bytes as u64, Ordering::AcqRel);
            self.inner
                .losses
                .queue_full
                .fetch_add(span_count as u64, Ordering::Relaxed);
            return;
        }
        let sender = self
            .inner
            .tx
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(sender) = sender.as_ref() else {
            self.inner
                .losses
                .shutdown
                .fetch_add(span_count as u64, Ordering::Relaxed);
            return;
        };
        if let Err(error) = sender.try_send(QueuedTrace { trace, bytes }) {
            let payload = error.into_inner();
            self.inner
                .queued_bytes
                .fetch_sub(payload.bytes as u64, Ordering::AcqRel);
            self.inner
                .losses
                .queue_full
                .fetch_add(payload.trace.span_count() as u64, Ordering::Relaxed);
        }
    }

    pub(crate) async fn shutdown(&self) {
        *self
            .inner
            .shutdown_deadline
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(tokio::time::Instant::now() + Duration::from_secs(1));
        self.inner.shutdown_requested.store(true, Ordering::Release);
        self.inner.shutdown_notify.notify_one();
        self.inner
            .tx
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        let worker = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(worker) = worker {
            let _ = tokio::time::timeout(Duration::from_millis(1100), worker).await;
        }
    }
}

fn parse_settings(
    endpoint: Option<&str>,
    raw_headers: Option<&str>,
    raw_rate: Option<&str>,
) -> Result<Option<Settings>> {
    let rate = raw_rate
        .unwrap_or("1")
        .parse::<f64>()
        .ok()
        .filter(|rate| rate.is_finite() && (0.0..=1.0).contains(rate))
        .context("Invalid KONAE_OTLP_TRACES_SAMPLE_RATE")?;
    let headers = if let Some(raw) = raw_headers {
        let HeaderConfig(values) =
            serde_json::from_str(raw).context("Invalid KONAE_OTLP_TRACES_HEADERS")?;
        values
            .into_iter()
            .map(|(name, value)| {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .context("Invalid KONAE_OTLP_TRACES_HEADERS")?;
                let value =
                    HeaderValue::from_str(&value).context("Invalid KONAE_OTLP_TRACES_HEADERS")?;
                if name == CONTENT_TYPE || name == HeaderName::from_static("host") {
                    bail!("Invalid KONAE_OTLP_TRACES_HEADERS");
                }
                Ok((name, value))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };
    let endpoint = reqwest::Url::parse(endpoint).context("Invalid KONAE_OTLP_TRACES_ENDPOINT")?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        bail!("Invalid KONAE_OTLP_TRACES_ENDPOINT");
    }
    Ok(Some(Settings {
        endpoint,
        headers,
        sample_rate: rate,
    }))
}

fn sample_key(data_root: &Path) -> Result<[u8; 32]> {
    let path = data_root.join("otlp-trace-sampling.key");
    if let Ok(mut file) = std::fs::File::open(&path) {
        let mut key = [0; 32];
        file.read_exact(&mut key)
            .context("Invalid OTLP sampling key")?;
        let mut extra = [0; 1];
        if file.read(&mut extra)? != 0 {
            bail!("Invalid OTLP sampling key");
        }
        return Ok(key);
    }
    let mut key = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow::anyhow!("Could not initialize trace sampling"))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(&key)?;
            file.sync_all()?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut file = std::fs::File::open(path)?;
            let mut existing = [0; 32];
            file.read_exact(&mut existing)
                .context("Invalid OTLP sampling key")?;
            Ok(existing)
        }
        Err(error) => Err(error.into()),
    }
}

async fn worker(config: WorkerConfig, mut rx: mpsc::Receiver<QueuedTrace>) {
    let mut queue = VecDeque::<(Value, usize, usize)>::new();
    let mut open = true;
    let mut shutdown_deadline = None;
    loop {
        if shutdown_deadline.is_none() && config.shutdown_requested.load(Ordering::Acquire) {
            shutdown_deadline = *config
                .shutdown_deadline
                .lock()
                .unwrap_or_else(|error| error.into_inner());
        }
        if queue.is_empty() {
            match rx.recv().await {
                Some(trace) => add_trace(&mut queue, trace, &config).await,
                None => {
                    while let Ok(trace) = rx.try_recv() {
                        config
                            .losses
                            .shutdown
                            .fetch_add(trace.trace.span_count() as u64, Ordering::Relaxed);
                        config
                            .queued_bytes
                            .fetch_sub(trace.bytes as u64, Ordering::AcqRel);
                    }
                    break;
                }
            }
        }
        let deadline = tokio::time::Instant::now() + BATCH_WAIT;
        while open && queue.len() < BATCH_SPANS {
            tokio::select! {
                message = rx.recv() => match message {
                    Some(trace) => add_trace(&mut queue, trace, &config).await,
                    None => {
                        open = false;
                        shutdown_deadline = Some(
                            (*config.shutdown_deadline.lock().unwrap_or_else(|error| error.into_inner()))
                                .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(1)),
                        );
                    },
                },
                _ = tokio::time::sleep_until(deadline) => break,
            }
        }
        if queue.is_empty() {
            continue;
        }
        if shutdown_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            config
                .losses
                .shutdown
                .fetch_add(queue.len() as u64, Ordering::Relaxed);
            let reserved_bytes = queue
                .iter()
                .map(|(_, _, reserved)| *reserved)
                .sum::<usize>();
            queue.clear();
            config
                .queued_bytes
                .fetch_sub(reserved_bytes as u64, Ordering::AcqRel);
            while let Ok(trace) = rx.try_recv() {
                config
                    .losses
                    .shutdown
                    .fetch_add(trace.trace.span_count() as u64, Ordering::Relaxed);
                config
                    .queued_bytes
                    .fetch_sub(trace.bytes as u64, Ordering::AcqRel);
            }
            break;
        }
        let mut batch = Vec::new();
        let mut bytes = 0usize;
        let mut reserved_bytes = 0usize;
        while let Some((_, span_bytes, _)) = queue.front() {
            let span_bytes = *span_bytes;
            if !batch.is_empty()
                && (batch.len() >= BATCH_SPANS || bytes.saturating_add(span_bytes) > BATCH_BYTES)
            {
                break;
            }
            bytes = bytes.saturating_add(span_bytes);
            let item = queue.pop_front().unwrap();
            reserved_bytes = reserved_bytes.saturating_add(item.2);
            batch.push(item);
        }
        let batch_len = batch.len();
        let spans = batch
            .into_iter()
            .map(|(span, _, _)| span)
            .collect::<Vec<_>>();
        let payload = json!({"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"konae-kernel"}}]},"scopeSpans":[{"scope":{"name":"konae.functions"},"spans":spans}]}]});
        let post_deadline = shutdown_deadline
            .unwrap_or_else(|| tokio::time::Instant::now() + POST_TIMEOUT)
            .min(tokio::time::Instant::now() + POST_TIMEOUT);
        let post = async {
            let mut request = config
                .client
                .post(config.endpoint.clone())
                .header(CONTENT_TYPE, "application/json");
            for (name, value) in &config.headers {
                request = request.header(name, value);
            }
            let reply = request.json(&payload).send().await.map_err(|_| ())?;
            if !reply.status().is_success() {
                return Ok(None);
            }
            let body = reply.bytes().await.map_err(|_| ())?;
            if body.len() > RESPONSE_BYTES {
                return Err(());
            }
            Ok(Some(
                serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
            ))
        };
        tokio::pin!(post);
        let response = tokio::select! {
            biased;
            _ = config.shutdown_notify.notified() => None,
            response = tokio::time::timeout_at(post_deadline, &mut post) => Some(response),
        };
        let response = match response {
            Some(response) => response,
            None => {
                let deadline = (*config
                    .shutdown_deadline
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()))
                .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(1));
                tokio::time::timeout_at(deadline, &mut post).await
            }
        };
        let lost = match response {
            Ok(Ok(Some(value))) => {
                let rejected = value
                    .pointer("/partialSuccess/rejectedSpans")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                if rejected > 0 {
                    config
                        .losses
                        .partial_success
                        .fetch_add(rejected, Ordering::Relaxed);
                }
                rejected
            }
            _ => {
                if config.shutdown_requested.load(Ordering::Acquire) {
                    config
                        .losses
                        .shutdown
                        .fetch_add(batch_len as u64, Ordering::Relaxed);
                } else {
                    config
                        .losses
                        .export_failed
                        .fetch_add(batch_len as u64, Ordering::Relaxed);
                }
                batch_len as u64
            }
        };
        let _ = lost;
        config
            .queued_bytes
            .fetch_sub(reserved_bytes as u64, Ordering::AcqRel);
        log_losses(&config.losses);
    }
    if !queue.is_empty() {
        config
            .losses
            .shutdown
            .fetch_add(queue.len() as u64, Ordering::Relaxed);
    }
    log_losses(&config.losses);
}

async fn add_trace(
    queue: &mut VecDeque<(Value, usize, usize)>,
    trace: QueuedTrace,
    config: &WorkerConfig,
) {
    let reserved_bytes = trace.bytes;
    let source_span_count = trace.trace.span_count();
    let encoded = tokio::task::spawn_blocking(move || {
        let spans = trace.trace.otlp_spans();
        let span_count = spans.len();
        let lengths = spans
            .iter()
            .map(|span| serde_json::to_vec(span).map_or(usize::MAX, |bytes| bytes.len()))
            .collect::<Vec<_>>();
        let total = serde_json::to_vec(&spans).map_or(usize::MAX, |bytes| bytes.len());
        (spans, lengths, total, span_count)
    })
    .await;
    let Ok((spans, lengths, total, span_count)) = encoded else {
        config
            .losses
            .trace_too_large
            .fetch_add(source_span_count as u64, Ordering::Relaxed);
        config
            .queued_bytes
            .fetch_sub(reserved_bytes as u64, Ordering::AcqRel);
        log_losses(&config.losses);
        return;
    };
    if total > TRACE_BYTES {
        config
            .losses
            .trace_too_large
            .fetch_add(span_count as u64, Ordering::Relaxed);
        config
            .queued_bytes
            .fetch_sub(reserved_bytes as u64, Ordering::AcqRel);
        log_losses(&config.losses);
        return;
    }
    let overhead = total.saturating_sub(lengths.iter().sum());
    let last = lengths.len().saturating_sub(1);
    for (index, (span, mut bytes)) in spans.into_iter().zip(lengths).enumerate() {
        if index == last {
            bytes = bytes.saturating_add(overhead);
        }
        queue.push_back((span, bytes, if index == last { reserved_bytes } else { 0 }));
    }
}

fn log_losses(losses: &Losses) {
    let counts = [
        losses.queue_full.load(Ordering::Relaxed),
        losses.trace_too_large.load(Ordering::Relaxed),
        losses.export_failed.load(Ordering::Relaxed),
        losses.partial_success.load(Ordering::Relaxed),
        losses.shutdown.load(Ordering::Relaxed),
    ];
    if counts.iter().all(|count| *count == 0) {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = losses.last_log.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < 60 {
        return;
    }
    if losses
        .last_log
        .compare_exchange(last, now.max(1), Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    eprintln!(
        "OTLP trace losses queue_full={} trace_too_large={} export_failed={} partial_success={} shutdown={}",
        counts[0], counts[1], counts[2], counts[3], counts[4]
    );
}

#[cfg(test)]
mod tests {
    use super::super::trace::{CompletedTrace, TraceBuffer, TraceContext, TraceRecord};
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn completed_trace(
        parent: Option<&str>,
        records: usize,
        action_name: Option<&str>,
    ) -> CompletedTrace {
        let mut headers = HeaderMap::new();
        if let Some(parent) = parent {
            headers.insert("traceparent", HeaderValue::from_str(parent).unwrap());
        }
        let context = TraceContext::from_headers(&headers, true);
        let trace = TraceBuffer::new(context, std::time::Instant::now(), "deno");
        trace.set_export_hook(Arc::new(|_| {}));
        for _ in 1..records {
            trace.push(TraceRecord {
                parent: Some(0),
                name: "konae.action",
                start_us: 1,
                duration_us: 2,
                action_name,
                origin: Some("function"),
                sequence: Some(1),
                rows_read: Some(1),
                rows_written: None,
                reply_bytes: None,
                sqlite_us: Some(1),
                error_code: None,
            });
        }
        trace.finish(200, "ok");
        trace.completed().unwrap()
    }

    #[test]
    fn configuration_accepts_json_headers_and_rejects_invalid_values() {
        let settings = parse_settings(
            Some("https://collector.example/v1/traces"),
            Some(r#"{"x-tenant":"test"}"#),
            Some("0.25"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(settings.endpoint.path(), "/v1/traces");
        assert_eq!(settings.sample_rate, 0.25);
        assert_eq!(settings.headers[0].0, "x-tenant");
        assert!(parse_settings(None, None, None).unwrap().is_none());
        assert!(parse_settings(Some("file:///tmp/traces"), None, None).is_err());
        assert!(
            parse_settings(
                Some("https://user:secret@collector.example/v1/traces"),
                None,
                None
            )
            .is_err()
        );
        assert!(
            parse_settings(
                Some("https://collector.example/v1/traces"),
                Some("[]"),
                None
            )
            .is_err()
        );
        assert!(
            parse_settings(
                Some("https://collector.example/v1/traces"),
                Some(r#"{"x":"a\nb"}"#),
                None
            )
            .is_err()
        );
        assert!(
            parse_settings(
                Some("https://collector.example/v1/traces"),
                None,
                Some("1.1")
            )
            .is_err()
        );
        assert!(parse_settings(None, Some("bad"), None).is_err());
    }

    #[tokio::test]
    async fn collector_receives_otlp_json_and_configured_headers_without_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let collector = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..count]);
                if let Some(at) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let body_at = at + 4;
                    let headers = String::from_utf8_lossy(&request[..at]).to_ascii_lowercase();
                    let body_len = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length: ")
                                .and_then(|n| n.parse::<usize>().ok())
                        })
                        .unwrap();
                    if request.len() >= body_at + body_len {
                        break;
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .await
                .unwrap();
            request
        });
        let endpoint = format!("http://{address}/v1/traces");
        let settings = parse_settings(
            Some(&endpoint),
            Some(r#"{"x-tenant":"tenant-fixture"}"#),
            Some("1"),
        )
        .unwrap()
        .unwrap();
        let exporter = Exporter::start(settings, [7; 32]).unwrap();
        let queued_at = tokio::time::Instant::now();
        exporter.enqueue(completed_trace(
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-b7ad6b7169203331-01"),
            1,
            None,
        ));
        let request = tokio::time::timeout(Duration::from_secs(4), collector)
            .await
            .unwrap()
            .unwrap();
        assert!(queued_at.elapsed() >= BATCH_WAIT);
        let text = String::from_utf8(request).unwrap();
        let (headers, body) = text.split_once("\r\n\r\n").unwrap();
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("x-tenant: tenant-fixture")
        );
        let payload: Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            payload
                .pointer("/resourceSpans/0/scopeSpans/0/spans/0/traceId")
                .and_then(Value::as_str),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert!(!body.contains("tenant-fixture"));
        exporter.shutdown().await;
    }

    #[tokio::test]
    async fn flushes_at_the_batch_span_limit() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let collector = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 8192];
            loop {
                let count = stream.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..count]);
                if let Some(at) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let body_at = at + 4;
                    let headers = String::from_utf8_lossy(&request[..at]).to_ascii_lowercase();
                    let body_len = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length: ")?.parse::<usize>().ok()
                        })
                        .unwrap();
                    if request.len() >= body_at + body_len {
                        let body =
                            std::str::from_utf8(&request[body_at..body_at + body_len]).unwrap();
                        let payload: Value = serde_json::from_str(body).unwrap();
                        let count = payload
                            .pointer("/resourceSpans/0/scopeSpans/0/spans")
                            .and_then(Value::as_array)
                            .unwrap()
                            .len();
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                            .await
                            .unwrap();
                        return count;
                    }
                }
            }
        });
        let endpoint = format!("http://{address}/v1/traces");
        let settings = parse_settings(Some(&endpoint), None, Some("1"))
            .unwrap()
            .unwrap();
        let exporter = Exporter::start(settings, [13; 32]).unwrap();
        let probe = completed_trace(None, 16, None);
        let per_trace = probe.otlp_spans().len();
        assert_eq!(BATCH_SPANS % per_trace, 0);
        let traces = BATCH_SPANS / per_trace;
        exporter.enqueue(probe);
        for _ in 1..traces {
            exporter.enqueue(completed_trace(None, 16, None));
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(4), collector)
                .await
                .unwrap()
                .unwrap(),
            BATCH_SPANS
        );
        exporter.shutdown().await;
    }

    #[tokio::test]
    async fn partial_success_counts_rejected_spans() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let collector = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            let _ = stream.read(&mut request).await;
            stream.write_all(b"HTTP/1.1 206 Partial Content\r\ncontent-length: 38\r\ncontent-type: application/json\r\n\r\n{\"partialSuccess\":{\"rejectedSpans\":1}}").await.unwrap();
        });
        let endpoint = format!("http://{address}/v1/traces");
        let exporter = Exporter::start(
            parse_settings(Some(&endpoint), None, None)
                .unwrap()
                .unwrap(),
            [8; 32],
        )
        .unwrap();
        exporter.enqueue(completed_trace(None, 1, None));
        tokio::time::timeout(Duration::from_secs(3), async {
            while exporter
                .inner
                .losses
                .partial_success
                .load(Ordering::Relaxed)
                == 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            exporter
                .inner
                .losses
                .partial_success
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            exporter.inner.losses.export_failed.load(Ordering::Relaxed),
            0
        );
        collector.await.unwrap();
        exporter.shutdown().await;
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_non_success_counts_failed_spans() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let collector = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 8192];
            let _ = stream.read(&mut buffer).await;
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nlocation: /v1/traces\r\ncontent-length: 0\r\n\r\n",
                )
                .await
                .unwrap();
            match tokio::time::timeout(Duration::from_millis(250), listener.accept()).await {
                Ok(Ok(_)) => 1,
                _ => 0,
            }
        });
        let endpoint = format!("http://{address}/v1/traces");
        let exporter = Exporter::start(
            parse_settings(Some(&endpoint), None, None)
                .unwrap()
                .unwrap(),
            [9; 32],
        )
        .unwrap();
        exporter.enqueue(completed_trace(None, 1, None));
        tokio::time::timeout(Duration::from_secs(3), async {
            while exporter.inner.losses.export_failed.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            exporter.inner.losses.export_failed.load(Ordering::Relaxed),
            1
        );
        assert_eq!(collector.await.unwrap(), 0);
        exporter.shutdown().await;
    }

    #[tokio::test]
    async fn collector_timeout_counts_the_batch_as_lost() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let collector = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(POST_TIMEOUT + Duration::from_millis(250)).await;
        });
        let endpoint = format!("http://{address}/v1/traces");
        let exporter = Exporter::start(
            parse_settings(Some(&endpoint), None, None)
                .unwrap()
                .unwrap(),
            [10; 32],
        )
        .unwrap();
        exporter.enqueue(completed_trace(None, 1, None));
        tokio::time::timeout(Duration::from_secs(5), async {
            while exporter.inner.losses.export_failed.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            exporter.inner.losses.export_failed.load(Ordering::Relaxed),
            1
        );
        exporter.shutdown().await;
        collector.abort();
    }

    #[tokio::test]
    async fn shutdown_spends_one_grace_period_and_counts_queued_spans() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let collector = tokio::spawn(async move {
            let _ = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
        });
        let endpoint = format!("http://{address}/v1/traces");
        let exporter = Exporter::start(
            parse_settings(Some(&endpoint), None, None)
                .unwrap()
                .unwrap(),
            [11; 32],
        )
        .unwrap();
        let traces = (0..20)
            .map(|_| completed_trace(None, 16, None))
            .collect::<Vec<_>>();
        let expected = traces
            .iter()
            .map(|trace| trace.otlp_spans().len())
            .sum::<usize>();
        for trace in traces {
            exporter.enqueue(trace);
        }
        let started = tokio::time::Instant::now();
        exporter.shutdown().await;
        assert!(started.elapsed() <= Duration::from_millis(1200));
        assert_eq!(
            exporter.inner.losses.shutdown.load(Ordering::Relaxed),
            expected as u64
        );
        collector.abort();
    }

    #[test]
    fn a_full_channel_drops_and_counts_the_trace() {
        let (tx, _rx) = mpsc::channel(QUEUE_TRACES);
        let inner = Arc::new(Inner {
            tx: Mutex::new(Some(tx)),
            worker: Mutex::new(None),
            shutdown_deadline: Arc::new(Mutex::new(None)),
            shutdown_requested: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            queued_bytes: Arc::new(AtomicU64::new(0)),
            client: Client::new(),
            endpoint: reqwest::Url::parse("http://127.0.0.1/v1/traces").unwrap(),
            headers: Vec::new(),
            losses: Arc::new(Losses::default()),
            sample_rate: 1.0,
            sample_key: [0; 32],
        });
        let exporter = Exporter { inner };
        for _ in 0..=QUEUE_TRACES {
            exporter.enqueue(completed_trace(None, 1, None));
        }
        assert!(exporter.inner.losses.queue_full.load(Ordering::Relaxed) > 0);
        assert!(exporter.inner.queued_bytes.load(Ordering::Relaxed) <= QUEUE_BYTES as u64);
    }

    #[tokio::test]
    async fn an_oversized_trace_is_dropped_and_counted() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let endpoint = format!("http://{address}/v1/traces");
        let exporter = Exporter::start(
            parse_settings(Some(&endpoint), None, None)
                .unwrap()
                .unwrap(),
            [12; 32],
        )
        .unwrap();
        let action = "x".repeat(100);
        let trace = completed_trace(None, 160, Some(&action));
        let expected = trace.otlp_spans().len();
        exporter.enqueue(trace);
        tokio::time::timeout(Duration::from_secs(3), async {
            while exporter
                .inner
                .losses
                .trace_too_large
                .load(Ordering::Relaxed)
                == 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            exporter
                .inner
                .losses
                .trace_too_large
                .load(Ordering::Relaxed),
            expected as u64
        );
        assert_eq!(exporter.inner.queued_bytes.load(Ordering::Relaxed), 0);
        exporter.shutdown().await;
    }
}
