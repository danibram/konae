# Revisión independiente

## Condiciones que debe conservar la propuesta

| Decisión | Condición y prueba necesaria |
| --- | --- |
| ADR0002 | El Kernel obtiene el actor de una sesión de la app. Una función no recibe credenciales de owner ni puede llamar herramientas de owner. El SQL autoriza la fila antes de leerla o modificarla. Probar credenciales ausentes, inválidas, caducadas y cruzadas, y Alice contra pedidos de Bob. |
| ADR0004 | El runtime limita cada consulta y respuesta aunque TypeScript construya SQL dinámico. Mantener LIMIT, filas, bytes, sentencias permitidas y presupuesto SQLite. La Gate estática no puede demostrar lo que hará código arbitrario. La propuesta debe declarar ese límite y compartir rechazos comprobables entre Gate y Kernel. |
| ADR0008 | La carpeta exportada debe llevar código, versiones, módulos, cuentas, datos y reglas. Las sesiones y secretos no viajan. Un bundle que depende de un servicio del host para autenticar o autorizar no demuestra portabilidad. Hay que ejecutar una importación en otro Kernel. |
| ADR0011 | La decisión actual prohíbe procesos y código propio de la app en el servidor. Las funciones cambian esa restricción y deben decirlo. No cambian por sí solas el actor nulo, el presupuesto SQL, la exclusión de ejecuciones ni la persistencia de slots del planificador. |
| ADR0015 | El módulo apagado rechaza su uso y no crea instancias. Una app sin llamadas tampoco las crea. El estado del módulo viaja en konae.json. Un contador de instancias no prueba ausencia de inicialización global de V8. |

Un isolate V8 no es una sandbox del sistema operativo. Hay que limitar las capacidades que el host expone, los recursos y el alcance de una caída del proceso. Esta prueba funcional no demuestra resistencia frente a escapes del motor ni aislamiento de memoria entre procesos.

Los snapshots solo pueden ser caché de código confiable y reproducible. No deben importar estado de usuarios, credenciales ni objetos nativos vivos. Un cambio de versión invalida la instancia y su caché; el formato de snapshot no reemplaza el formato portable de la app.

Cancelar una future no detiene necesariamente JavaScript síncrono ni una operación nativa. Un watchdog del isolate debe probarse en un proceso desechable con timeout externo. SQLite necesita su propia interrupción y rollback. El experimento debe distinguir ambas pruebas.

Reutilizar una función conserva sus globales. El actor, las cookies, los headers y los handles de petición deben limitarse a la petición actual. Alternar Alice, Bob y apps detecta algunas fugas, pero no demuestra que código arbitrario no retenga estado. Hace falta una decisión explícita sobre qué estado puede sobrevivir.

## Alcance de la verificación

El verificador HTTP guarda un resultado JSON por criterio y observa ambas bases mediante SQLite en modo readonly. La comparación de cuerpos JSON exige claves y valores exactos, sin depender del orden de serialización; conserva el cuerpo recibido para diagnóstico. Cada petición fallida debe dejar intactas orders, audit y sessions en ambas apps. Cada actualización válida debe cambiar solo su pedido y añadir una fila de auditoría del actor correcto.

Estas pruebas escriben mediante HTTP. Deben ejecutarse con DATA_DIR desechables, sin benchmarks ni otros escritores simultáneos. No prueban idempotencia, exportación/importación real, paridad Gate/Kernel, permisos de owner, seguridad del motor ni límites de SQL dinámico. El SPEC deja la idempotencia pendiente. La prueba aparte de cancelación nativa revela el fallo descrito abajo.

## Ejecución

Los scripts usan Python estándar. Desde la raíz del checkout, con los bundles, el binario Rust y el build Next ya construidos, ejecutar:

```sh
python3 spikes/functions-runtime/checks/run_all.py
python3 spikes/functions-runtime/checks/probe_runtime.py
```

`run_all.py` crea procesos propios en puertos loopback efímeros, escribe las bases y logs dentro de `checks/results`, y termina cada grupo de procesos al salir. Ejecuta los servidores en serie. El binario por defecto es `/tmp/konae-deno-core-target/release/konae-functions-spike`; `--binary` permite otro. `--node` selecciona el ejecutable Node y `--only native|deno|node|next|lifecycle` selecciona una parte. Los directorios temporales de datos se borran al terminar; los JSON y logs quedan.

`probe_runtime.py` inspecciona las ops accesibles desde un bundle propio y, si encuentra `op_fetch`, lo llama contra un socket loopback propio. Después mantiene un bloqueo de escritura SQLite durante 350 ms en otro proceso desechable, con presupuesto JS de 100 ms, y observa si una respuesta fallida deja un commit. No llama a servidores compartidos.

Para comprobar un servidor que otro agente haya arrancado y autorizado para escrituras, ejecutar:

```sh
python3 spikes/functions-runtime/checks/verify.py \
  --url http://127.0.0.1:43203 \
  --data-dir /ruta/absoluta/a/datos-desechables \
  --name node \
  --output spikes/functions-runtime/checks/results/node-external.json
```

Cada script devuelve código 1 si observa algún fallo. No convierte divergencias conocidas en éxitos. `verify.py` no fuerza un fallo interno en bases ajenas; `run_all.py` añade esa prueba creando una tabla audit con `CHECK (quantity != 7)` antes de arrancar cada servidor. El UPDATE de cantidad 7 debe revertirse cuando falla la inserción en audit. La cantidad 6 comprueba la recuperación posterior.

## Resultados observados

La ronda final se ejecutó el 20 de noviembre de 2026, con Python 3.14.5 y Node 26.0.0. `results/artifacts.json` guarda las versiones y los SHA-256 del binario, bundles, fuentes Rust y lockfiles usados. Todos los procesos de verificación terminaron antes de liberar los benchmarks del padre.

| Grupo | Correctas | Fallos | Sin comprobar | Informe |
| --- | ---: | ---: | ---: | --- |
| Rust nativo | 200 | 0 | 7 | [native.json](results/native.json) |
| Deno | 200 | 0 | 7 | [deno.json](results/deno.json) |
| Node | 200 | 0 | 7 | [node.json](results/node.json) |
| Next | 196 | 4 | 7 | [next.json](results/next.json) |
| Ciclo de vida Deno | 25 | 0 | 6 | [deno-lifecycle.json](results/deno-lifecycle.json) |
| Ops y cancelación nativa | 1 | 1 | 1 | [runtime-ops-probe.json](results/runtime-ops-probe.json) |

Los cuatro fallos de Next son `a.id.GET.%ZZ`, `a.id.POST.%ZZ`, `b.id.GET.%ZZ` y `b.id.POST.%ZZ`. El SPEC pide 400 y `{"error":"invalid_input"}`. Next devuelve 500, `text/plain` e `Internal Server Error`; no cambia ninguna de las dos bases. La ruta de integración es `baselines/next/app/[...path]/route.js`, líneas 5 y 6. Next decodifica el parámetro antes del handler y lanza `DecodeError` en `baselines/node_modules/next/dist/shared/lib/router/utils/route-matcher.js`, líneas 19 y 21. Conservar este fallo permite distinguir el framework de la lógica común.

El rollback provocado por el constraint de audit y la recuperación posterior pasan en los cuatro servidores. Los casos positivos observan cada UPDATE y la auditoría en ambas bases. La alternancia usa cantidades distintas entre apps para detectar un enrutamiento de datos incorrecto.

El ciclo de vida confirma cero instancias sin llamadas y con el módulo apagado, reutilización entre actores, funciones y apps independientes, capacidad de dos entradas, TTL de 200 ms, invalidación de versión y recreación tras un timeout síncrono. `cache.rs:263` expone stats de solo lectura; las consultas de stats no causan el desalojo TTL. El control de versión cambia la clave y vuelve a cargar el mismo bundle. No demuestra una publicación de código diferente.

## Fallo que bloquea producción

`native-timeout.no-mutation` incumple el SPEC. Después de calentar la función, el verificador mantiene `BEGIN IMMEDIATE` en otra conexión durante 350 ms. Deno tiene `JS_TIMEOUT_MS=100`. La respuesta es 500 con `{"error":"internal_error"}` y stats cuenta un timeout, pero `orders` cambia la cantidad de Alice de 2 a 7 y `audit` añade `(1, 1, 'alice', 7)`. La app b y las sesiones permanecen intactas.

`rust-host/src/runtime.rs:46` ejecuta `data::update` como op síncrona. El watchdog termina JavaScript, pero `rust-host/src/data.rs:66` espera el bloqueo SQLite y `rust-host/src/data.rs:73` confirma la transacción sin consultar esa terminación. El error HTTP llega después del commit. El cliente no puede inferir del 500 que la operación no ocurrió; repetirla puede duplicar efectos. El padre y el autor recibieron el resultado. No he cambiado su implementación.

## Límites que siguen abiertos

- `rust-host/src/runtime.rs:111` evalúa el módulo antes de crear el watchdog en la línea 114. La prueba de bucle cubre un handler ya cargado. No ejecuté un bucle durante la inicialización; la lectura de código muestra que ese tramo queda fuera del presupuesto.
- `rust-host/src/runtime.rs:78` crea el runtime con opciones por defecto y sin una política de memoria de la app. No hice pruebas de agotamiento de memoria ni medí RSS. La capacidad de caché y la cola de 64 peticiones no equivalen a contención de memoria del proceso.
- `rust-host/src/runtime.rs:146` arma el watchdog al ejecutar el handler. El tiempo previo en la cola no consume ese presupuesto. `rust-host/src/cache.rs:155` limita la cola a 64 peticiones, pero no fija un deadline de admisión o espera. SQLite tiene un busy timeout de cinco segundos en `rust-host/src/data.rs:21`, independiente del presupuesto JS. No probé saturación de la cola ni desconexión del cliente.
- El probe no encontró `Deno.core`, `__bootstrap.core`, un binding léxico `core` ni `readOrder`, y `fetch` no está definido. La fuente instala `PermissionsContainer` sin permisos. No intenté una llamada a `op_fetch` porque esos caminos no la exponen. Esto no demuestra que no exista otro acceso a ops ni una sandbox del sistema operativo.
- Los handles de datos consultan el actor que está en `OpState` durante la petición. Alternar los handlers concretos pasa; falta demostrar el cierre de tareas pendientes y capacidades retenidas por código arbitrario. Un global de JavaScript puede conservar datos obtenidos en una petición anterior.
- No probé desalojo mientras una función está ocupada, límites de bytes de respuesta, cancelación de cliente, aislamiento ante fallos del motor ni snapshots. No existe un test de exportación/importación de cuentas, datos y funciones sobre el Kernel real.
- Las consultas de este experimento están fijadas en Rust o Node. No exponen SQL arbitrario ni prueban la Gate, límites de filas o bytes, autorizador SQLite, deadline SQL, errores compartidos o herramientas de owner. Los mismos handlers en Node no prueban la equivalencia de identidad de una app exportada.

## Lectura del ADR0018 propuesto

El padre incorporó las dos precisiones señaladas durante la revisión. El ADR explica que JavaScript puede retener globales y que admitir código propio de app cambia una restricción de ADR0011. Mantiene ADR0002 y ADR0004 vigentes. Las condiciones del conector, la portabilidad y la contención son requisitos propuestos; esta ronda no las convierte en garantías implementadas. El commit tras 500 debe conservarse como bloqueo explícito, junto al fallo de formato y status de Next para `%ZZ`.

## Lectura final del informe y del ADR

Revisé el [ADR0018](../../../docs/adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md) y el [informe de medición](../results/2026-11-20/README.md) sin arrancar servidores, compilar ni ejecutar carga. El ADR mantiene `Status: proposed`, conserva ADR0002 y ADR0004 vigentes y registra el commit tras 500 como incumplimiento que bloquea producción. No encontré otra corrección sustantiva pendiente en sus conclusiones.

Los 80 registros de las dos series suman 200.000 peticiones secuenciales y 160.000 concurrentes, todas con respuesta verificada y cero errores registrados. Cada combinación tiene cinco rondas. Cada ronda POST registra tantas filas nuevas de auditoría como peticiones medidas; GET no añade ninguna. Recalculé las medianas de p50, p95, tasa, CPU del servidor y fracción de CPU del cliente; coinciden con los resúmenes. Los contadores respaldan las cuatro instancias Deno durante la serie secuencial y el desalojo de tres en la tercera ronda concurrente. Los 14 enlaces locales de ambos documentos resuelven a archivos existentes.

El informe limita las tasas al cliente usado, distingue la RSS del árbol de procesos de un pico o presupuesto por función, señala las versiones SQLite distintas y la actividad ajena, y describe la recarga sobre un proceso ya usado. El ADR separa estas observaciones de las garantías pendientes de arranque, cola, memoria, capacidades por petición, aislamiento y exportación. Esa interpretación respeta el alcance de la prueba.

La única diferencia numérica encontrada fue de redondeo. La mediana de las cinco recargas en `idle-after-and-cold.json` es 27,364625 ms. El informe la redondea a 27,36 ms y la versión leída del ADR mostraba 27,37 ms. Se comunicó al padre para unificarla en 27,36 ms; no cambia la conclusión.

La huella original de `provenance.json` precede a la captura automática de artefactos que añadí a `checks/run_all.py` y `checks/probe_runtime.py` después de las pruebas funcionales. Esos cambios añaden `record_artifacts`, sus imports y sus llamadas, para guardar versiones y SHA-256. No modifican solicitudes HTTP, fixtures, SQL, caché, timeouts ni criterios de aceptación. La comparación actual encuentra diferencias únicamente en esos dos archivos de código; el binario, las fuentes del runtime, los bundles y los scripts de benchmark conservan las huellas originales. Deben conservarse las huellas original y posterior, con esta explicación, sin reemplazar la procedencia de la ejecución.
