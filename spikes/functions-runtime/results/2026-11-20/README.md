# Medición del 20 de noviembre de 2026

Recomiendo continuar con funciones TypeScript, Hono opcional y un conector del Kernel. `deno_core` es un candidato viable por el coste observado. Este experimento no autoriza su uso en producción; la cancelación nativa falla y faltan garantías del Kernel real.

## Comparación en caliente

Cada celda de latencia resume cinco rondas con 5.000 peticiones y una conexión. Se muestra la mediana de los p50 de ronda y, entre corchetes, el menor y mayor p50. El p95 y la CPU son medianas entre rondas. No son intervalos de confianza.

| Servidor | Ruta | p50 HTTP, ms [mín, máx] | p95 HTTP, ms | CPU servidor, ms/petición |
| --- | --- | ---: | ---: | ---: |
| Rust nativo | GET | 0.064 [0.057, 0.067] | 0.108 | 0.020 |
| Rust + deno_core + Hono | GET | 0.087 [0.080, 0.105] | 0.177 | 0.066 |
| Node + Hono | GET | 0.081 [0.078, 0.093] | 0.162 | 0.062 |
| Next production | GET | 0.256 [0.217, 0.277] | 0.451 | 0.280 |
| Rust nativo | POST | 0.100 [0.087, 0.108] | 0.170 | 0.056 |
| Rust + deno_core + Hono | POST | 0.123 [0.113, 0.144] | 0.248 | 0.096 |
| Node + Hono | POST | 0.166 [0.139, 0.177] | 0.386 | 0.144 |
| Next production | POST | 0.369 [0.280, 0.423] | 1.297 | 0.450 |

En este caso Deno añade unos 0,023 ms al p50 nativo tanto en lectura como en escritura. Su p50 es cercano al de Node en lectura y menor en escritura. Next tiene un p50 aproximadamente tres veces mayor que Deno en ambas rutas. Son comparaciones de estas integraciones y estas operaciones pequeñas, no una clasificación universal de motores.

La comparación Rust/Deno comparte servidor HTTP, autenticación y SQLite. Node importa los mismos bundles Hono. Next comparte el esquema Zod y reproduce la lógica en Route Handlers. Node y Next usan el mismo adaptador SQLite. Rust incorpora SQLite 3.50.2 y Node declara 3.53.0; también difieren sus compilaciones y bindings. La ventaja en POST frente a Node no puede atribuirse solo al motor JavaScript.

## Concurrencia y límite de la medición

Cada combinación ejecutó cinco rondas de 4.000 peticiones con ocho conexiones. El cliente comprobó el status y cuerpo de todas las respuestas. El contador de auditoría confirmó una inserción por cada POST medido. Las dos series suman 360.000 peticiones medidas sin errores, más calentamiento y comprobaciones previas.

| Servidor | Ruta | p50 HTTP, ms | Peticiones/s observadas | CPU cliente / tiempo real |
| --- | --- | ---: | ---: | ---: |
| Rust nativo | GET | 0.323 | 19845 | 1.32 |
| Rust + deno_core + Hono | GET | 0.360 | 18750 | 1.38 |
| Node + Hono | GET | 0.377 | 17571 | 1.34 |
| Next production | GET | 1.567 | 4055 | 0.30 |
| Rust nativo | POST | 0.415 | 17098 | 1.58 |
| Rust + deno_core + Hono | POST | 0.497 | 13186 | 1.23 |
| Node + Hono | POST | 0.585 | 9781 | 0.67 |
| Next production | POST | 2.107 | 2903 | 0.25 |

El cliente usa más de un segundo de CPU por segundo real en varias combinaciones. Esta prueba no separa cuánto limita el generador, los sockets y el servidor, ni busca el punto de saturación. La tasa es la conseguida por este cliente; no representa la capacidad máxima del backend.

Los perfiles separados de GET muestran muchas muestras de espera en condición y `kevent`. Entre las muestras activas aparecen sockets y SQLite en Rust, compilación/asignación V8 en Deno y operaciones V8 y GC en Node/Next. No permiten atribuir toda la diferencia a una función concreta ni demostrar el cuello de botella bajo saturación. Se conservan los [perfiles](profiles/results.json), sus resúmenes `*-collapsed.txt` y las trazas completas comprimidas `*.txt.gz`. Las peticiones de profiling no cuentan en las series anteriores.

## Carga y memoria

| Servidor | RSS antes de llamadas a funciones, MiB | RSS después de las series, MiB |
| --- | ---: | ---: |
| Rust nativo | 4.0 | 6.3 |
| Rust + deno_core + Hono | 4.2 | 97.4 |
| Node + Hono | 42.3 | 125.8 |
| Next production | 122.3 | 302.5 |

La RSS suma el proceso y sus hijos; puede contar páginas compartidas varias veces y no mide el pico. El modo nativo usa el mismo binario enlazado con V8, sin crear un runtime. Next incluye el lanzador y el servidor. Los cinco segundos de reposo registraron un incremento de CPU redondeado a cero en todos los servidores; no prueban coste cero. El modo Deno habilitado mantiene un reaper cada 100 ms.

La verificación previa creó cuatro instancias Deno. Durante la serie secuencial se conservaron las cuatro; en la tercera ronda concurrente el TTL desalojó tres y quedó `a/orders`. La RSS posterior sigue en unos 97 MiB con una instancia. No debe dividirse por cuatro para estimar memoria por función, ni suponerse que desalojar devuelve toda la memoria al sistema. Cada ronda conserva sus contadores en `runtime_stats`.

La primera petición tras un desalojo explícito tardó una mediana de 27.36 ms, con rango 26.08–33.94 ms en cinco muestras. Cada muestra comprobó cero instancias antes y exactamente una nueva después. Esto mide recarga de instancia y bundle con el proceso y las cachés de disco ya usados. No mide arranque desde un proceso nuevo, ni usa snapshots.

## Corrección y bloqueos

La [revisión independiente](../../checks/review.md) registra 200 comprobaciones correctas en Rust nativo, Deno y Node; Next pasa 196 y falla cuatro por URLs con `%ZZ`. El ciclo de vida Deno pasa 25 casos, incluyendo módulo apagado, ausencia de llamadas, reutilización, separación por app y función, TTL, capacidad, cambio de clave de versión y recuperación tras un bucle síncrono. Los casos sin comprobar permanecen marcados en los JSON.

El [probe nativo](../../checks/results/runtime-ops-probe.json) reproduce un fallo de la propuesta. Un bloqueo SQLite de 350 ms supera el presupuesto JS de 100 ms. El cliente recibe 500, pero el pedido y la auditoría quedan confirmados. Terminar V8 no cancela la op síncrona ni impide su commit. La implementación necesita un presupuesto nativo y una política explícita para resultados inciertos e idempotencia.

También faltan deadline de inicialización y espera en cola, límites de memoria y respuesta, cierre de capacidades por petición, contención del motor y pruebas de publicación y exportación/importación sobre el Kernel. Los globals pueden conservar valores entre peticiones. Ocultar las ops y denegar permisos no demuestra una sandbox del sistema operativo.

## Entorno y repetición

La ejecución usó macOS 26.6.1 arm64, Apple M3 Pro de 11 CPU, Python 3.14.5, Node 26.0.0 y Next 16.4.6 en producción. El host Rust se compiló en release sin debug; `deno_core` 0.413.0 usa V8 150.4.0. Hono 4.13.19, Zod 4.6.7 y esbuild 0.28.2 están fijados en los locks.

La máquina era compartida. La carga media inicial a un minuto fue 5,44 y hubo actividad ajena. Las cinco rondas rotaron el orden de los servidores; no hubo compilaciones de este experimento durante la medición. Esta evidencia basta para valorar el coste del prototipo, pero no para prometer estas latencias en una instalación ni extrapolar a cientos de funciones.

Los [comandos de preparación](../../README.md) reconstruyen los cuatro servidores. La ejecución guardada corresponde a:

```sh
python3 spikes/functions-runtime/run.py \
  --binary /tmp/konae-deno-core-target/release/konae-functions-spike \
  --output /Users/dbr/.bb/thread-storage/thr_4kbab3tzcv/functions-adr/measurement-1 \
  --allow-next-malformed-url --hold
python3 spikes/functions-runtime/profile.py \
  --targets /Users/dbr/.bb/thread-storage/thr_4kbab3tzcv/functions-adr/measurement-1/targets.json \
  --output /Users/dbr/.bb/thread-storage/thr_4kbab3tzcv/functions-adr/measurement-1/profiles
```

Para repetir, usar un directorio de salida nuevo y la ruta local del binario. `--hold` conserva los procesos exclusivamente para perfilar; después hay que cerrar sus grupos. Esta ejecución los cerró y guarda sus identificadores en [cleanup.json](cleanup.json). Sin profiling, omitir `--hold` para que el ejecutor los cierre automáticamente.

[versions.json](versions.json) registra versiones y comandos. [provenance.json](provenance.json) fija hashes del binario, fuentes, locks y bundles, más hardware y una muestra de actividad ajena. [sequential](sequential/summary.json) y [concurrency-8](concurrency-8/summary.json) contienen resúmenes; sus `rounds.jsonl` conservan cada ronda, CPU, percentiles, errores, auditoría y contadores Deno. Las bases desechables y dependencias compiladas no se añaden al repositorio.

Después de la verificación, el revisor añadió la captura de versiones y hashes a `checks/run_all.py` y `checks/probe_runtime.py`. `post_measurement_changes` conserva sus huellas originales y finales. La [lectura final](../../checks/review.md) confirma que ese cambio no altera fixtures ni criterios. El binario y las fuentes ejecutadas en las mediciones mantienen las huellas originales.
