pub fn config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config
        .epoch_interruption(true)
        .memory_reservation(64 * 1024 * 1024);
    config
}
