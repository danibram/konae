# App compartida del experimento

Estos handlers implementan el workload de [SPEC.md](../SPEC.md). Node y el host Rust con Deno importan los mismos bundles. El conector solo cubre dos operaciones de pedidos y no es un SDK de Konae.

```sh
npm --prefix spikes/functions-runtime/app ci
npm --prefix spikes/functions-runtime/app run check
npm --prefix spikes/functions-runtime/app run build
```

El build produce tres módulos ESM independientes en `dist/`, que Git ignora. `orders.js` incluye Hono y Zod; `preferences.js` incluye Hono. `schema.js` permite a Next usar los mismos esquemas. No se resuelven paquetes durante una petición.

Cada función exporta `handle(request, context): Promise<Response>`. La URL conserva la ruta completa del SPEC, por ejemplo `http://localhost/a/orders/1`. El host autentica la cookie contra SQLite antes de llamar al handler y limita el cuerpo a 16384 bytes para este workload. La integración del Kernel tiene un límite separado de 100000000 bytes, descrito en [ADR-0019](../../../docs/adr/0019-functions-stream-request-bodies-with-bounded-ipc.md).

[contract.ts](src/contract.ts) fija el contrato del adaptador.

```ts
import { handle } from './dist/orders.js';

const response = await handle(request, {
  actor: { userId: authenticatedActor },
  data: {
    readOrder: (id) => readAuthorizedOrder(database, authenticatedActor, id),
    updateOrder: (id, quantity) => updateAndAudit(database, authenticatedActor, id, quantity),
  },
});
```

El adaptador captura la base y el actor autenticado. El handler no puede elegir un actor para las operaciones. El adaptador debe filtrar por propietario en SQL y agrupar actualización y auditoría en una transacción.

`orderIdSchema` convierte un identificador válido de texto a número. `quantitySchema` valida un objeto estricto con una propiedad `quantity`, entera entre 1 y 10. Ambos salen de `src/schema.ts` y de `dist/schema.js`.

| Dependencia | Versión |
| --- | --- |
| Hono | 4.13.19 |
| Zod | 4.6.7 |
| esbuild | 0.28.2 |
| TypeScript | 5.9.3 |

`package-lock.json` fija las dependencias transitivas. Los bundles y `node_modules` no forman parte del código para revisión.
