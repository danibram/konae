#[cfg(feature = "quickjs-runner")]
pub fn config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config
        .epoch_interruption(true)
        .memory_reservation(64 * 1024 * 1024);
    config
}

#[cfg(feature = "starling-runner")]
pub fn starling() -> wasmtime::Config {
    let mut pool = wasmtime::PoolingAllocationConfig::new();
    pool.total_component_instances(1)
        .total_core_instances(8)
        .total_memories(4)
        .total_tables(8)
        .max_memory_size(256 << 20);
    let mut config = wasmtime::Config::new();
    config
        .epoch_interruption(true)
        .wasm_reference_types(true)
        .wasm_function_references(true)
        .allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pool));
    config
}
