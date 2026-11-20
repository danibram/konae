# Clasificación de OOM del worker

## Cambio

El supervisor captura los contadores `oom_kill` y `oom_group_kill` de `memory.events` al preparar un subgrupo cgroup. Si el canal del worker termina o el lector informa EOF, vuelve a leer esos contadores antes de limpiar el grupo. Solo clasifica el cierre como `worker_memory_limit` cuando al menos uno aumentó. Si el contador no se puede leer o no aumentó, devuelve `worker_failed`. El camino de commit incierto sigue gestionado por el Kernel y no se sustituye por esta clasificación.

## Verificación

Comando ejecutado desde la raíz del repositorio:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --offline --manifest-path kernel-rs/Cargo.toml -p konae-functions-executor
```

Resultado: 6 pruebas pasaron. Incluyen parser de contadores OOM, discriminación de aumento frente a crash ordinario y rechazo de contadores ausentes. `git diff --check` pasó para ambos archivos cambiados.

SHA-256 tras la prueba:

```text
ba041764897cd50127a42c5785388a9ea8dea4b6b2c8f4d78165babe6166ae27  kernel-rs/functions-executor/src/containment.rs
67b1788326fc4fa1e30c4a06b342e942758059fdd227941c22f1fc0b1168cf3a  kernel-rs/functions-executor/src/supervisor.rs
```

## Límite de esta evidencia

Esta ronda no ejecutó el binario ARM64 ni verificó el código de respuesta en un cgroup Linux real. La prueba de grupo previa sigue documentando el comportamiento del kernel Linux, pero el runner de esa prueba precede a este cambio. La suite local comprueba la decisión de clasificación, no el ciclo de vida completo bajo OOM.

## Criterios de producción

Aún abiertos: runner independiente de los archivos JS del registro Cargo; aislamiento Linux frente a filesystem, red, credenciales, FD y controles cgroup; lifecycle con límite `pids`, SIGKILL del supervisor y limpieza de subgrupos huérfanos; HTTP+auth+SQLite+V8 completo bajo Linux estricto; actualización de los ADR y documentación restante. Este cambio no declara el runner listo para producción.
