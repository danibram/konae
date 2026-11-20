# Ejecutor por proceso y verificación

La implementación conserva HTTP, autenticación y SQL en el host. Un hijo ejecuta una función de una app y versión, con una petición activa. El host lo crea después de admitir la primera llamada y lo conserva hasta desalojo, fallo o apagado. El módulo apagado y la ausencia de llamadas no crean hijos ni inicializan V8. El código y los resultados anteriores permanecen como evidencia histórica.

## Reparto del código

| Ruta | Responsabilidad |
| --- | --- |
| `kernel-rs/functions-executor/src/supervisor.rs` | Crear el proceso, vigilar el deadline, validar IPC y respuesta, contar operaciones, terminar y recoger el hijo. No depende de Deno ni SQLite. |
| `kernel-rs/functions-executor/src/protocol.rs` | ABI `konae-functions/1`, tipos compartidos, frames de hasta 16 MiB y límites de entrada. El lector valida la longitud antes de reservar el payload. |
| `kernel-rs/functions-executor/src/containment.rs` | Política explícita de memoria. Preparar y verificar el cgroup antes de enviar `Init`, o seleccionar sondeo de desarrollo. |
| `kernel-rs/functions-executor/src/budget.rs` | Deadline absoluto, cancelación y fase de la mutación compartidos. Lo implementó el autor de presupuesto y datos. |
| `spikes/functions-runtime/rust-host/src/worker.rs` | Adaptar las operaciones de pedidos al SQL del padre y ejecutar el protocolo del hijo. El callback posee app, actor y clave de idempotencia. |
| `spikes/functions-runtime/rust-host/src/runtime.rs` y `bootstrap.js` | Inicializar V8 en el hijo, construir Request/Response, revocar capacidades y ejecutar el apagado opcional. |
| `cache.rs` y `main.rs` del spike | Admisión, cola, caché, módulo apagado y traducción HTTP. Los cambió su autor, con las firmas acordadas. |

Se reutilizan las rutas Hono, los bundles y el patrón de caché por app y función. La versión debe identificar código inmutable; cambiar una etiqueta sobre el mismo bundle no demuestra publicación. El runner actual sigue siendo el binario experimental configurado por el operador. No se ha creado un ejecutable distribuible de producción ni se ha enlazado Deno con el Kernel.

## Contrato de ejecución

- El supervisor recibe un deadline absoluto que incluye cola, arranque, evaluación del módulo, handler y serialización. No reinicia el presupuesto entre fases. Un reader dedicado y stdin no bloqueante permiten comprobarlo aunque V8 no coopere.
- El hijo recibe fuente y datos de petición por pipes. No recibe conexiones SQLite, herramientas de owner, claves de idempotencia ni el entorno heredado. `Input.actor` solo informa al handler; la autoridad permanece en el callback del padre. El dispatcher del Kernel debe retirar su cookie de identidad antes de construir Request.
- Cada llamada lleva generación de proceso, petición, token aleatorio y secuencia de operación. El hijo y el supervisor los validan. Una capacidad retenida conserva su identidad original y falla con `capability_expired` durante otra petición. El callback no consulta la base cuando esa validación falla.
- `context.actions.call(name, input)` devuelve una Promise y envía la operación `action`. El host decide qué acción admite. `data.readOrder` y `data.updateOrder` son adaptadores del fixture. Un error nativo registrado prevalece aunque el handler capture la excepción.
- `Output.body` admite UTF-8 estricto. El puente consume la respuesta por chunks, cuenta bytes antes de acumularlos y rechaza bytes inválidos. El supervisor vuelve a limitar body y headers y rechaza headers de transporte. No promete respuestas binarias en esta ABI.
- El callback nativo debe cumplir el mismo presupuesto. El supervisor no puede interrumpir por sí solo una función Rust arbitraria que el callback ejecute en su hilo. El estado de mutación y los recibos duraderos deciden si corresponde rollback, replay o `mutation_outcome_unknown`. Matar V8 no revierte un commit.
- El runtime rechaza actividad nativa pendiente tras el handler. `AbortSignal.timeout` no se admite en esta versión porque las estadísticas de Deno excluyen sus timers del inventario ordinario. El código usa `Request.signal`; no se exponen timers genéricos. Los globales JavaScript pueden conservar valores y no ofrecen aislamiento de datos entre peticiones.

## Apagado

El bundle puede exportar `shutdown(signal: AbortSignal)`. El host solo lo ejecuta con la función inactiva y las capacidades revocadas. Una gracia limitada permite cooperación; su agotamiento fuerza kill y wait. Un bucle síncrono, OOM o crash no garantiza que el hook termine o llegue a ejecutarse. El reader del hijo termina el proceso al detectar EOF del padre, aunque JavaScript esté bloqueado.

La señal de cancelación y la espera cooperativa no amplían el deadline de petición. La respuesta HTTP puede llegar después de ese instante por planificación del sistema, recogida del proceso y limpieza. El sondeo usa intervalos de hasta 5 ms y la limpieza de cgroup reintenta hasta 100 ms; esto no es una garantía de tiempo real ni un límite absoluto del scheduler o de `wait`.

## Memoria y contención

`WorkerLimits` conserva heap, respuesta, RSS y número de operaciones. La API genérica exige `MemoryPolicy`; no tiene un constructor que seleccione silenciosamente el modo de desarrollo.

`DevelopmentPolling` limita el heap de V8 y detecta exceso de RSS por sondeo. Una asignación puede superar el umbral antes del siguiente sondeo. La prueba usa una asignación finita de 112 MiB para demostrar detección y supervivencia del padre, no ausencia de exceso.

`LinuxCgroup` exige un directorio cgroup v2 delegado por el operador, con memory y pids habilitados. Solo crea su subgrupo nuevo. Configura y relee `memory.max`, `memory.swap.max=0`, `pids.max=64` y `memory.oom.group=1`. Verifica `oom_score_adj=0` del hijo y su pertenencia al grupo antes de enviar `Init`. Si falta el controlador, la delegación, un permiso o la confirmación, rechaza `containment_unavailable`. No activa controladores del directorio padre ni toca grupos ajenos.

`memory.max` limita la memoria cargada al cgroup, no un valor exacto de RSS sin exceso temporal. Las cargas previas a migrar el PID pueden permanecer en el grupo de origen. V8 solo se inicializa después de la migración. Al cerrar normalmente, el supervisor mata el grupo, recoge el hijo y elimina su directorio. Un SIGKILL del supervisor impide ejecutar su destructor; aunque el pipe cierra el hijo, puede quedar un directorio cgroup vacío que el operador deberá reconciliar.

Separar procesos contiene la caída de V8. No crea por sí solo una sandbox de archivos o red frente a un escape del motor. Esta implementación no cambia el UID del hijo ni instala aislamiento de filesystem o red del sistema operativo.

## Evidencia nueva

El checkpoint pasó `cargo check --release` y `cargo build --release --locked` del spike. Las cuatro pruebas del crate en macOS comprueban frames inválidos, roundtrip y rechazo de delegación ausente. El padre comunicó cinco pruebas del crate correctas en Linux, incluyendo rechazo de archivos que imitan controles cgroup. No se ha probado el camino completo de ejecución sobre una delegación real escribible.

La suite [hardening-worker.py](hardening-worker.py) pasa 15 casos en [su informe nuevo](hardening-worker-results-20261120T122208/results.json). Cada proceso y dato de prueba es desechable; el JSON conserva hashes del binario y del script.

| Invariante observado | Prueba |
| --- | --- |
| Cancelación Web y apagado cooperativo | Request.signal, hook normal y señal abortada durante shutdown. |
| Shutdown no vuelve a autorizar datos | Una referencia retenida falla sin emitir operación hacia el padre. |
| Padre muerto y JS bloqueado no dejan un worker vivo | EOF directo y SIGKILL del host con bucle síncrono. |
| Un hook bloqueado no impide desalojar | Desalojo fuerza kill y recoge el PID antes de responder. |
| El contrato de respuesta cuenta bytes y rechaza pérdida | Exceso con texto multibyte y rechazo de UTF-8 inválido. |
| Una captura de excepción no oculta un fallo nativo | Promise de actions.call rechazada y error canónico preservado. |
| El presupuesto de operaciones corta llamadas adicionales | El tercer acceso supera un límite de dos. |
| El fallo de heap y el exceso detectado de RSS conservan el host | Asignaciones finitas, health del padre y desaparición del hijo. |
| No se conserva un timer de sistema por ese API | AbortSignal.timeout rechazado aunque el handler capture la excepción. |
| El actor anónimo no se confunde con una cuenta | Contexto con userId null. |

Repetir desde la raíz, con el binario construido:

```sh
python3 spikes/functions-runtime/checks/hardening-worker.py
CARGO_TARGET_DIR=/tmp/konae-deno-core-target cargo test --release --locked \
  --manifest-path kernel-rs/Cargo.toml -p konae-functions-executor -j 2
```

El padre verifica aparte init y handler infinitos, Promise sin resolución, recuperación, idempotencia y la regresión del commit tardío. La prueba Linux con límite de memoria de un contenedor solo demuestra el límite del contenedor; no sustituye la prueba de `LinuxCgroup` con un subgrupo delegado. Esa comprobación efectiva, la distribución del runner y la integración completa Gate/Kernel/export permanecen decisiones y pruebas separadas.
