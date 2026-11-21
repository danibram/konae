# Prueba local de functions con Hono

Esta app de prueba usa `konae build` y `konae dev` del checkout. `app/app/functions/notes.ts` atiende GET y POST con Hono, valida el título con Zod y llama acciones SQL declaradas. Cada consulta limita filas por `actor.id`. Las mutaciones usan la clave de idempotencia que exige el Kernel.

Los únicos datos son dos cuentas `example.test` y notas de prueba. No uses esta configuración para datos reales. La app admite registro abierto para crear esas cuentas.

## Arranque en macOS

Desde la raíz del checkout, compila el Kernel y el runner actual. Cargo reutiliza el target existente.

```sh
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo build --locked -j 2 --manifest-path kernel-rs/Cargo.toml -p kernel --bin kernel
CARGO_TARGET_DIR=/tmp/konae-deno-core-target CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 CARGO_PROFILE_RELEASE_DEBUG=0 \
  cargo build --release --locked -j 2 --manifest-path spikes/functions-runtime/rust-host/Cargo.toml
npm ci --prefix spikes/functions-runtime/evidence/2026-11-20-local-dev/app
npm ci --prefix spikes/functions-runtime/evidence/2026-11-20-local-dev/app/app
bun run scripts/konae.ts build --dir spikes/functions-runtime/evidence/2026-11-20-local-dev/app
```

Arranca el servidor con el runner compilado desde este checkout.

```sh
KONAE_KERNEL_BIN="$PWD/kernel-rs/target/debug/kernel" \
KONAE_FUNCTIONS_RUNNER=/tmp/konae-deno-core-target/release/konae-functions-spike \
KONAE_FUNCTIONS_DEVELOPMENT=1 \
  bun run scripts/konae.ts dev --dir spikes/functions-runtime/evidence/2026-11-20-local-dev/app --port 8796 --events-port 8797 --reset
```

`KONAE_FUNCTIONS_DEVELOPMENT=1` permite ejecutar el runner en macOS. Desactiva la contención Linux y usa observación de RSS. No demuestra aislamiento de producción.

En otra terminal, ejecuta la prueba HTTP. El script crea o inicia sesión con las dos cuentas ficticias, prueba el contrato y guarda `results.json`.

```sh
KONAE_KERNEL_BIN="$PWD/kernel-rs/target/debug/kernel" \
KONAE_FUNCTIONS_RUNNER=/tmp/konae-deno-core-target/release/konae-functions-spike \
KONAE_FUNCTIONS_DEVELOPMENT=1 \
  bun spikes/functions-runtime/evidence/2026-11-20-local-dev/check-local.mjs
```

La página local es [http://functions-local-demo.localhost:8796](http://functions-local-demo.localhost:8796). Inicia sesión con `ada@example.test` o `bea@example.test` y la contraseña `konae-local-demo`. El evento de desarrollo está en `http://localhost:8797`.

Para parar la demo, pulsa Ctrl-C en la terminal de `konae dev`. El ensayo expuesto por BB Connect está en [https://danibram--8796.getbb.app](https://danibram--8796.getbb.app). Quien la abra necesita una sesión del BB dueño.

## Resultado observado

La ronda en `results.json` pasó el rechazo anónimo con 401, la validación inválida con 400, el alta con 201, la repetición con 201, el conflicto con 409, el aislamiento de filas entre cuentas, el rechazo de una acción fuera de la lista y la recarga del bundle de `v1` a `v2`. Un `INSERT ... SELECT` parametrizado con `json_each` pasó el build, insertó dos notas del actor y dejó el recuento intacto cuando el guard rechazó un lote inválido con 403. También confirmó que el dueño no puede apagar `functions` mientras la app publicada lo declara. Al retirar temporalmente la declaración, apagar el módulo dejó el endpoint en 404 y no inició ningún proceso runner. El script restaura la declaración y enciende el módulo antes de terminar.

Usage guardó latencia total, cola, inicialización y ejecución por versión. Las cifras de `results.json` son una sola ronda local con procesos fríos y recargas. No son un benchmark.

El build cambió `localKernel` para pasar al Kernel las opciones `KONAE_FUNCTIONS_RUNNER`, `KONAE_FUNCTIONS_DEVELOPMENT`, `KONAE_FUNCTIONS_CGROUP`, `KONAE_FUNCTIONS_UID` y `KONAE_FUNCTIONS_GID`. Antes, `konae dev` filtraba esas variables y devolvía `function.runner-unavailable`. `scripts/konae.test.ts` cubre el paso de las opciones y confirma que el CLI sigue descartando otros valores `KONAE_`.

La imagen Linux estricta y sus requisitos están en [la evidencia de hardening](../2026-11-20-production-hardening/README.md). Esta prueba de macOS no sustituye las comprobaciones de cgroup v2, Landlock y seccomp que exige producción.

## Requisitos para migrar Kamynando

- Mantén una app privada por clínica y conserva invitaciones de miembros. Las consultas clínicas deben filtrar y autorizar en SQL según el perfil de la app. `actor.id` identifica a la cuenta de esa app.
- Declara cada consulta y mutación en `functions.json`. Da a cada función su lista de Server Actions permitidas. No sustituyas la lista por SQL general.
- El ensayo confirmó que un lote pequeño puede usar una sola acción `INSERT ... SELECT` sobre `json_each(?)`. Valida el array en Hono, codifícalo como JSON para el binding `string`, limita cada lote y usa el guard para rechazarlo entero si algún elemento no vale. El request de función tiene un límite de 16 KiB y el conector admite hasta 64 llamadas nativas por petición.
- Conserva las llamadas directas a Server Actions para cuerpos que excedan 16 KiB y para flujos que no necesitan lógica Hono. El tamaño y el origen de cada petición cambian con la ruta.
- Conserva `miniapp.files` para subir y leer bytes, además de `files_meta` para sus relaciones SQL. El contrato inicial de Functions no expone los bytes de archivos.
- Mantén las claves Google y S3 en sus servicios actuales. La función recibe el actor y las acciones permitidas, no secretos ni credenciales del dueño.
- El build SPA y la reconstrucción de un export requieren una comprobación aparte. Esta prueba no valida el builder ni los cuerpos grandes de Kamynando.

La prueba no leyó datos de Kamynando y no modificó su checkout. No confirma la semántica clínica de sus acciones ni la seguridad de sus filas.

## Verificación de fixtures del CLI

El 20 de noviembre se reprodujeron los dos fallos de `scripts/konae.test.ts`. Sus fixtures creaban un `node_modules` vacío; `buildLocal` refresca el builder que importa `esbuild`, por lo que Node no podía resolver el paquete desde esas carpetas temporales. Se enlazó el `node_modules` del checkout en los dos fixtures. Las aserciones siguen iguales y los builds usan la dependencia real. Los dos casos afectados pasaron y luego pasó la suite completa, 70 aprobadas y 0 fallidas. Ver [el resultado CLI](./cli-suite.json) y [el registro resumido](./cli-suite.log).
