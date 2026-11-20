# Experimento de funciones TypeScript para Konae

Este experimento compara una función Hono sobre `deno_core` con Rust, Node y Next.js. Conserva autenticación por cookie, validación, autorización por propietario y una mutación SQLite con auditoría en la misma transacción. Sus servidores usan datos desechables. La integración posterior en el Kernel tiene pruebas separadas.

La propuesta está en [ADR-0018](../../docs/adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md). El [SPEC](SPEC.md) define los datos, las respuestas y los límites del experimento.

El [informe medido](results/2026-11-20/README.md) conserva latencias, CPU, memoria, perfiles y procedencia. La [revisión independiente](checks/review.md) conserva las comprobaciones funcionales y sus límites. Esa versión confirmó un commit SQLite después de devolver un error por timeout.

## Integración experimental en el Kernel

El Kernel ejecuta el bundle en un hijo externo. Autentica la petición y aplica las Server Actions en el proceso padre. Cada función arranca con su primera llamada admitida y recibe un presupuesto que abarca las esperas, el arranque, JavaScript y SQLite. Un POST con acciones guarda el recibo en la misma transacción que sus cambios. [ADR-0018](../../docs/adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md) describe los límites y el contrato de reintento.

El Kernel entrega `Request.body` a Hono como un stream y acepta hasta 100000000 bytes por petición. Ese límite pertenece a la integración del Kernel. El límite de 16384 bytes en el SPEC sigue describiendo solo los servidores del experimento, no el Kernel. Consulta [ADR-0019](../../docs/adr/0019-functions-stream-request-bodies-with-bounded-ipc.md) para los límites de memoria, acciones e idempotencia.

En macOS, configura `KONAE_FUNCTIONS_RUNNER` con la ruta absoluta del ejecutable del spike y `KONAE_FUNCTIONS_DEVELOPMENT=1` para desarrollo. El presupuesto por defecto es de 1000 ms y el TTL de 60000 ms. `KONAE_FUNCTIONS_TIMEOUT_MS` y `KONAE_FUNCTIONS_IDLE_MS` cambian esos valores. En modo estricto, el operador configura `KONAE_FUNCTIONS_CGROUP` con la ruta absoluta de una delegación cgroup v2 de Linux. El supervisor rechaza el arranque del worker si falla la contención previa al envío del bundle.

Desde la raíz del repositorio, con el runner ya compilado, ejecuta la prueba del Kernel y la puerta de importación de Konae.

```sh
KONAE_TEST_FUNCTION_WORKER=/ruta/absoluta/konae-functions-spike \
  CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo test --locked --manifest-path kernel-rs/Cargo.toml -p kernel -p konae \
  functions -- --include-ignored --test-threads=1
```

El [registro integrado](evidence/2026-11-20-kernel-functions/logs.txt) conserva los comandos, resultados y huellas. Las pruebas usan HTTP y el worker real. Verifican cambios de versión, plazos, Usage y ejecución tras exportar e importar una app con sus cuentas, filas y reglas. La [ronda final del runner](evidence/2026-11-20-worker-final/logs.txt) conserva las pruebas de protocolo, límites, capacidades y apagado.

La [ronda Linux](evidence/2026-11-20-linux-executor/README.md) verificó compilación y rechazos sin delegación. Queda pendiente ejecutar V8 con una delegación escribible y demostrar el límite y la limpieza del cgroup del supervisor. El OOM de Docker correspondió al cgroup del contenedor.

## Comparación histórica

Las 360.000 peticiones del informe corresponden al runtime original dentro del proceso Rust. Para repetir la comparación, extrae el [código archivado](results/2026-11-20/source-before-hardening.tar.gz) en un directorio vacío y sigue las instrucciones de su README. Los comandos de las secciones siguientes pertenecen a ese código. Las cifras preceden al dispatcher, los recibos y los procesos separados.

## Preparación

Se necesitan Rust, Python 3 y una versión de Node compatible con Next y `node:sqlite`. Los manifests y lockfiles fijan las dependencias del experimento. No se usan credenciales externas.

Desde la raíz del repositorio:

```sh
npm --prefix spikes/functions-runtime/app ci
npm --prefix spikes/functions-runtime/app run check
npm --prefix spikes/functions-runtime/app run build
npm --prefix spikes/functions-runtime/baselines ci
npm --prefix spikes/functions-runtime/baselines run build
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR="$PWD/spikes/functions-runtime/.runs/target" cargo build --release --locked --manifest-path spikes/functions-runtime/rust-host/Cargo.toml
```

La primera compilación descarga las bibliotecas precompiladas de V8 si no están en caché. Se necesitan varios GiB libres para dependencias y artefactos; no es una medida del tamaño distribuible del Kernel.

## Verificación y mediciones

```sh
python3 spikes/functions-runtime/run.py \
  --binary spikes/functions-runtime/.runs/target/release/konae-functions-spike \
  --output spikes/functions-runtime/.runs/measurement-1 \
  --allow-next-malformed-url
```

El directorio de salida debe ser nuevo. El script arranca cuatro procesos en loopback, prepara datos desechables, ejecuta las comprobaciones HTTP, mide cinco rondas alternadas y cierra los procesos propios al terminar. `--base-port` cambia el primero de los cuatro puertos. Los servidores reciben copias independientes de los datos.

Next devuelve un 500 anterior al handler para los cuatro casos GET/POST de `/a/orders/%ZZ` y `/b/orders/%ZZ`. El SPEC exige un 400. La opción `--allow-next-malformed-url` conserva esos fallos en los resultados y permite medir las peticiones válidas únicamente si no hay otros fallos y las bases permanecen intactas. Sin esa opción, la ejecución se detiene al detectar la divergencia. No se añade un proxy para ocultar el comportamiento de Next.

`checks/` contiene la verificación independiente y el ejecutor para rollback, ciclo de vida y bucles síncronos. Sus instrucciones concretas y resultados se registran junto al informe de revisión. Esas pruebas usan procesos y bases desechables; no deben ejecutarse contra una instalación real.

```sh
python3 spikes/functions-runtime/checks/run_all.py \
  --binary spikes/functions-runtime/.runs/target/release/konae-functions-spike
python3 spikes/functions-runtime/checks/probe_runtime.py \
  --binary spikes/functions-runtime/.runs/target/release/konae-functions-spike
```

Ambos scripts devuelven código 1 mientras sigan los fallos documentados. No se convierten en éxitos por ser conocidos. `profile.py` toma perfiles macOS de servidores propios mantenidos con `run.py --hold`; el informe medido incluye los comandos y exige cerrar esos grupos al terminar.

`benchmark.py` también acepta un archivo de servidores ya iniciados mediante `--targets`. Cada entrada contiene `name`, `url`, `pid` y `data_dir`. El proceso indicado debe ser el servidor o su padre exclusivo, porque la medición suma CPU y RSS de sus descendientes.

## Qué mide

- Latencia HTTP desde el envío hasta leer todo el body, con conexiones persistentes. Cada respuesta debe tener status y cuerpo correctos. La comprobación JSON ocurre después del temporizador de latencia y sí cuenta en el tiempo total del cliente.
- CPU del proceso y sus hijos, medida mediante `ps`. Su resolución puede redondear a cero intervalos pequeños. Se conserva también la CPU del cliente.
- RSS muestreada del proceso y sus hijos. No representa el máximo entre muestras ni memoria física exclusiva; las páginas compartidas pueden contarse más de una vez.
- GET y POST con 200 peticiones de calentamiento antes de cada ronda. El POST debe producir exactamente una fila de auditoría por petición; el GET no debe modificar las tablas.
- Una conexión y ocho conexiones concurrentes, en series separadas. Un cliente Python que se sature limita la tasa observada, por lo que no se presenta como capacidad máxima del servidor.
- Reposo antes y después del tráfico, y primeras llamadas Deno después de desalojar explícitamente la función. El arranque de instancia conserva las cachés de disco y la inicialización global del proceso que ya haya ocurrido.

Las rondas rotan el orden de los servidores. Las compilaciones y otras cargas ajenas deben terminar antes de medir. El informe conserva la carga observada y los comandos para repetir la prueba; una máquina compartida no es un laboratorio aislado.

## Alcance de las mediciones históricas

Los servidores Rust del experimento comparten el camino HTTP, la autenticación y SQLite entre sus modos nativo y Deno. No son el Kernel de producción. Las sesiones son tokens públicos de prueba y no ejercitan alta de cuentas, hash de credenciales, revocación o criptografía de sesiones reales.

El conector del experimento tiene operaciones de pedidos concretas. No implementa SQL arbitrario, la Validation Gate, el contrato completo de módulos, exportación/importación, correo ni red de salida. Su autorización de propietario es la política de esta app de prueba, no una política universal impuesta a todas las apps.

Reutilizar el mismo bundle Hono desde Node prueba esa separación entre handler y adaptador. No prueba que las cuentas, los datos y todos los permisos migren sin trabajo.

Los límites de memoria total, contención del motor, cancelación de operaciones nativas y evaluación inicial del módulo requieren pruebas propias antes de producción. Los resultados no justifican retirar restricciones del Kernel para ganar velocidad.
