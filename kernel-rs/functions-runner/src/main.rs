mod runtime;
mod worker;

fn main() -> anyhow::Result<()> {
    if !std::env::args().any(|arg| arg == "--function-worker") {
        anyhow::bail!("runner must be started by the Kernel");
    }
    worker::worker_main()
}
