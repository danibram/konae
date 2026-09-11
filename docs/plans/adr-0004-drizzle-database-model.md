# Plan de implementación de ADR-0004: modelo Drizzle gestionado

## Resultado buscado

Cada Managed Project tendrá cinco fuentes editables, sustituyendo `schema.sql` por un `schema.ts` Drizzle restringido. Usuario y agente editarán ese único Database Model; Littlebox generará una migración SQL revisable, la probará contra una copia de los datos, creará un backup, la aplicará y registrará la nueva revisión. El runtime del navegador seguirá accediendo a datos exclusivamente mediante Server Actions.

El cambio debe ser incremental. No se eliminará el camino `schema.sql` hasta que los proyectos existentes puedan convertirse y verificarse sin perder estructura ni datos.

## Punto de partida

- `ProjectFile` fija actualmente `schema.sql` como uno de cinco ficheros en `src/lib/types.ts`.
- El editor, el agente y su prompt conocen literalmente esos cinco nombres.
- `ValidationGate` ejecuta `schema.sql` dos veces con `sql.js`, lo aplica sobre una copia de la base local y sólo detecta divergencias; no genera una migración.
- El preview vuelve a ejecutar el esquema al abrir su base.
- El Kernel guarda el Capsule dentro de cada Deployment, crea un SQLite por Deployment y ejecuta `schema.sql` durante `deploy()`.
- La exportación de SQLite existe, pero los backups automáticos, la retención y el restore todavía no están implementados.

## Decisión previa necesaria

Antes de habilitar migraciones sobre datos alojados debe resolverse en un ADR separado quién posee la base persistente:

- **Recomendación:** un Project Environment posee database y file storage; los Deployments inmutables referencian ese Environment y una revisión esperada del esquema.
- **Modelo actual:** cada Deployment posee su propio SQLite y blobs. En este modelo publicar crea otra base, por lo que no existe una continuidad clara sobre la que migrar.

ADR-0004 puede avanzar hasta generación, preview, Development Kernel y conversión de proyectos sin esta decisión. La aplicación alojada debe permanecer desactivada hasta cerrarla.

## Seam principal

Crear un módulo profundo `database-lifecycle` con dos operaciones para sus callers:

```ts
interface DatabaseLifecycle {
  planChange(input: PlanDatabaseChange): Promise<MigrationPlan>;
  applyChange(input: ApplyDatabaseChange): Promise<AppliedSchemaRevision>;
}
```

`planChange` incluye compilación del modelo, generación SQL, validación de la migración y clasificación de riesgo. `applyChange` comprueba que el plan y la revisión de origen siguen vigentes, obtiene la aprobación necesaria, crea el recovery point, aplica y verifica. Preview, publicación, Admin Interface y Development Kernel no deben coordinar Drizzle, backups y transacciones por separado.

Tipos mínimos del seam:

- `DatabaseModelSource`: contenido de `schema.ts`, dialecto fijado y versión de toolchain.
- `SchemaRevision`: hash del modelo, snapshot Drizzle, cabeza de migraciones y fecha de aceptación.
- `MigrationPlan`: identidad inmutable, revisión origen/destino, SQL generado, diagnósticos, riesgos y acciones afectadas.
- `AppliedSchemaRevision`: revisión aceptada, migración aplicada y recovery point asociado.
- `Approval`: decisión explícita para rename, pérdida de datos u otra operación ambigua; nunca un booleano implícito.

Adaptadores internos:

- `DrizzleSchemaCompiler`: ejecuta `generate`, `check`, `pull --init` para conversiones y produce artefactos deterministas.
- `DatabasePersistence`: clona, crea recovery points, aplica SQL y restaura. Tendrá al menos adaptadores `sql.js` y Bun/SQLite; un futuro D1 será otra implementación del mismo seam.
- `MigrationRepository`: conserva planes, SQL, snapshots, hashes, revisiones y estado de aplicación.

Drizzle debe quedar localizado en este módulo. No se repartirán llamadas a Drizzle Kit por el editor, rutas HTTP o Deployment Store.

## Fases

### 0. Spike técnico y contrato soportado

1. Fijar versiones exactas compatibles de `drizzle-orm` y `drizzle-kit`; no usar rangos para artefactos persistentes.
2. Probar en Bun el ciclo SQLite completo: `schema.ts` → `generate` → SQL/snapshot → `check` → apply con `drizzle-orm/bun-sqlite`.
3. Probar `pull --init` sobre un SQLite creado desde el `schema.sql` actual y comparar tablas, columnas, índices, constraints y foreign keys.
4. Verificar cómo Drizzle comunica renames interactivos y diseñar una salida no interactiva `requires-approval`; Littlebox nunca debe aceptar automáticamente un rename ambiguo.
5. Definir el subconjunto permitido en Managed: imports exclusivamente desde el módulo SQLite fijado, exports de tablas/relaciones y ausencia de drivers, variables de entorno, red, filesystem y efectos laterales.
6. Medir arranque frío/caliente de Drizzle Kit y decidir entre proceso efímero y pool de workers.

**Salida:** prueba automatizada de ida y vuelta y una fixture de artefactos. Si el CLI no proporciona un seam estable, encapsularlo como comando sustituible; no importar módulos internos no documentados por toda la aplicación.

### 1. Versionar el Capsule y añadir `schema.ts`

1. Añadir versión explícita al formato de fuente del Project, separada de `CapsuleManifest.version`.
2. Definir Capsule v2 con `schema.ts` en lugar de `schema.sql`; mantener un lector v1 durante la transición.
3. Añadir un starter Drizzle equivalente a la tabla `tasks` actual.
4. Guardar en el Project la revisión de esquema aceptada y la versión exacta del toolchain que produjo sus artefactos.
5. Hacer que normalización, import/export, reset y persistencia conserven correctamente v1 y v2.

**Aceptación:** un proyecto nuevo se guarda, recarga y exporta con `schema.ts`; un proyecto v1 sigue abriendo sin modificación destructiva.

### 2. Implementar `database-lifecycle` en aislamiento

1. Implementar `planChange` con workspace efímero, configuración generada por Littlebox y caché por `(toolchain, previousSnapshotHash, modelHash)`.
2. Rechazar imports o código fuera del contrato antes de evaluar `schema.ts`.
3. Ejecutar la evaluación en proceso aislado, sin red ni credenciales, con timeout, límites de CPU/memoria/salida y directorio temporal propio.
4. Normalizar el resultado de Drizzle a `MigrationPlan`; no filtrar rutas temporales ni mensajes internos al editor.
5. Aplicar el plan a un clon, comparar el esquema final con el modelo objetivo y preparar todas las Server Actions.
6. Implementar `applyChange` con control de concurrencia por base, compare-and-swap sobre la revisión origen e idempotency key.

**Aceptación:** crear tabla, añadir columna nullable, añadir índice y cambiar constraint producen planes deterministas. Dos aplicaciones del mismo plan no duplican una migración y dos planes concurrentes no pueden avanzar desde la misma revisión.

### 3. Integrar editor, preview y agente

1. Añadir soporte TypeScript/Drizzle a CodeMirror, incluyendo tipos fijados, autocomplete y diagnósticos locales en worker.
2. Mostrar `schema.ts` como el modelo por defecto en Code y Schema Studio.
3. Sustituir todas las listas, instrucciones y heurísticas del agente que mencionan `schema.sql`; `schema.ts` y `actions.json` siguen siendo los ficheros de riesgo.
4. Mantener una única herramienta del agente, `validate_capsule`; internamente llamará a `planChange` y devolverá diagnósticos y resumen de la migración, no detalles del CLI.
5. Aplicar el SQL de un plan válido únicamente sobre un clon del SQLite de preview. El último preview válido permanece activo si el nuevo modelo no compila o no converge.
6. Añadir una vista read-only del SQL generado con riesgos, impacto y revisión origen/destino.

**Aceptación:** un agente añade una columna en `schema.ts`, obtiene feedback tipado, valida, ve el SQL generado y actualiza el preview sin editar migraciones ni usar shell.

### 4. Backups y aplicación en Development Kernel

1. Implementar `DatabasePersistence.createRecoveryPoint()` correctamente para WAL; no copiar un `.sqlite` abierto ignorando `-wal` y `-shm`.
2. Persistir backup, metadatos, checksum, esquema origen y migración destino antes del primer cambio real.
3. Aplicar la migración en transacción cuando SQLite lo permita, verificar el resultado y registrar la revisión sólo al final.
4. Inyectar fallos después de backup, a mitad de migración, antes del journal y después del journal para probar recuperación e idempotencia.
5. Exponer list/restore mediante Admin Interface, nunca Runtime Interface.
6. Añadir límites de tamaño, retención y auditoría.

**Aceptación:** una migración válida conserva datos; una migración fallida no anuncia la nueva revisión; el recovery point puede restaurarse y volver a ejecutar las Server Actions anteriores.

### 5. Convertir proyectos `schema.sql` existentes

1. Ejecutar el `schema.sql` histórico en un SQLite temporal y usar introspección para crear `schema.ts`, snapshot y migración baseline.
2. Comparar ambos esquemas mediante metadata SQLite normalizada, no mediante igualdad textual del DDL.
3. Conservar el SQL original como artefacto histórico. Los `INSERT` de seed se mantienen en una migración custom; no se confunden con el Database Model.
4. Clasificar triggers, views u objetos no representables. Preservarlos como migración custom sólo si puede demostrarse que futuras migraciones no los eliminan; en caso contrario bloquear la conversión con un informe accionable.
5. Preparar todas las Server Actions contra la base convertida y comparar datos/contadores antes de marcar el baseline como aceptado.
6. Mantener rollback al Capsule v1 hasta que el primer cambio v2 se aplique correctamente.

**Aceptación:** el starter y las fixtures representativas se convierten sin diferencias estructurales ni pérdida de filas; un caso no soportado falla cerrado y conserva el proyecto v1.

### 6. Integrar publicación alojada

Esta fase comienza después del ADR sobre propiedad de datos.

1. Separar persistencia del Environment de los ficheros inmutables del Deployment si ésa es la decisión adoptada.
2. Hacer que Deployment referencie `schemaRevisionId` y rechazar publicación cuando acciones/artefacto esperan otra revisión.
3. Sustituir `db.exec(hosted.files["schema.sql"])` por `databaseLifecycle.applyChange(...)` en el plano administrativo.
4. Serializar publicación y migración por Environment; el runtime puede seguir sirviendo la revisión anterior mientras se valida el plan, pero no mezclar acciones nuevas con esquema antiguo.
5. Definir la estrategia de compatibilidad: migraciones expand/contract para cambios incompatibles o ventana de mantenimiento explícita para las apps sencillas iniciales.
6. Registrar actor, plan, aprobación, backup, migración, resultado y Deployment asociado.

**Aceptación:** publicar una revisión compatible migra una sola vez y cambia el Deployment sólo después de verificarla; un fallo mantiene código, esquema y acciones anteriores operativos.

### 7. Eject y operación externa

1. Exportar `schema.ts`, `drizzle/` completo, versión fijada, `drizzle.config.ts`, scripts `db:generate`, `db:migrate`, `db:check` y configuración del Development Kernel.
2. No incluir credenciales ni una URL administrativa persistente.
3. Añadir comando autenticado para solicitar un túnel administrativo corto cuando el propietario quiera operar la base alojada.
4. Detectar y mostrar drift antes de aceptar un nuevo Build Artifact o migración externa.
5. Documentar que `push` es una decisión explícita del External Project Owner y que rompe las garantías del historial exportado.

**Aceptación:** el proyecto eyectado instala dependencias, crea una base local desde cero, ejecuta sus migraciones y funciona con el Development Kernel sin cambios en el frontend.

## Seguridad

- Tratar `schema.ts` como código no confiable aunque su interfaz sea pequeña.
- Compilar/evaluar fuera del proceso del Kernel que guarda secretos y sesiones.
- Bloquear imports dinámicos, top-level await, acceso a globals peligrosos y cualquier módulo no allowlisted.
- No aceptar `drizzle.config.ts` aportado por un Managed Project; Littlebox genera la configuración.
- No enviar una base alojada al navegador para validarla. El Kernel valida contra un snapshot server-side.
- Firmar o hashear `MigrationPlan`; `applyChange` debe negarse si cambió modelo, acciones, revisión origen o SQL.
- Requerir aprobación específica para cada riesgo, no un “aceptar todo” reutilizable.

## Rendimiento

- Diagnóstico sintáctico/tipado calentado en el editor: objetivo p95 menor de 150 ms.
- Debounce de generación canónica; no ejecutar Drizzle Kit en cada tecla.
- Caché content-addressed de modelo/snapshot y pool caliente si el spike demuestra una mejora clara.
- Preview conserva el último plan válido y aplica el SQL sobre un clon; nunca bloquea la edición esperando un deploy.
- Objetivo inicial para un esquema pequeño: plan canónico calentado p95 menor de 1 s, medido antes de fijarlo como SLO.
- `drizzle-orm` y `drizzle-kit` no deben entrar en el bundle runtime de las mini-apps. Sólo los tipos/editor pueden cargarse de forma lazy.

## Matriz mínima de pruebas

- **Modelo:** tabla, PK autoincremental, FK, unique, índice, default y relación.
- **Evolución:** add nullable, add required con backfill, rename ambiguo, drop destructivo y recreación de tabla SQLite.
- **Datos:** preservación de filas, constraints fallidas, seeds y blobs SQLite.
- **Acciones:** acción válida, columna eliminada, binding incompatible y rollback a acciones anteriores.
- **Concurrencia:** dos planes desde la misma revisión, reintento tras timeout y apply duplicado.
- **Recuperación:** fallo antes/después de backup, fallo parcial, journal ausente y restore verificado.
- **Seguridad:** import prohibido, bucle infinito, salida excesiva, lectura de entorno, filesystem y red.
- **Compatibilidad:** conversión v1 limpia, v1 con seeds y v1 con objeto no representable.
- **E2E:** usuario y agente editan `schema.ts`, aprueban migración, conservan datos, publican y eyectan.

## Orden recomendado de entregas

1. Spike Drizzle + contrato soportado.
2. Capsule v2 y starter con `schema.ts`.
3. `planChange` determinista y validación de acciones.
4. Editor/agent/preview sobre migraciones generadas.
5. `applyChange`, backups y restore en Development Kernel.
6. Conversión segura de proyectos v1.
7. ADR de Project Environment y persistencia compartida.
8. Aplicación alojada y publicación coordinada.
9. Eject Drizzle completo y túnel administrativo temporal.

Cada entrega debe dejar un camino funcional de extremo a extremo; no se introducirán simultáneamente un editor `schema.ts` y un Kernel que todavía sólo entienda `schema.sql` sin un adaptador de compatibilidad.

## Definición de terminado

- `schema.ts` es la única fuente editable del esquema en todos los Managed Projects nuevos.
- El agente puede modificarlo y validar sin terminal ni conocimiento de Drizzle Kit.
- Toda mutación de esquema tiene plan SQL, revisión origen/destino, aprobación cuando corresponda, backup y audit record.
- Preview, Development Kernel y hosting ejercitan el mismo interface de lifecycle con adaptadores distintos.
- El Kernel no vuelve a ejecutar un esquema declarativo al abrir una base.
- Los proyectos v1 se convierten de forma verificada o permanecen intactos con un diagnóstico accionable.
- Eject produce un proyecto Drizzle convencional que puede recrear la base local y continuar su historial.

## Referencias técnicas

- [Drizzle Kit generate](https://orm.drizzle.team/docs/drizzle-kit-generate): genera SQL y snapshots comparando el modelo actual con el historial previo.
- [Drizzle Kit check](https://orm.drizzle.team/docs/drizzle-kit-check): comprueba consistencia del historial generado.
- [Drizzle Kit pull](https://orm.drizzle.team/docs/drizzle-kit-pull): introspecciona una base existente y `--init` permite establecer un baseline.
- [Drizzle con Bun SQLite](https://orm.drizzle.team/docs/get-started/bun-sqlite-existing): adaptador soportado para el Kernel actual basado en `bun:sqlite`.
