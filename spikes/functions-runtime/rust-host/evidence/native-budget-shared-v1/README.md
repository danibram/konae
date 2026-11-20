# Presupuesto compartido

[test.txt](test.txt) registra 19 pruebas aprobadas después de extraer `Budget` a `konae-functions-executor`. El spike reexporta ese tipo. [manifest.json](manifest.json) registra los hashes y el alcance de la captura de componentes hecha después de la ejecución.

```sh
CARGO_TARGET_DIR=/tmp/konae-deno-core-target CARGO_BUILD_JOBS=2 \
  cargo test --release --locked --bin native-data-tests \
  --manifest-path spikes/functions-runtime/rust-host/Cargo.toml \
  -- --test-threads=1 --nocapture
```

SQLite bloqueado durante 350 ms devuelve deadline a los 100 ms, sin modificar cantidad, auditoría ni recibos. Un commit que termina después del deadline devuelve `mutation_outcome_unknown`; el replay conserva una única escritura.

Estas pruebas cubren la mutación individual del fixture del spike. Las 18 pruebas de [kernel-function-data-v2](../kernel-function-data-v2/README.md) cubren una transacción por invocación con acciones del Kernel y respuesta HTTP persistida. Esta ejecución no sustituye las 19 pruebas históricas conservadas en [native-budget-v2](../native-budget-v2/README.md).

La ejecución usó el target existente, jobs=2 y release sin debug. No arrancó servidores ni hizo benchmarks.
