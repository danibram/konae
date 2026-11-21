# Referencias Node y Next.js

Este paquete implementa las referencias HTTP de `../SPEC.md`. Next tiene una divergencia conocida para escapes URL inválidos. No demuestra conformidad total ni seguridad para producción. Los servidores solo escuchan en `127.0.0.1`.

Node importa `handle` de `../app/dist/orders.js` y `../app/dist/preferences.js`. Son los mismos bundles Hono que ejecuta Deno. `node/server.mjs` aporta el transporte y `lib/host.mjs` autentica, limita el body y entrega un adaptador SQLite ligado al actor. No existe una copia local del handler Hono.

Next ejecuta route handlers compilados con `next build` y servidos con `next start`. `next/handlers.mjs` implementa los endpoints e importa `orderIdSchema` y `quantitySchema` desde `../app/dist/schema.js`. Ambos runtimes usan el esquema Zod del mismo código fuente. Next no usa el servidor de desarrollo.

| Componente | Versión |
| --- | --- |
| Node probado | 26.0.0 |
| Node mínimo declarado | 22.13.0, por `node:sqlite` sin flag experimental |
| SQLite incluido en Node probado | 3.53.0 |
| V8 incluido en Node probado | 14.6.202.33-node.19 |
| Next | 16.4.6 |
| React y React DOM | 19.2.0 |
| Hono | 4.13.19 |
| Adaptador Hono para Node | 1.19.17 |
| Zod dentro del bundle compartido | 4.6.7 |

`package-lock.json` fija las dependencias de este paquete. El lockfile de `../app` fija las del bundle compartido. Las pruebas se ejecutaron con Node 26.0.0; no se verificó Node 22.13.0.

## Compilar y arrancar

Desde la raíz del checkout, con Node y npm en PATH, prepara primero los bundles compartidos y después Next.

```sh
npm --prefix spikes/functions-runtime/app ci --prefer-offline --no-audit --no-fund
npm --prefix spikes/functions-runtime/app run build
npm --prefix spikes/functions-runtime/baselines ci --prefer-offline --no-audit --no-fund
NEXT_TELEMETRY_DISABLED=1 npm --prefix spikes/functions-runtime/baselines run build
cd spikes/functions-runtime/baselines
```

Node no requiere una compilación propia. Estos comandos usan directorios de datos distintos y arrancan en primer plano. Ejecútalos en terminales separadas y detén cada servidor con Ctrl-C.

```sh
NODE_ENV=production PORT=43103 DATA_DIR="$PWD/data/node" node node/server.mjs
NODE_ENV=production PORT=43104 DATA_DIR="$PWD/data/next" node next/start.mjs
```

`next/start.mjs` solo lanza el CLI `next start` con el mismo ejecutable Node, el puerto configurado y el host loopback. Reenvía SIGINT y SIGTERM al hijo. Los valores predeterminados de los puertos son 43103 y 43104. Usa `DATA_DIR` absoluto, porque Next puede cambiar el directorio de trabajo.

Hay una sola instalación de dependencias en `baselines/node_modules`, unos 340 MiB durante esta prueba. El build de Next ocupa unos 78 MiB. El build usa un worker y no genera una distribución standalone que copie dependencias. Ningún comando usa ni modifica `/tmp/konae-fn-spike/next-app`.

## Datos y transacciones

Cada proceso abre una conexión persistente para `a.sqlite` y otra para `b.sqlite` en su `DATA_DIR`. Node las abre al arrancar. Next las abre al atender su primera petición, incluida `/health`, y las conserva mediante un singleton del proceso. El esquema y las filas iniciales se instalan una sola vez por apertura, nunca por petición. Un reinicio conserva cantidades y auditoría existentes.

Ambos usan WAL, `synchronous=FULL`, `busy_timeout=5000` y sentencias preparadas persistentes. Autentican consultando la tabla `sessions` de la app seleccionada. El adaptador liga el actor a cada lectura y actualización mediante `owner_id = ?` en SQL. `BEGIN IMMEDIATE` engloba `UPDATE ... RETURNING` e inserción de auditoría; cualquier fallo revierte ambos cambios.

El host autentica antes de validar ID, JSON o tamaño del body. Cuenta los bytes recibidos, también con transferencia chunked. Las respuestas de los handlers conservan el header `set-cookie`. La lectura de cookies usa el parser de Hono en ambas referencias.

## Pruebas y alcance

```sh
npm --prefix spikes/functions-runtime/baselines test
```

El comando arranca y detiene ambos servidores de producción en 43303 y 43304. Crea datos temporales dentro de `.tmp/` y los elimina al terminar. Necesita el build previo de Next y los bundles compartidos. No ejecuta carga.

La prueba cubre salud, rutas desconocidas, sesiones ausentes/ajenas/caducadas, autenticación antes de validación, IDs, JSON estricto, aislamiento por actor y app, 16384/16385 bytes, lectura tras escritura y auditoría. También instala un trigger que fuerza el fallo de auditoría para comprobar que la actualización se revierte. Las preferencias se prueban con y sin cookies y headers. El cliente usa HTTP sin añadir `Accept-Language`, por lo que puede verificar el valor `null`.

Next devuelve un 500 de texto `Internal Server Error` ante `/a/orders/%ZZ`, antes de ejecutar el route handler. Node devuelve el 400 JSON definido en SPEC. Next también se salta la autenticación del handler en esa ruta malformada. La prueba local conserva la expectativa de SPEC como un caso pendiente que falla; no trata el 500 como conformidad. No se ha añadido un proxy ni un servidor personalizado para alterar ese resultado.

La última ejecución local registró 17 pruebas correctas y una expectativa fallida marcada como pendiente para Next.

La suite independiente de `../checks/verify.py` obtuvo 200 comprobaciones correctas para Node y 196 correctas con 4 fallos para Next. Los fallos de Next corresponden a `%ZZ` por GET y POST en ambas apps. La suite confirmó que esos fallos dejan intactas las bases. Sus resultados viven en `../checks/results/node.json` y `../checks/results/next.json`.

La prueba no cubre carga, cancelación, procesos múltiples sobre el mismo directorio, reinicios durante transacciones ni agotamiento de recursos. POST no tiene idempotencia. Next compila e incluye el esquema compartido en su build, mientras Node importa el bundle al arrancar; si cambia `../app`, recompila Next antes de comparar.
