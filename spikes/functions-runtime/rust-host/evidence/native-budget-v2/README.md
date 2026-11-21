# Presupuesto nativo e idempotencia

Esta evidencia corresponde al código identificado en [manifest.json](manifest.json) y conservado en [tested-source.tar.gz](tested-source.tar.gz). La ejecución de [test.log](test.log) pasó 19 pruebas. No sustituye la evidencia histórica del fallo ni las mediciones anteriores.

```sh
CARGO_TARGET_DIR=/tmp/konae-deno-core-target CARGO_BUILD_JOBS=2 \
  cargo test --release --locked --bin native-data-tests \
  --manifest-path spikes/functions-runtime/rust-host/Cargo.toml \
  -- --test-threads=1 --nocapture
```

## Resultados

| Caso | Resultado observado |
| --- | --- |
| SQLite bloqueado 350 ms, presupuesto 100 ms | Regresa en 100 ms con deadline; cantidad 2, auditoría 0 y recibos 0 |
| Mutex ocupado | Vence o acepta cancelación sin esperar a que se libere |
| SQL costoso en trigger | El progress handler interrumpe y revierte pedido, auditoría y recibo |
| Cancelación antes del commit | Revierte toda la transacción |
| COMMIT directo tras cancelación | El commit hook de SQLite lo rechaza, incluso sin el check del adaptador |
| WAL confirmado y retorno del commit retrasado 80 ms, presupuesto 40 ms | Devuelve `mutation_outcome_unknown`; conserva el único efecto y permite replay |
| Respuesta perdida tras commit | Replay devuelve la respuesta guardada y no repite auditoría |
| Dos conexiones con la misma clave concurrente | Una mutación y una repetición |
| Misma clave con otro payload | `idempotency_conflict` sin escribir |
| Mismo texto de clave en otra app o actor | Son operaciones independientes |
| Reapertura de la base | Conserva el recibo y devuelve la respuesta original |
| Pedido no autorizado o inexistente | Conserva ese resultado para un replay estable |

## Contrato para el supervisor

El supervisor crea `Budget` con el deadline absoluto al admitir HTTP y conserva clones del mismo estado. Autenticación, lectura y mutación reciben `&Budget`. La espera por mutex usa `try_lock`; SQLite tiene `busy_timeout=0`, reintentos cancelables de hasta 1 ms, progress handler cada 100 instrucciones y veto de commit. El rollback desactiva el progress handler para poder limpiar una transacción vencida.

`Budget::cancel()` fija la cancelación antes de leer la fase de mutación. La operación marca `Committing` antes del último check y del COMMIT. Si el supervisor ve `Clean` tras cancelar, la operación posterior debe pasar otro check que la rechaza. Si ve `Committing`, `Committed` o `Uncertain`, debe devolver `mutation_outcome_unknown` y conservar la clave para repetir. `unknown` no significa rollback.

La mutación recibe la clave que conserva el host, no una clave elegida por el código invitado. La clave debe contener de 1 a 128 bytes ASCII visibles. El recibo usa actor, operación y clave como identidad; el archivo SQLite determina la app. Compara identificador y cantidad como payload canónico. El recibo, la mutación y la auditoría se confirman en una sola transacción. La repetición usa un presupuesto nuevo y devuelve la respuesta guardada, aunque otra petición haya cambiado después el pedido.

`DataError::code()` distingue `deadline_exceeded`, `cancelled`, `idempotency_key_required`, `invalid_input`, `idempotency_conflict`, `mutation_outcome_unknown` y `storage_error`. El último no contiene trazas ni mensajes de SQLite. `MutationOutcome` devuelve `order` y `replayed`. El presupuesto conserva la fase comprometida aunque JavaScript falle después de la operación.

## Alcance

Las pruebas ejecutan SQLite real en bases temporales. La prueba del retorno tardío usa el [hook de WAL de SQLite](https://www.sqlite.org/c3ref/wal_hook.html), que SQLite invoca después del commit. El retraso pertenece solo a esa prueba. El [commit hook](https://www.sqlite.org/c3ref/commit_hook.html) puede vetar una transacción antes de que continúe el commit; no convierte un commit ya iniciado en cancelable.

El mecanismo impide comenzar el commit cuando ya conoce el vencimiento y detecta interrupciones durante SQL. No garantiza interrumpir una llamada bloqueada al sistema operativo ni un fsync. Si el deadline vence durante un commit que ya pasó su último punto de rechazo, el efecto puede persistir y debe comunicarse como incierto. El supervisor tiene que dejar de esperar en su deadline y conservar la misma clave para resolverlo con replay.

Esta ronda prueba las piezas nativas. Todavía no prueba su integración HTTP con el supervisor nuevo, la contención del ejecutor, sus límites de memoria, las capacidades por petición ni la publicación y exportación del Kernel. Los recibos no caducan automáticamente; una política futura de retención debe definir hasta cuándo garantiza replay.
