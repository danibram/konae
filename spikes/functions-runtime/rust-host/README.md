# Host experimental Rust y Deno

Este servidor ejecuta el [workload común](../SPEC.md) en loopback. Los modos `native` y `deno` comparten Axum, autenticación SQLite, bases, consultas y transacciones. `deno` carga los handlers Hono compilados de [app](../app/README.md) con `deno_core` y las implementaciones Web oficiales de Deno. No modifica el Kernel ni publica apps.

## Compilar y arrancar

Comandos desde la raíz del checkout, probados en macOS arm64 con Rust 1.91.0.

```sh
npm --prefix spikes/functions-runtime/app ci
npm --prefix spikes/functions-runtime/app run check
npm --prefix spikes/functions-runtime/app run build
CARGO_TARGET_DIR=/tmp/konae-deno-core-target CARGO_BUILD_JOBS=2 \
  cargo build --release --locked --manifest-path spikes/functions-runtime/rust-host/Cargo.toml

MODE=native PORT=43101 DATA_DIR=/tmp/konae-native-round \
  /tmp/konae-deno-core-target/release/konae-functions-spike
```

Otro proceso ejecuta Deno con sus propias bases.

```sh
MODE=deno PORT=43102 DATA_DIR=/tmp/konae-deno-round \
  /tmp/konae-deno-core-target/release/konae-functions-spike
```

El proceso escribe su PID, puerto y directorio en stderr. Quien lance cada proceso debe conservar su PID y terminarlo antes de otra ronda. `smoke.py` crea y termina sus propios hijos. La implementación no deja servidores ejecutándose para las mediciones.

El perfil release fija `debug=0` y elimina la información de depuración. `Cargo.lock` fija las transitivas. El target se guarda fuera del checkout y ocupó aproximadamente 1,9 GiB después de las compilaciones de desarrollo del experimento. El binario tiene aproximadamente 78 MiB. Las dependencias principales son las siguientes.

| Crate | Versión |
| --- | --- |
| deno_core | 0.413.0 |
| deno_web | 0.291.0 |
| deno_webidl | 0.260.0 |
| deno_fetch | 0.284.0 |
| deno_net | 0.252.0 |
| deno_permissions | 0.119.0 |
| deno_v8, transitiva | 0.5.0 |
| v8, transitiva | 150.4.0 |
| axum | 0.8.9 |
| rusqlite | 0.37.0 |
| tokio | 1.48.0 |

V8 152.2.0 cacheado para el experimento anterior no es compatible con este conjunto. Esta compilación descarga el artefacto precompilado de V8 150.4.0. No usa `rusty_v8` directamente como motor de la aplicación; el acceso a `deno_core::v8` sirve para convertir valores y terminar ejecución mediante el mecanismo de `JsRuntime`.

## Configuración

| Variable | Valor por defecto | Uso |
| --- | --- | --- |
| `MODE` | `native` | Acepta `native` o `deno` |
| `PORT` | 43101 en native, 43102 en deno | Siempre escucha en `127.0.0.1` |
| `DATA_DIR` | `/tmp/konae-functions-<mode>` | Crea `a.sqlite` y `b.sqlite`; conserva datos existentes |
| `APP_BUNDLE_DIR` | `../app/dist`, relativo al manifiesto al compilar | Directorio de bundles ESM |
| `FUNCTIONS_ENABLED` | `1` | `0` desactiva funciones Deno y devuelve 404 en sus rutas |
| `CACHE_CAPACITY` | `4` | Máximo de entradas; debe ser positivo |
| `CACHE_TTL_MS` | `60000` | Tiempo sin actividad antes de desalojar una entrada |
| `JS_TIMEOUT_MS` | `100` | Deadline de ejecución del handler; no incluye cola ni inicialización |

Las bases usan WAL, `synchronous=FULL`, espera de bloqueo de 5 segundos y consultas preparadas en caché. Las mutaciones usan `BEGIN IMMEDIATE`, filtran propietario en el `UPDATE` e insertan auditoría antes del commit. No hay idempotencia.

`FUNCTIONS_ENABLED=0` solo modifica el modo Deno. El modo nativo sigue siendo la referencia sin funciones JavaScript. En native y Deno apagado no se crea el barrido TTL, ningún hilo de función ni ningún `JsRuntime`.

## Caché y actor

Cada entrada usa la clave app, función y versión. Las versiones iniciales son `v1`. El primer acceso autenticado crea un hilo ejecutor, un `JsRuntime` y un hilo watchdog para esa clave. El ejecutor carga el bundle de la función elegida. Las demás funciones y apps no crean runtimes.

Cada instancia ejecuta una petición cada vez. Distintas instancias tienen hilos independientes; las operaciones de una misma base se serializan con un mutex. Una cola admite hasta 64 trabajos pendientes por instancia. El host devuelve 500 `internal_error` si la cola está llena o la capacidad está ocupada por instancias activas. La prueba no propone esa respuesta como política de saturación para producción.

El actor vive en `OpState` solo durante la petición y vuelve a `None` al terminar. Las operaciones Rust usan la base asignada a la instancia y el actor autenticado; no aceptan identidad ni SQL desde JavaScript. Rust también comprueba los rangos de identificador y cantidad. `preferences` no puede invocar las operaciones de pedidos.

Un barrido cada 100 ms desaloja entradas inactivas aunque no llegue tráfico. El barrido solo existe en deno habilitado y tiene ese coste fijo incluso con caché vacía. La capacidad expulsa la entrada inactiva usada hace más tiempo. La evicción cierra la cola y espera al hilo para que el runtime y su watchdog se destruyan. Ninguna entrada ocupada se desaloja.

V8 se inicializa al crear el primer `JsRuntime`. El ejecutable contiene el código de V8 aun en modo apagado, pero no inicializa la plataforma allí. Después de desalojar la última instancia la plataforma global sigue inicializada. Cero runtimes no implica que el RSS vuelva al valor anterior al primer acceso.

## APIs Web y conector

[bootstrap.js](src/bootstrap.js) instala las clases oficiales de Deno para Request, Response, Headers, URL, URLSearchParams, texto, streams, eventos, AbortSignal, Blob y FormData. No implementa polyfills locales. `deno_net` se necesita porque `Request` importa el módulo de cliente HTTP y este importa TLS.

El host instala `Permissions::none_without_prompt`, encierra sus referencias a `core` en una función y retira `Deno` y `__bootstrap` del objeto global antes de cargar la app. No publica `fetch`, APIs de archivos, entorno, procesos ni temporizadores. Esto limita la interfaz de este experimento; no es una auditoría del conjunto de ops registrado por las extensiones.

El conector solo ofrece `readOrder` y `updateOrder`. No es el SDK de producción, no reutiliza el validador del Kernel y no admite SQL arbitrario. El host serializa la petición a JSON para construir la llamada JS y serializa la respuesta de vuelta. Ese coste forma parte de las mediciones del host. Los bundles se leen al crear la instancia, no en cada petición.

## Controles de prueba

Todos los controles están en el mismo listener loopback y no requieren credenciales. No son una API de Konae.

| Petición | Resultado |
| --- | --- |
| `GET /__stats` | Solo lee contadores; no ejecuta evicción |
| `POST /__control/evict` | Desaloja entradas inactivas y devuelve `{"evicted":N}` |
| `POST /__control/version` | Recibe `{"app":"a","function":"orders","version":"v2"}`; invalida esa función y devuelve `{"ok":true}` |
| `GET /__control/loop` | Ejecuta `while (true) {}` en la instancia a/orders; el watchdog debe producir 500 |

El cambio de versión devuelve 409 `busy` si la función tiene trabajos en curso. Acepta solo apps y funciones del SPEC, y versiones de entre 1 y 64 caracteres. Cambia la clave de caché y vuelve a leer el mismo archivo al siguiente acceso. No implementa almacenamiento de artefactos por versión ni un despliegue atómico.

`/__control/loop` devuelve 404 en native y en modo apagado. Solo debe invocarse en un proceso hijo que la prueba pueda terminar. `smoke.py` cumple esa condición y usa un timeout HTTP externo.

Ejemplo de stats después del primer pedido.

```json
{
  "mode": "deno",
  "functionsEnabled": true,
  "engineInitialized": true,
  "initializationAttempts": 1,
  "queueCapacity": 64,
  "runtimesCreated": 1,
  "runtimesLive": 1,
  "cacheHits": 0,
  "evictions": 0,
  "timeouts": 0,
  "capacity": 4,
  "ttlMs": 60000,
  "instances": [{
    "app": "a", "function": "orders", "version": "v1",
    "runtimesCreated": 1, "requests": 1, "busy": 0, "idleMs": 2
  }]
}
```

Los contadores superiores son acumulativos salvo `runtimesLive`. `runtimesCreated` cuenta inicializaciones completas; `initializationAttempts` también cuenta intentos fallidos. `engineInitialized` indica que se intentó crear el motor y permanece verdadero después. `cacheHits` cuenta accesos a una entrada ya existente, incluso si debe recrear un runtime terminado por timeout. `evictions` cuenta entradas eliminadas por TTL, capacidad o controles. `timeouts` cuenta disparos del watchdog. Los contadores por entrada desaparecen al desalojarla. `busy` cuenta trabajos activos y en cola; `requests` cuenta trabajos terminados, incluidos errores y el bucle de prueba. Después de un timeout puede existir una entrada con cero runtimes vivos hasta que la siguiente petición lo recree.

## Comprobaciones y límites

```sh
python3 spikes/functions-runtime/rust-host/smoke.py \
  /tmp/konae-deno-core-target/release/konae-functions-spike
```

La prueba propia pasó 74 comprobaciones de HTTP, SQLite, actor, rollback, capacidad, TTL sin tráfico, versión, modo apagado y watchdog. La suite independiente de [checks](../checks/) pasó 200 comprobaciones para native, 200 para deno y 25 de ciclo de vida. Las mediciones de rendimiento las coordina el padre; este directorio no contiene una carga larga.

El watchdog persiste por instancia. Usa una condición y un mutex compartidos para armar, cancelar y disparar `IsolateHandle::terminate_execution`. Cancelar y disparar se excluyen bajo el mismo bloqueo para evitar que el deadline anterior termine la siguiente petición. La prueba interrumpe un bucle JavaScript síncrono, destruye el runtime terminado y sirve la siguiente petición con otro runtime.

Quedan bloqueos para producción.

- El watchdog no cancela una operación Rust o SQLite. La prueba independiente mantuvo un bloqueo SQLite durante 350 ms con deadline JS de 100 ms. El host respondió 500 y contó un timeout, pero cambió la cantidad de 2 a 7 e insertó auditoría. La transacción pudo hacer commit después del deadline. Un cliente no puede deducir del 500 si la mutación ocurrió. La evidencia está en [runtime-ops-probe.json](../checks/results/runtime-ops-probe.json).
- El deadline no cubre inicialización, compilación del bundle ni espera en cola. Un bundle que se bloquee durante su evaluación puede bloquear su hilo y la destrucción de su entrada. Hace falta contención del proceso para esos casos.
- No hay límite de memoria del isolate, del cuerpo de respuesta ni una prueba contenida de agotamiento de memoria. Un fallo fatal de V8 puede terminar todo el host. Dos isolates no prueban aislamiento de seguridad entre apps.
- No se han demostrado revocación de tareas asíncronas retenidas entre peticiones, defensa ante prototipos modificados por código hostil, disponibilidad bajo saturación ni cierre de todos los accesos indirectos a ops. Ocultar globals y denegar permisos no sustituye esa revisión.
- No hay compatibilidad completa con Deno, Node, npm, imports dinámicos, streaming HTTP ni Server Actions del Kernel. La app se empaqueta de antemano; los errores inesperados se convierten en 500 sin trazas HTTP.
- La versión de caché es una etiqueta de prueba, y el SPEC no incluye idempotencia. El experimento no demuestra exportación de funciones con cuentas, datos y reglas, ni igualdad de diagnósticos entre Validation Gate y Kernel.

La documentación oficial de [deno_core](https://docs.rs/deno_core/0.413.0/deno_core/) describe `JsRuntime`. La composición del runtime de Deno está en [worker.rs](https://github.com/denoland/deno/blob/main/runtime/worker.rs); este host incluye solo las extensiones necesarias para el workload.
