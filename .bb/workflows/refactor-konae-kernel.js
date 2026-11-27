export const meta = {
  name: "refactor-konae-kernel",
  description: "Implementar la arquitectura común de Konae con GPT-6-Luna high y verificar los recorridos existentes.",
  phases: [
    { title: "Estado y almacenamiento" },
    { title: "Ejecución común" },
    { title: "Trabajo durable" },
    { title: "Publicación y migración" },
    { title: "Integración y pruebas" }
  ]
};

const resultSchema = {
  type: "object",
  properties: {
    status: { type: "string", enum: ["complete", "blocked"] },
    summary: { type: "string" },
    files: { type: "array", items: { type: "string" } },
    checks: { type: "array", items: { type: "string" } },
    remaining: { type: "array", items: { type: "string" } }
  },
  required: ["status", "summary", "files", "checks", "remaining"],
  additionalProperties: false
};

const shared = [
  "Trabajas en /Users/dbr/htdocs/miniapp-builder, en el checkout compartido del workflow. IMPLEMENTA código y verifica el resultado.",
  "El usuario autoriza dividir e implementar el refactor del Kernel del mapa completo de docs/architecture.md. Solo existe una instalación Konae en producción y acepta una migración específica. El objetivo es añadir módulos sin volver a modificar cada entrada, el migrador, la exportación y los permisos SQL.",
  "Lee AGENTS.md, docs/architecture.md y los archivos afectados. Lee ~/.dotfiles/skills/unslop/SKILL.md y aplica sus reglas en toda prosa. No subdelegues. No hagas commit, push, release ni operaciones sobre producción. La migración de esta entrega se prepara y ensaya localmente. No imprimas secretos.",
  "El checkout ya tenía 86 rutas modificadas por otros trabajos, incluidos cron de Functions, latencias y controles. Presérvalos. Hay copia de referencia en /Users/dbr/.bb/thread-storage/thr_pgj2uuptz3/kernel-refactor-baseline. No restaures archivos completos desde HEAD. Cada fase ve las ediciones de las anteriores. Solo hay un implementador de este workflow activo cada vez.",
  "Conserva el modelo de app portable, identidad por app, SQL autorizado por fila y mismo rechazo en Gate y Kernel. Mantén el contrato independiente del motor JavaScript y sus adaptadores. Lee las skills de TypeScript si editas TypeScript.",
  "El usuario pidió incorporar ideas de thr_dd39y2nd7c. Lee la sección Refactor acceptance de docs/architecture.md. Las fases de ejecución, publicación y verificación deben incluir el contrato de aceptación de resultados del worker, cuerpo/ETag/regla de una misma release y un primer caso portable de permisos ligado a la revisión probada.",
  "Este trabajo implementa la base común y los tres cierres de la review. Documents y el adaptador FTPS son los siguientes consumidores, todavía propuestos. No implementes la lógica de facturación ni los parsers de vecin.dad en el Kernel.",
  "Review aceptada: (1) HTTP, MCP, Functions, cron y continuaciones necesitan ejecución común con app, release, actor o encargo, permisos, límites y transacción; dispatcher llama handlers y estos usan transportes o supervisor. (2) fijar código de un job no fija el esquema; publicación debe comprobar pendientes antes de migrar. (3) tablas reservadas, versiones y migraciones del módulo deben estar registradas y ser conocidas por migrador, autorizador y exportación; estado de módulos puede existir con Actions apagado.",
  "Prefiere interfaces pequeñas, llamadas Rust dentro del proceso y reutilización de lo existente. No introduzcas un broker externo, un motor de workflows de apps o plugins dinámicos. Las reglas de cada módulo siguen dentro de ese módulo.",
  "Lleva la fase hasta una implementación utilizada desde los recorridos normales y probada. Evita helpers sin consumidor, stubs, TODOs que difieran requisitos de la fase y cambios cosméticos. Añade pruebas de regresión para garantías nuevas y ejecuta las existentes pertinentes. Registra comandos y resultados reales.",
  "No declares complete si quedan requisitos de TU fase pendientes. remaining puede describir dependencias explícitas de fases posteriores, distinguiéndolas de bloqueos. Si necesitas una decisión rutinaria, resuélvela usando el código y el objetivo. Si hay un bloqueo concreto irrecuperable, devuelve blocked y el motivo exacto."
].join("\n");

const stages = [
  {
    phase: "Estado y almacenamiento",
    task: [
      "Implementa el registro de estado por módulo y su uso real en persistencia y almacenamiento.",
      "1. Sustituye las listas dispersas de tablas internas por un inventario de propietarios, nombres reservados y versiones usado por schema_change, schema_transform y el autorizador SQLite. Incorpora las tablas existentes de schema y recibos de Functions. Protege lectura/escritura/DDL de app SQL y evita que schema.sql borre estado de módulos.",
      "2. El ciclo de vida debe distinguir uso habilitado y estado persistido. Mantén y transfiere el estado que exista, con errores explícitos para versiones requeridas desconocidas. Un módulo que persiste datos puede abrir almacenamiento sin activar Actions.",
      "3. Encauza el inventario de componentes exportables y referencias de blobs por el registro, empezando por los módulos reales y Files. Export/import, copies, limpieza y files-migrate deben consultar el mismo inventario de objetos vivos y cancelar borrados si no puede leerse. Conserva las ACL de Files dentro de Files. Evita una interfaz vacía que el exportador nunca use.",
      "4. Mantén los bytes y IDs de datos existentes. Un formato nuevo necesario tendrá versión y conversión explícita reutilizable por la fase de migración.",
      "Verifica tabla interna preservada tras publicación, acceso SQL denegado, snapshot/export/import existentes y Files local/S3 según las pruebas disponibles. Reporta las interfaces que usarán las siguientes fases."
    ].join("\n")
  },
  {
    phase: "Ejecución común",
    task: [
      "Implementa una ejecución común de operaciones y transacciones a partir de Actions y Functions.",
      "1. Extrae del acoplamiento a Functions el contexto de ejecución y las garantías de transacción, cancelación, resultado incierto e idempotencia. El contexto fija app, release, actor o encargo, autoridad y límites. Cada operación sigue aplicando la autorización y las reglas de su módulo.",
      "2. Haz que runtime MCP, HTTP actions, Functions, schedule/cron y continuaciones de Calls compartan la ejecución relevante, guardas y efectos. No dupliques la decisión entre cada entrada. Conserva las diferencias declaradas entre actores humanos, anónimos y encargos programados.",
      "3. La ejecución dentro de una transacción existente debe reutilizar conexión y presupuesto sin readquirir sus bloqueos. Las funciones que agrupan acciones conservan su atomicidad. La idempotencia debe mantener el alcance de actor/operación, fingerprint y comportamiento de reintento actual.",
      "4. Usa el registro de estado de la fase previa. Prepara la participación tipada de efectos en el mismo commit para la fase siguiente. Esta fase debe seguir compilando y conservar los contratos actuales de los efectos mientras se integra la outbox durable.",
      "5. Todas las rutas SQL comparten admisión por app e instalación. El permiso vive hasta que acaba la ejecución real, aunque el cliente se desconecte. Las consultas síncronas siguen un camino directo.",
      "Prueba identidad, allowlists, rollback, guardas, timeout/cancelación, reintento y una operación equivalente invocada por los diferentes recorridos. Reporta API y límites de transacción para los handlers durables."
    ].join("\n")
  },
  {
    phase: "Trabajo durable",
    task: [
      "Implementa trabajo durable portable y despacho común usando Calls y notify como consumidores reales.",
      "1. Guarda las intenciones de efectos de app junto con la mutación que los promete, en la misma SQLite y transacción. Registra tablas con la fase de estado. Publica el aviso al dispatcher después del commit y recupera pendientes al reiniciar/importar. Un rollback no debe dejar un envío pendiente.",
      "2. Mantén identidad estable de operación, formato del payload y contrato/revisión necesarios para reanudar. Calls debe conservar la declaración del envío y la continuación aceptadas para que una republicación no cambie un envío pendiente. Prepara una consulta de contratos pendientes para el plan de publicación de la fase siguiente.",
      "3. El dispatcher reparte turnos entre apps y tiene límites por recurso y por app. Invoca handlers de módulos. Los handlers de red usan transporte y los que requieren un proceso usan el supervisor. No hagas que todas las tareas pasen por IPC. Las colas en memoria son reconstruibles desde estado durable.",
      "4. Conserva el control de destinatarios de notify. El correo de cuenta conserva su transacción con identidad y la cola de instalación existente según corresponda. Si el envío usa mail.sqlite como transporte, define una transferencia idempotente por ID estable y no reconozcas una intención como acabada antes de asegurar su recuperación. Conserva el resultado incierto de SMTP.",
      "5. Una continuación SQL registra su resultado y efectos de forma que un reinicio no repita la mutación. Los reintentos externos siguen sujetos al contrato de cada proveedor. Las apps sin trabajo quedan inactivas tras recuperación y no necesitan un proceso propio.",
      "6. Implementa conversión local recuperable de colas actuales o una vía de drenaje comprobable que la fase siguiente pueda usar para el único despliegue. No descartes datos ni conviertas estados inciertos en éxito.",
      "Prueba cortes entre commit y aviso, reentrega, continuidad tras export/import, dos apps con trabajo y equidad, mail en testbox y Calls con transporte local simulado. El alcance es la base de jobs y sus consumidores existentes."
    ].join("\n")
  },
  {
    phase: "Publicación y migración",
    task: [
      "Implementa la compatibilidad entre releases, esquema y trabajo pendiente, y prepara la migración de la única instalación.",
      "1. Antes de activar esquema y código, comprueba los contratos retenidos por el trabajo pendiente contra el esquema candidato. Una continuación antigua que usa una columna eliminada debe impedir la activación con rechazo explicativo y mismo código/mensaje en las puertas aplicables. Conserva la release actual ante rechazo.",
      "2. Los jobs que solo consumen snapshots pueden avanzar con sus recursos fijados. Retén bundles/declaraciones/recursos necesarios hasta que los jobs concluyan, y transfiérelos con la app. Separa ese estado de caches nativas regenerables.",
      "3. Permite completar pendientes antes de una migración incompatible o transformar explícitamente su estado mediante una migración definida. No ejecutes silenciosamente el handler actual para un contrato viejo. Evita rechazar publicaciones de CSS o cambios de esquema compatibles.",
      "4. Escribe una migración específica y reejecutable para los formatos anteriores que realmente cambiaron en este workflow. Debe trabajar sobre una copia local, comprobar precondiciones, respaldar, preservar IDs, cuentas, permisos y bytes, y dar verificación y recuperación concretas. Puedes simplificar compatibilidad histórica por ser un único despliegue, pero los datos existentes necesitan una ruta real y ensayada.",
      "5. Actualiza docs/architecture.md y el mapa completo para representar lo implementado, incluyendo ejecución común, dispatcher hacia handlers, registro de tablas y comprobación de pendientes durante publicación. Mantén Documents/FTPS como propuestos. Regenera el SVG desde el Mermaid.",
      "Ensaya una carpeta anterior con datos sintéticos y trabajo pendiente, migra dos veces, verifica que las filas y envíos no se duplican y prueba export/import. Da el comando exacto del ensayo y un runbook breve con preflight, aplicación y rollback sobre backup. No accedas a producción."
    ].join("\n")
  },
  {
    phase: "Integración y pruebas",
    task: [
      "Eres un implementador nuevo que debe comprobar el refactor completo y corregir problemas de integración.",
      "Lee las entregas anteriores y el diff real. Sigue los recorridos normales desde SDK/MCP, HTTP, Functions, cron, Calls, notify, publicación, export/import y limpieza. Busca los tres defectos originales y cualquier bypass introducido.",
      "Corrige las omisiones de alcance, errores y regresiones que encuentres. Verifica que las interfaces nuevas tienen consumidores reales, que la app no accede a tablas internas y que no queda un inventario de tablas o blobs divergente.",
      "Ejecuta los tests de Rust y TypeScript relevantes, build del editor, verificaciones del sitio y pruebas de regresión añadidas. Usa cargos/checks existentes, inspecciona fallos y separa con evidencia cualquier fallo preexistente del refactor. No debilites pruebas para obtener verde.",
      "Repite el ensayo de migración desde datos anteriores y el caso pendiente incompatible al publicar. Comprueba que cambiar solo presentación conserva los trabajos y que el import puede recuperarlos. Verifica que los archivos de otros trabajos previos siguen presentes.",
      "La entrega final debe incluir archivos cambiados, resumen de lo implementado, comandos/resultados de comprobación, límites reales y cualquier bloqueante. Solo devuelve complete si las cinco fases están integradas y los requisitos pueden comprobarse. No despliegues ni publiques."
    ].join("\n")
  }
];

const results = [];
for (const stage of stages) {
  phase(stage.phase);
  const context = results.length ? "\nEntregas previas del mismo checkout:\n" + JSON.stringify(results) : "";
  const prompt = shared + "\n\nTu entrega:\n" + stage.task + context;
  let result = await agent(prompt, { provider: "codex", model: "gpt-6-luna", reasoningLevel: "high", label: stage.phase, phase: stage.phase, schema: resultSchema });
  if (result.status !== "complete") {
    log("La fase " + stage.phase + " requiere resolver un bloqueo antes de continuar.");
    result = await agent(shared + "\nResuelve la entrega bloqueada y termínala.\n" + stage.task + "\nResultado previo:\n" + JSON.stringify(result) + context,
      { provider: "codex", model: "gpt-6-luna", reasoningLevel: "high", label: "Resolver " + stage.phase, phase: stage.phase, schema: resultSchema });
  }
  results.push({ phase: stage.phase, ...result });
  if (result.status !== "complete") {
    throw new Error("Bloqueo en " + stage.phase + ": " + result.summary + " " + result.remaining.join(" "));
  }
}
return results;
