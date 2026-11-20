# Transacción de una invocación del Kernel

[test.txt](test.txt) registra 18 pruebas del coordinador con SQLite real. [regression.txt](regression.txt) registra otras 28 pruebas de conexiones, cambios de esquema, resultados de acciones y límite de base. [manifest.json](manifest.json) identifica los archivos capturados después de ejecutar las pruebas. El archivo de fuentes conserva estos componentes; los archivos compartidos pueden contener trabajo concurrente de integración y no representan una captura completa del checkout probado.

```sh
CARGO_BUILD_JOBS=2 CARGO_PROFILE_TEST_DEBUG=0 \
  cargo test -p kernel --lib function_data::tests \
  --manifest-path kernel-rs/Cargo.toml \
  -- --test-threads=1 --nocapture
```

## Integración

`function_data::admit(InvocationConfig, Budget)` devuelve `Admission::Replay(Output)` o `Admission::Run(Invocation)`. El dispatcher comprueba autenticación y permisos actuales antes de llamar. Conserva el bloqueo de publicación y el permiso SQL dentro de la tarea bloqueante hasta que termine `finish` o `abort`, aunque HTTP ya haya vencido.

La configuración recibe actor, archivo de base, programa de acciones y clave desde el host. `path` contiene `path_and_query`; `headers` contiene las mismas cabeceras que recibe el handler. Cada llamada usa `Invocation::action(name, input)`. JavaScript no decide SQL, actor, app ni clave.

Un POST con acciones abre `BEGIN IMMEDIATE` antes de crear el worker. La admisión consulta primero el recibo y serializa dos peticiones con la misma clave. Todas las lecturas y escrituras comparten esa transacción. Retener el bloqueo de escritura mientras corre JavaScript tiene un coste de concurrencia por app; el presupuesto absoluto incluye esta espera. El dispatcher aporta la admisión SQL acotada.

GET abre una conexión de solo lectura al invocar su primera acción y rechaza mutaciones. Una función sin acciones no abre la base. Solo se admiten query y mutation sin efectos externos, usando el mismo validador, bindings de actor, guard y límites de resultados del Kernel.

`finish(Output)` valida la respuesta antes de persistirla. Las respuestas 2xx y 3xx confirman; las respuestas con estado 400 o mayor revierten. Un fallo nativo invalida toda la invocación aunque JavaScript lo capture. `abort` y Drop revierten; una capacidad cerrada no acepta operaciones nuevas.

## Recibos y cancelación

La base determina la app. La clave primaria del recibo contiene actor, nombre de función y `Idempotency-Key`, sin versión. El fingerprint incluye método, URI con query, cuerpo y cabeceras visibles normalizadas. Excluye credenciales, cookies, host y la propia clave para permitir otro login o traslado. Un payload distinto devuelve `idempotency_conflict`; un replay devuelve la respuesta HTTP guardada sin ejecutar JavaScript.

El recibo y todos los cambios se confirman juntos. El commit hook puede rechazar una transacción vencida antes de confirmar. Un commit que ya pasó ese punto puede terminar después del deadline. Ese caso devuelve `mutation_outcome_unknown` y un reintento con la misma clave recupera el recibo. Unknown no significa rollback ni permite repetir con otra clave.

SQLite usa espera ocupada cero, reintentos cancelables de hasta 1 ms y progress handler cada 100 instrucciones. El rollback desactiva el progress handler para limpiar una transacción vencida. Ninguna prueba demuestra que se pueda interrumpir un fsync o una llamada bloqueada del sistema operativo.

Los recibos no caducan. La base conserva el límite del Kernel de 256 MiB. La respuesta guardada admite hasta 1 MiB de JSON serializado, 128 cabeceras y 16 KiB de nombres y valores. Una invocación admite hasta 64 acciones. La conexión comparte la configuración WAL y synchronous=NORMAL del Kernel.

## Pruebas y límites

Las pruebas cubren rollback tras error, aborto y Drop, respuesta 400, redirect 303, fallo FK capturado, guard, actor, límites de resultados, concurrencia de claves, separación por app, fingerprint de query y cabeceras, commit tardío y replay. Prueban que el recibo sobrevive a un cambio de esquema y no aparece como DDL editable.

El authorizer rechaza triggers y vistas en conexiones de funciones, incluso durante SQL interno de recibos. Las pruebas intentan leer la tabla y copiar `NEW.response_json` mediante triggers. La admisión rechaza esquemas alterados y objetos adjuntos a la tabla reservada. La forma del esquema no demuestra la procedencia de datos importados; el propietario que importa una base sigue siendo una autoridad de confianza.

Esta ronda prueba el coordinador y sus helpers dentro del crate Kernel. No ejecuta el dispatcher HTTP con el worker Deno ni demuestra publicación, traslado completo, contención del proceso o límites de memoria. Esas comprobaciones pertenecen a la integración que coordina el padre. La evidencia histórica del spike permanece intacta.
