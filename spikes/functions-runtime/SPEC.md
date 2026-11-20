# Prueba de funciones TypeScript bajo demanda

Este código es experimental. No cambia el Kernel ni habilita funciones en las apps publicadas.

## Pregunta

¿Permite `deno_core` ejecutar funciones Hono independientes, reutilizarlas y desalojarlas, con un coste razonable frente a Rust y Next.js, conservando autenticación, validación y autorización?

## Comparación

- Un servidor Rust con dos modos, `native` y `deno`, comparte HTTP, autenticación, SQLite y formato de respuestas. El modo `deno` utiliza `deno_core` real y APIs Web de Deno, no los polyfills mínimos del experimento anterior.
- Node con Hono y Next.js en modo producción implementan el mismo comportamiento sobre copias independientes de la misma base de datos.
- Cada servidor escucha solo en loopback y acepta `PORT` y `DATA_DIR`. Los puertos por defecto son 43101 para Rust, 43102 para Deno, 43103 para Node y 43104 para Next.
- El cliente verifica cuerpos y efectos, cuenta errores y conserva cada ronda. Las mediciones de carga se ejecutan en serie, después de terminar las compilaciones.

## Datos y HTTP

Cada servidor crea dos bases SQLite, `a.sqlite` y `b.sqlite`, en su `DATA_DIR`. La app se selecciona por el primer segmento de la ruta. Esto es enrutamiento del experimento, no el modelo de dominios del producto.

Tablas mínimas:

```sql
CREATE TABLE orders (id INTEGER PRIMARY KEY, owner_id TEXT NOT NULL, quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 10));
CREATE TABLE sessions (token TEXT PRIMARY KEY, user_id TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE audit (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL, actor_id TEXT NOT NULL, quantity INTEGER NOT NULL);
```

En cada app existen el pedido 1 de `alice`, con cantidad 2, y el pedido 2 de `bob`, con cantidad 4. Las sesiones son credenciales públicas exclusivas del experimento. La app a admite `a-alice` y `a-bob`; la b admite `b-alice` y `b-bob`. Caducan en 4102444800. Cada app tiene también `<app>-expired`, caducada en 1. No se utilizan cuentas ni datos reales.

- `GET /health` responde `{"ok":true}`.
- Las rutas de pedidos requieren cookie `session`. Una sesión ausente, desconocida, caducada o de otra app responde 401 con `{"error":"unauthorized"}`.
- `GET /<app>/orders/<id>` valida un entero positivo de hasta nueve dígitos. Si el identificador es inválido responde 400 con `{"error":"invalid_input"}`. Si el pedido no existe o pertenece a otro usuario responde 404 con `{"error":"not_found"}`. Si puede leerlo responde 200 con `{"id":1,"ownerId":"alice","quantity":2}`, según la fila.
- `POST /<app>/orders/<id>` acepta únicamente `{"quantity":<entero entre 1 y 10>}`. Rechaza JSON inválido y campos extra con el mismo 400. Actualiza el pedido autorizado e inserta una fila de auditoría en una transacción. Responde con el pedido actualizado. No implementa idempotencia en esta primera prueba; debe registrarse como una garantía pendiente.
- Autenticación antes de validación en todos los servidores. No filtrar por propietario únicamente después de una mutación. Un fallo no modifica las tablas.
- `GET /<app>/preferences` es una segunda función independiente, con la misma autenticación. Lee la cookie `theme` y el header `accept-language`. Responde `{"theme":"system","language":null,"userId":"alice"}`, sustituyendo los valores presentes, y escribe `seen=1; Path=/; HttpOnly; SameSite=Lax` como cookie. No usa una instancia de pedidos.
- El tamaño máximo del body para estos servidores de benchmark es 16384 bytes. Un exceso responde 413 con `{"error":"body_too_large"}`. Este contrato del workload es distinto del límite de entrada de Functions del Kernel, que acepta 100000000 bytes según [ADR-0019](../../docs/adr/0019-functions-stream-request-bodies-with-bounded-ipc.md).
- Las rutas no declaradas responden 404. Los errores inesperados responden 500 con `{"error":"internal_error"}` sin trazas.

## Ciclo de vida experimental

En el servidor Deno, una función tiene clave app + nombre + versión. Una instancia atiende una petición cada vez. Las apps y funciones sin llamadas no crean instancias. Una caché con capacidad y tiempo de inactividad configurables desaloja instancias que no estén ejecutándose. La configuración `FUNCTIONS_ENABLED=0` debe permitir comprobar que no se crea ningún runtime. El experimento debe explicar qué parte del motor se inicializa incluso sin instancias.

El servidor Rust expone `/__stats` y controles de prueba solo en loopback. El implementador documentará el formato exacto. Permitir observar creación, reutilización, desalojo y cambio de versión. La instrumentación no es una API propuesta para Konae.

La prueba debe distinguir el límite temporal real de un simple timeout de una future. Si se prueba un bucle JS infinito, hacerlo en un proceso hijo que el cliente pueda terminar. No ejecutar pruebas de agotamiento de memoria sin contención.

## Entregables y límites

Se conservan código, versiones fijadas, comandos de reproducción, pruebas de comportamiento y datos de rendimiento. El ADR describe una propuesta, sus resultados y los bloqueos para producción. Una prueba no demuestra ausencia de vulnerabilidades ni convierte los bypasses del antiguo Kernel en una referencia válida.
