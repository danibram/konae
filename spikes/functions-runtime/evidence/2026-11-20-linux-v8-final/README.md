# V8 bajo el supervisor cgroup v2

El ejecutor de Konae creó un cgroup v2 de 256 MiB para un worker V8 real en Linux ARM64. Cuando la carga del subgrupo alcanzó el límite, Linux registró un OOM de grupo. El supervisor devolvió `worker_failed`, recogió el worker y retiró el directorio. Una llamada nueva cargó otro worker y obtuvo HTTP 200 en la respuesta del protocolo. [Eventos y recuperación](strict.jsonl) · [Registro del contenedor](strict.log).

## Qué probó la ronda

- Docker limitó su contenedor a 512 MiB y 96 tareas. La preparación montó cgroup v2 dentro del namespace privado del contenedor, activó `memory` y `pids` en un directorio propio y dejó al supervisor en una hoja hermana del worker. `setpriv` retiró las capacidades antes de ejecutar la prueba. Tanto el supervisor como el worker conservaron UID 0 dentro del contenedor, con los conjuntos de capacidades efectivas y de límite vacíos, con `NoNewPrivs=1`.
- La prueba comprobó el PID del worker dentro del subgrupo después de `Ready`, `oom_score_adj=0`, `memory.max=268435456`, `memory.swap.max=0`, `pids.max=64`, `memory.oom.group=1` y la presencia de `cgroup.kill`. El supervisor confirma la migración antes de enviar `Init`. El worker lee ese mensaje antes de crear V8.
- Con el worker solo, el sondeo RSS devolvió `worker_memory_limit`. El cgroup registró 57 eventos `max` y ningún OOM. El PID y el directorio desaparecieron. La prueba inicial esperaba un OOM y por eso terminó con una aserción fallida. [Primera prueba de presión](single-worker.jsonl) · [Aserción](single-worker.log).
- Para adelantar el límite del grupo al umbral RSS de un solo PID, un ayudante de prueba cargó 96 MiB en el mismo subgrupo. El handler V8 intentó escribir un `Uint8Array` de 320 MiB. La delegación registró un evento `oom` y un `oom_group_kill` en sus contadores jerárquicos. Los contadores locales de la delegación y del cgroup de Docker permanecieron en cero. El ayudante salió con SIGKILL. Al volver del supervisor, el PID de V8 y su subgrupo habían desaparecido. El reemplazo empezó con el contador de llamadas a 1 y se recogió tras el cierre normal. [Resultados](strict.jsonl) · [Semántica de `memory.events`](https://docs.kernel.org/admin-guide/cgroup-v2.html#memory-interface-files).

El heap de V8 y la memoria cargada al grupo cubren magnitudes distintas. El límite de cgroup también recoge memoria ajena al heap, como la usada por este `Uint8Array`. El límite admite excesos transitorios y la carga anterior a migrar el PID puede quedar en el grupo de origen. El sondeo RSS sigue activo bajo la política estricta y puede cambiar tanto el primer mecanismo de corte como el código devuelto. Por ahora, el EOF tras un OOM aparece como `worker_failed`.

## Memoria y latencia

| Camino | Frío, mediana y rango | Caliente, mediana y rango |
| --- | ---: | ---: |
| Supervisor + IPC + V8 en Linux estricto | 45,44 ms · 43,18–62,56 | 0,20 ms · 0,14–0,37 |
| GET de Kernel + HTTP + sesión + regla de filas + SQLite + V8 en macOS | 29,52 ms · 27,42–32,08 | 2,54 ms · 2,06–3,11 |

Cada fila contiene tres arranques y cinco llamadas por arranque al mismo worker. En Linux el handler devuelve JSON pequeño sin operaciones nativas. Sus tres picos de memoria cgroup antes de la presión fueron de 32,42 a 34,32 MiB. La muestra describe ese bundle bajo esos límites. El mínimo de memoria para arrancar o servir apps reales sigue sin medir.

En macOS el test usa el Kernel local compilado en perfil de pruebas y el worker de release experimentado antes. Publica tres versiones del bundle y verifica el contador del isolate y la fila autorizada en cada GET. El reloj del cliente incluye el envío y la lectura completa del cuerpo por loopback. Estos datos muestran el efecto de reutilizar el proceso en cada prueba. El Kernel completo en Linux estricto, la carga concurrente y los percentiles de producción siguen sin medir. [Muestras y medianas](results.json) · [Registro HTTP](kernel-http-macos.log).

## Arranque y aislamiento

El binario Linux solo falló en un contenedor limpio antes de emitir `Ready`. `deno_core` 0.413.0 buscó las fuentes JS de cuatro extensiones bajo la ruta absoluta del registro Cargo usada al compilar. Con los archivos JS montados en esa ruta y en solo lectura, el mismo binario arrancó. Las fuentes montadas proceden de `deno_web`, `deno_webidl`, `deno_net` y `deno_fetch`. El runner sigue siendo experimental. Su empaquetado necesita una prueba de arranque en una máquina sin el registro de compilación. [Arranque fallido](init-diagnostic.log) · [Hashes de extensiones](extensions.sha256).

El cgroup contiene memoria y tareas, y permite matar al grupo. La prueba leyó el límite de tareas. Queda pendiente agotar ese límite. El handler vio `Deno` y `fetch` como `undefined`. Esas restricciones de la API JS son distintas del aislamiento de código nativo tras un fallo del motor. El supervisor arranca el hijo con su misma identidad Unix y los mismos namespaces. El worker podría heredar los permisos de escritura del supervisor sobre la delegación si un fallo del motor le permitiera ejecutar código nativo. El aislamiento de red y la raíz de solo lectura en esta ronda los impuso Docker. Para producción sigue pendiente una barrera del sistema operativo para archivos, red, credenciales y la delegación misma, además de probar el cierre cuando el supervisor recibe SIGKILL. La migración del hijo y la limpieza normal se verificaron. Un SIGKILL del supervisor puede dejar un directorio vacío.

## Reproducción

Desde la raíz del repo en Docker ARM64 con cgroup v2, imagen Rust cacheada y registro Cargo preparado para el lock del spike. El montaje escribible se limita al namespace cgroup privado del contenedor de prueba. El script comprueba los límites propios del contenedor antes de crear la delegación. El build de V8 usa el archivo ARM64 oficial precompilado y Cargo trabaja offline. El host descarga solo ese archivo y dos crates que faltaban para Linux.

```sh
evidence=spikes/functions-runtime/evidence/2026-11-20-linux-v8-final
shasum -a 256 -c "$evidence/sources.sha256"
sh "$evidence/run.sh" > /tmp/konae-strict.jsonl 2> /tmp/konae-strict.log
```

[run.sh](run.sh) conserva los comandos de build y ejecución. Los registros de la ronda conservan la compilación del worker y la del ejecutor de prueba por separado. El worker se compiló una vez. El build usó dos jobs, optimización Rust de nivel 1 y cero debuginfo. Ocupó 1,3 GiB del tmpfs de compilación. [Build del worker](build.log) · [Build de la prueba](probe-build.log) · [Imagen](images.txt) · [Fuentes](sources.sha256) · [Binarios Linux](linux-binaries.sha256).

La muestra integrada de macOS se puede repetir con el worker experimental ya compilado.

```sh
KONAE_TEST_FUNCTION_WORKER=/tmp/konae-deno-core-target/release/konae-functions-spike \
KONAE_TEST_FUNCTION_TIMINGS=1 CARGO_BUILD_JOBS=2 \
cargo test --locked --offline --manifest-path kernel-rs/Cargo.toml -p kernel --lib \
  functions_http::tests::real_functions_http_auth_transaction_publish_deadline_and_usage \
  -- --exact --ignored --nocapture --test-threads=1
```
