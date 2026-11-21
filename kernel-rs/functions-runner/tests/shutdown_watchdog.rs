use konae_functions_executor::{Budget, MemoryPolicy, Worker, protocol::WorkerLimits};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Bundle(PathBuf);

impl Bundle {
    fn new(source: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "konae-functions-runner-shutdown-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("orders.mjs"), source).unwrap();
        Self(path)
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn worker(bundle: &Bundle) -> Worker {
    Worker::spawn_with_policy(
        Path::new(env!("CARGO_BIN_EXE_konae-functions-runner")),
        "orders",
        &bundle.0,
        &Budget::new(Duration::from_secs(10)),
        WorkerLimits::default(),
        MemoryPolicy::DevelopmentPolling,
    )
    .unwrap()
}

fn invoke(worker: &mut Worker) {
    let output = worker
        .invoke_with(
            konae_functions_executor::protocol::Input {
                url: "http://example.test/".into(),
                method: "GET".into(),
                headers: Vec::new(),
                body: Vec::new(),
                body_len: 0,
                actor: "alice".into(),
            },
            &Budget::new(Duration::from_secs(2)),
            &mut |_, _| Err("forbidden".into()),
        )
        .unwrap();
    assert_eq!(output.body, "ok");
}

#[test]
fn shutdown_watchdog_interrupts_a_synchronous_infinite_loop() {
    let bundle = Bundle::new(
        "export function handle() { return new Response('ok'); } export function shutdown() { while (true) {} }",
    );
    let mut worker = worker(&bundle);
    invoke(&mut worker);

    let error = worker.shutdown(Duration::from_millis(250)).unwrap_err();

    assert!(worker.shutdown_invoked());
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "shutdown_failed"
    );
}

#[test]
fn shutdown_deadline_covers_a_pending_hook_that_ignores_cancellation() {
    let bundle = Bundle::new(
        "export function handle() { return new Response('ok'); } export async function shutdown() { await new Promise(() => {}); }",
    );
    let mut worker = worker(&bundle);
    invoke(&mut worker);

    let error = worker.shutdown(Duration::from_millis(250)).unwrap_err();

    assert!(worker.shutdown_invoked());
    assert_eq!(
        konae_functions_executor::error_code(&error),
        "shutdown_failed"
    );
}
