# Caché de procesos y deadline HTTP

Esta captura conserva el binario usado por las pruebas, 24 pruebas correctas del binario y cinco pruebas correctas con servidores HTTP reales. Los logs registran el resultado y `artifacts.json` contiene tamaños, hashes y hora de captura.

El build terminó antes de añadir los tests de procesos bajo `cfg(test)` a `main.rs`. Los archivos de `sources/` se capturaron después de las pruebas. No se tomó un snapshot completo antes de compilar. El agente del worker siguió modificando su código posteriormente; esta captura no demuestra correspondencia completa entre las fuentes actuales y el binario. Hace falta la ronda final congelada que coordina el padre.

Las cinco pruebas de proceso comprueban una mutación vencida en cola, otra cuyo cliente desconecta, ausencia de workers en modo nativo o con funciones apagadas, desalojo por TTL y destrucción del proceso al cambiar versión o vaciar la caché. Las dos mutaciones descartadas dejan cantidad, auditoría y recibos intactos. Las pruebas comprueban que el PID antiguo ya no existe después de completar la destrucción.

Comandos ejecutados desde la raíz del checkout:

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/tmp/konae-deno-core-target cargo build --release --locked --manifest-path spikes/functions-runtime/rust-host/Cargo.toml --bin konae-functions-spike
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/tmp/konae-deno-core-target cargo test --release --locked --manifest-path spikes/functions-runtime/rust-host/Cargo.toml --bin konae-functions-spike
KONAE_SPIKE_TEST_BINARY=/tmp/konae-deno-core-target/release/konae-functions-spike CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/tmp/konae-deno-core-target cargo test --release --locked --manifest-path spikes/functions-runtime/rust-host/Cargo.toml --bin konae-functions-spike process_tests -- --ignored --test-threads=1
```

El segundo comando dejó pendientes los dos tests de proceso escritos en ese momento. El tercero ejecutó después los cinco casos de proceso ya completos. Los 24 tests restantes no cambiaron entre ambas ejecuciones. No se ejecutaron benchmarks y los tests detuvieron sus servidores.

La copia local del binario se llama `konae-functions-spike` y está excluida de Git. El repositorio conserva su huella y los comandos para construirlo. Para ejecutar los tests contra la copia local, usa su ruta absoluta en `KONAE_SPIKE_TEST_BINARY`. Los bundles siguen siendo los de `spikes/functions-runtime/app/dist`, salvo que se indique `APP_BUNDLE_DIR`; tampoco se ha conservado aquí un snapshot completo de esos bundles.
