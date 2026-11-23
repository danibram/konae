use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{path::Path, process::Command, time::Instant};
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig, Store,
    component::{Component, InstancePre, Linker},
};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let [_, file, url, expected, runs] = args.as_slice() else {
        anyhow::bail!("Usage: starling-bench PRECOMPILED_COMPONENT URL EXPECTED_REPLY RUNS");
    };
    let runs: usize = runs.parse()?;
    ensure!(runs >= 2, "At least two samples are required");
    let mut pool = PoolingAllocationConfig::new();
    pool.total_component_instances(1)
        .total_core_instances(8)
        .total_memories(4)
        .total_tables(8)
        .max_memory_size(128 << 20);
    let mut config = Config::new();
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
    let engine = Engine::new(&config)?;
    let file = Path::new(file);
    let load_start = Instant::now();
    // This harness takes native code compiled locally by Wasmtime 38.0.4.
    let component = unsafe { Component::deserialize_file(&engine, file) }?;
    let deserialize_ms = elapsed(load_start);

    let mut linker = Linker::new(&engine);
    linker.instance("konae:starling/host")?.func_wrap(
        "read-order",
        |_, (id,): (u32,)| -> Result<(String,)> {
            let row = if id == 1 {
                r#"{"id":1,"ownerId":"alice","quantity":2}"#
            } else {
                "null"
            };
            Ok((row.to_owned(),))
        },
    )?;
    let instance = linker.instantiate_pre(&component)?;
    let mut instantiate = Vec::with_capacity(runs);
    let mut call = Vec::with_capacity(runs);
    let mut round_trip = Vec::with_capacity(runs);
    let before = rss_kib()?;
    let warmup = sample(&instance, url, expected)?;
    for _ in 0..runs {
        let (instantiate_ms, call_ms, round_trip_ms) = sample(&instance, url, expected)?;
        instantiate.push(instantiate_ms);
        call.push(call_ms);
        round_trip.push(round_trip_ms);
    }
    let after = rss_kib()?;
    println!(
        "{}",
        json!({
            "samples": runs,
            "compiled_bytes": file.metadata()?.len(),
            "deserialize_ms": deserialize_ms,
            "warmup_ms": {"instantiate": warmup.0, "call": warmup.1, "round_trip": warmup.2},
            "instantiate_ms": summary(instantiate),
            "call_ms": summary(call),
            "round_trip_ms": summary(round_trip),
            "rss_before_kib": before,
            "rss_after_kib": after,
        })
    );
    Ok(())
}

fn sample(instance: &InstancePre<()>, url: &str, expected: &str) -> Result<(f64, f64, f64)> {
    let start = Instant::now();
    let mut store = Store::new(instance.engine(), ());
    let guest = instance.instantiate(&mut store)?;
    let instantiate = elapsed(start);
    let handle = guest.get_typed_func::<(&str,), (String,)>(&mut store, "handle")?;
    let start_call = Instant::now();
    let (reply,) = handle.call(&mut store, (url,))?;
    handle.post_return(&mut store)?;
    let call = elapsed(start_call);
    drop(store);
    let round_trip = elapsed(start);
    ensure!(reply == expected, "Unexpected response {reply}");
    Ok((instantiate, call, round_trip))
}

fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn summary(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let percentile = |quantile: f64| {
        let index = quantile * (samples.len() - 1) as f64;
        let lo = index.floor() as usize;
        let hi = index.ceil() as usize;
        samples[lo] + (samples[hi] - samples[lo]) * index.fract()
    };
    json!({"p50": percentile(0.5), "p95": percentile(0.95), "max": samples.last()})
}

fn rss_kib() -> Result<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()?;
    ensure!(output.status.success(), "ps failed");
    std::str::from_utf8(&output.stdout)?
        .trim()
        .parse()
        .context("Bad RSS")
}
