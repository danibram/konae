# Integración mínima verificable en el Kernel

Lectura del código real para el quinto bloqueo de ADR-0018. Este documento propone cambios; no los implementa ni afirma que las pruebas existentes hayan pasado en esta revisión. No se han ejecutado builds ni carga.

## Infraestructura existente

| Necesidad | Código que se debe reutilizar | Ampliación necesaria |
| --- | --- | --- |
| Contrato de módulos | `src/sdk/contract.ts`, `src/sdk/render.ts`; genera `kernel-rs/kernel/src/modules.json` | Añadir `functions` en el contrato fuente, sin llamadas de navegador. No editar solo el JSON generado. |
| Estado, permisos y cambios | `modules::{defaults,validate_state,check_requirements,require_on,set,read,preserve_release_state}`; owner MCP en `konae/src/mcp.rs` | Detección de uso publicado y dependencias condicionales de auth/actions. Mantener `expected`, `module.off`, `module.in-use` y lectura posterior. |
| Validación | `SqlJsCapsuleValidator.validate` en `src/agent/gate.ts`; `runtime::validate_release`; `action_shape::read`; `refusal.rs` y `refusal-codes.json` | Un parser de `functions.json` en TS y otro en Rust, con los mismos casos, códigos, palabras y orden de rechazo. |
| Publicación | `DeployInput`, `deploy_input`, `stage_local_artifact`, `sources::stage_release`, `persist_release`, `switch_dist`, `Recorded::put_back` en `kernel/src/lib.rs` | Añadir artefacto de servidor al staging, rollback y registro de la misma publicación. |
| Apertura local | `folder::{open,open_all,decide_app_modules}`, `Settings`, `ReleasedSettings` | `folder::open` reconstruye hoy un input con actions/schema y detecta rebuild por digest de `dist/`. Debe leer funciones y reconocer un cambio exclusivo del servidor. |
| Identidad y rutas | `stack`, `deployment_for`, `authenticate_deployment`, `app_cookie_name`, `http::{get_route,post_route}` | Despachar funciones dentro de las capas de host, path, seguridad y mantenimiento existentes. No usar cookies ni sesiones del spike. |
| Datos | `invoke_action`, `resolve_bindings`, `action_lock`, `run_action_in_slot`, `app_db::open`, `restrict_database`, `arm_deadline` | Extraer una entrada interna que reciba Deployment y actor ya verificados, conservando autorización, permisos, guard, Usage y efectos existentes. |
| Exportar e importar | `package::{export,capture,snapshot,revoke_access,install}`, `write_folder_tar`; `src/lib/{export-project,import-folder}.ts`; `src/deployments/import.ts` | El tar incluye hoy solo `konae.json`, `app/` y `dist/`. Incorporar `server/` y transportarlo en CLI, editor e import de fuentes. No basta copiarlo al disco. |

`capture` ya limpia el SQLite de identidad exportado mediante `revoke_access`; `install` también revoca accesos al importar. Se conservan cuentas, hashes de contraseña y reglas. Se borran sesiones, retos y enlaces; las claves importadas quedan revocadas y los tokens de invitación se eliminan. `secrets.sqlite` y `.konae/` no forman parte de la exportación. Mantener estas exclusiones y probarlas en los bytes exportados y en el destino.

## Formato propuesto

La app conserva fuentes y lockfile en `app/`. Un manifiesto mínimo podría ser el siguiente.

```json
{
  "version": 1,
  "functions": {
    "orders": {
      "entry": "functions/orders.ts",
      "mount": "/api/orders",
      "methods": ["GET", "POST"],
      "access": "authenticated",
      "actions": ["orders.read", "orders.update"]
    },
    "preferences": {
      "entry": "functions/preferences.ts",
      "mount": "/api/preferences",
      "methods": ["GET"],
      "access": "authenticated",
      "actions": []
    }
  }
}
```

`entry` es relativo a `app/`. `mount` admite la ruta exacta y descendientes por segmentos; no interpreta parámetros de Hono. El handler conserva el path lógico de la app. Ausencia de `access` significa `authenticated`; `anonymous` exige declaración explícita. Rechazar solapamientos entre mounts, con HTTP Actions, page paths y archivos estáticos, además de las rutas reservadas. Empezar con GET y POST sin prometer otros métodos.

`functions` tendría `switch: true`, `permission: publish`, `options: []`, `requires: []` y estado inicial apagado. Las declaraciones privadas requieren auth; la lista `actions` no vacía requiere actions y nombres existentes. Una función de preferencias no necesita una base de datos. No inferir la activación del módulo por encontrar código. El estado viaja en `konae.json` y en el manifiesto de exportación existentes.

Añadir `serverArtifact` opcional a `DeployInput`, con versión 1, `abi: "konae-functions/1"`, copia normalizada del manifiesto, mapa de bundles por nombre y sus bytes UTF-8. Compilar un ESM autocontenido por función, con export `handle`; sin imports externos ni resolución npm durante una llamada. Guardar el resultado en `server/manifest.json` y `server/<nombre>.mjs`. El Kernel calcula y verifica el SHA-256; un digest del cliente no autentica código.

Los bundles no deben entrar en `BuildArtifact.files` ni en `dist/`, que el Kernel sirve públicamente. El servidor almacena la versión normalizada publicada en `ReleasedSettings` y `Deployment`; la fuente editable `app/functions.json` puede estar adelantada respecto a lo publicado. No cargar esa fuente como si fuera la versión activa.

## Gate y preview

Añadir `functions-cases.json` al patrón de `module-cases.json`, `module-use-cases.json` y los casos de acciones. Validar versión, claves desconocidas, nombres, rutas, acceso, existencia de entradas, módulos y acciones referenciadas. Los validadores de publicación TS/Rust comprueban también correspondencia manifiesto/bundles, ABI, paths, tamaño y hashes. La Gate necesita recibir los hechos del build para comprobar el artefacto; no puede afirmar que verificó bytes que no recibió.

Registrar los nuevos códigos en `refusal-codes.json`. Reutilizar `module.off` y `module.requires` donde proceda. La Gate analiza declaraciones y las Server Actions existentes, pero no importa ni ejecuta los módulos de servidor. Mostrar que la preview no ejecuta funciones y remitir a `konae dev` o Deployment. Un test con un efecto en el nivel superior del bundle debe demostrar que validar/publicar no lo ejecuta.

El puente inicial solo ofrece identidad verificada e invocación de las acciones permitidas. No entrega headers de autorización, sesiones, SQL del propietario ni una app seleccionable por JavaScript. El Kernel vuelve a comprobar la capacidad de la petición y la allowlist en cada op.

Este alcance no ofrece una transacción arbitraria de varias acciones. Las acciones actuales ejecutan una sentencia; llamar update y audit por separado no sería atómico. Si el SDK final expone `data.transaction`, necesita una implementación nativa propia que comparta restricciones SQL, bindings de actor, límites y deadlines. La prueba del spike de update+audit no demuestra esa integración. No dar por cerrado ese contrato usando únicamente el puente de acciones.

## Publicación y versión

Añadir staging de `server/`, rollback y limpieza a la secuencia de `deploy_input`. La nueva versión debe incluir manifiesto y hashes de ambos artefactos. Un cambio solo de bundle debe producir otra versión y abrirse correctamente tras reiniciar o exportar. No aceptar snapshots V8 ni cachés nativas importadas; las cachés locales pertenecen a `.konae/` y se pueden regenerar.

El dispatcher debe fijar una versión antes de ejecutar. Propuesta mínima para probar atomicidad sin mezclar esquemas: cada petición de función toma lectura de `publish:<id>` durante toda su ejecución; `deploy_input` ya toma escritura de ese nombre para publicar. Tras adquirirlo, la petición vuelve a leer el Deployment activo. La nueva función no entra durante el cambio y la publicación espera las llamadas antiguas, sujetas a deadline. Definir el orden de locks y comprobar que ninguna op vuelve a adquirir `publish:<id>`. Exportación, importación y apertura local deben usar un orden compatible.

El rollback existente cubre fallos inyectados, no prueba por sí solo recuperación ante caída del proceso entre renames. Añadir pasos de fallo para servidor y pruebas de reinicio en los límites del journal antes de afirmar atomicidad frente a crash. Un simple segundo `rename` fuera de esta secuencia no sirve.

## Pruebas reales que deben cerrar el trabajo

1. Ejecutar los mismos casos de declaraciones contra la Gate TS y `runtime::validate_release`, comparando código y mensaje exactos. Añadir casos de módulo apagado, mount reservado, bundle ausente/alterado, ABI desconocida y exceso de tamaño.
2. Arrancar el Kernel mediante `serve` y un listener loopback con SQLite real. Publicar mediante `deploy_input` o el flujo owner MCP. Crear dos cuentas por HTTP real de auth, invocar funciones y comprobar filas propias/ajenas, rechazos y auditoría del camino elegido. No usar las credenciales públicas del spike.
3. Usar los endpoints reales `modules_read` y `module_set`, incluido `expected` y read-back. Confirmar que una app sin funciones no inicializa ejecutores y que una declaración rechazada no crea instancias. Mantener la regla de no apagar módulos en uso; probar apagado tras publicar sin declaraciones.
4. Publicar bundles distintos A/B cuyo resultado cambie. Mantener una llamada A en curso, publicar B y verificar que cada respuesta corresponde a una versión completa. Inyectar cada fallo de staging/swap y reiniciar. Extender `a_release_that_fails_at_any_step_leaves_the_old_actions_on_the_old_rows` con el servidor y sus hashes.
5. Exportar con `package::export` y consumir sus transferencias; importar mediante el flujo real de release/package en una instalación B. Abrir la carpeta con `folder::open`, arrancar otro listener y volver a iniciar sesión con la misma cuenta. Invocar la función importada, verificar datos y reglas, rechazar la sesión antigua y comprobar ausencia de secretos y tokens activos. Mantener una fuente adelantada en A para demostrar que B ejecuta el bundle publicado.
6. Desde HTTP, comprobar que `/server/orders.mjs`, rutas reservadas y rutas de cachés no entregan código del servidor. La preview no crea ejecutores. Las llamadas antiguas no conservan capacidades tras terminar ni saltan a otra app después de publicar.

`AppState::for_test`, `serve`, SQLite y las implementaciones de export/import permiten estas pruebas sin mocks. Hay pruebas existentes de rollback en `lib.rs`, apertura en `folder.rs` y limpieza de identidad en `package.rs`; reutilizar sus fixtures. Esta revisión no las ha ejecutado.

## Tamaño, dependencias y reparto

Límites actuales observados: 20 MiB y 1000 archivos de fuentes; 16 MiB de artefacto estático; 26 MB de JSON por release; 32 MiB por upload ordinario. Proponer para el primer contrato 32 funciones, 1 MiB por bundle y 8 MiB de servidor en total, manteniendo el límite global de release. Esos valores son una propuesta que necesita casos de frontera; no son límites ya implementados ni garantizan que toda combinación quepa en un release.

Los bundles actuales ocupan 816020 bytes para orders, 62018 para preferences y 755347 para schema. Orders ya contiene Zod; el bundle schema separado solo sirve a la referencia Next y no debe viajar como una tercera función. El binario Kernel presente ocupa 29618416 bytes; no se ha reconstruido ni medido un Kernel con V8.

El spike fija deno_core 0.413.0 y extensiones Web; el Kernel aún no depende de Deno. El spike usa rusqlite 0.37.0 y el Kernel 0.39.0 con hooks/limits. Una integración enlazada debe resolver el conflicto de `libsqlite3-sys` y reutilizar el camino de datos del Kernel. Un ejecutor externo evita ese enlace, pero exige IPC, empaquetado y versión de ABI; la prueba debe ejecutar ese proceso real. La elección depende de la contención que prepara el agente Rust, no de un adaptador simulado.

Reparto propuesto, después de acordar manifiesto y ABI:

- Agente de contrato TS: `src/sdk`, Gate, parser/casos TS, builder de servidor, tipos del payload y lectura/escritura de la carpeta. Coordina `functions-cases.json` y códigos con Rust.
- Agente de integración Kernel: parser Rust, módulos, `DeployInput`, `ReleasedSettings`, publicación/rollback, `folder::open`, dispatcher y export/import Rust. Reutiliza identidad y datos existentes.
- Agente de runtime: ejecutor contenido, ABI y capacidades por petición, deadlines/cola/memoria/idempotencia. Entrega una entrada invocable por el dispatcher real del Kernel.
- Agente de verificación: fixtures compartidos, listener Kernel real, owner MCP/read-back, publicaciones A/B, fallos y traslado entre instalaciones. No comparte escrituras de implementación.
- Padre: coordina firmas y locks, integra los resultados y modifica ADR-0018 solo para las garantías observadas. La revisión de contrato puede avanzar mientras se endurece el ejecutor; la prueba final requiere ambos.
