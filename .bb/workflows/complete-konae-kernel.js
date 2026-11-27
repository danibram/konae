export const meta = {
  "name": "complete-konae-kernel",
  "description": "Completar el refactor con entregas pequeñas, utilizadas y verificadas por Lunas high.",
  "phases": [
    {
      "title": "Mutación y aviso atómicos"
    },
    {
      "title": "Entrega recuperable de avisos"
    },
    {
      "title": "Cola portable de Calls"
    },
    {
      "title": "Continuaciones y efectos"
    },
    {
      "title": "Despacho y capacidad"
    },
    {
      "title": "Publicación con pendientes"
    },
    {
      "title": "Migración de la instalación"
    },
    {
      "title": "Pruebas portables y mapa"
    },
    {
      "title": "Revisión y cierre"
    }
  ]
};

const resultSchema = {
  type: "object",
  properties: {
    status: { type: "string", enum: ["complete", "needs_work", "blocked"] },
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
const recovery = [
  "Continúas el workflow wfr_0d88ca04-875d-44aa-a5c4-11fff9289cb3 que terminó incompleto. Lee /Users/dbr/.bb/thread-storage/thr_pgj2uuptz3/kernel-refactor-recovery/previous-results.json. Hay código de estado, Context, admisión y equidad de Calls, pero la durabilidad sigue pendiente.",
  "La fase anterior se declaró bloqueada porque necesitaba modificar recorridos existentes. Esas modificaciones están autorizadas y son el trabajo. Puedes editar y ampliar archivos existentes para completar TU entrega.",
  "Tienes una entrega pequeña con criterios verificables. Completa sus cambios y sus pruebas. Continúa hasta terminarlos. Si hay requisitos de fases posteriores, descríbelos como tales. Usa blocked solo para una dependencia externa o decisión que realmente impida seguir. Si queda código de tu entrega por escribir, devuelve needs_work con los puntos exactos.",
  "Antes de editar revisa el código actual. Las primeras fases previas declararon complete, pero el código aún puede tener omisiones. Arregla las que sean necesarias para TU entrega. No vuelvas a desarrollar desde cero el trabajo existente.",
  "No cambies .bb/workflows. Este workflow tiene un único escritor por vez. El coordinador mantiene la fuente. No ejecutes formato de archivos enteros. Respeta las pruebas y el trabajo previo del usuario.",
  "Verifica y corrige fallos antes de entregar. Tus pruebas deben atravesar la entrada normal que cambias. Guarda evidencia de comandos en tu thread storage y devuelve rutas. Informa cualquier dependencia aguas abajo con el nombre preciso."
].join("\n");

const stages = [
  {
    "phase": "Mutación y aviso atómicos",
    "task": "Implementa solo el registro transaccional de intenciones de notify. Mira actions/execution.rs: run_mutation aún hace commit sin el contexto de notify. Añade una operación de mutación que reciba el contexto necesario y prepare un registro portable antes de confirmar. Guarda ID estable, declaración de aviso, datos de RETURNING, entrada, actor y cuenta verificados, fecha y revisión. Registra su tabla en el inventario de módulos y permite acceso interno acotado mediante el autorizador existente. Conecta el camino normal de MCP, HTTP, schedule y continuaciones de Calls. Mantén su respuesta, acceso y guardas. Las Functions que hoy rechazan notify deben conservar esa validación hasta habilitar el contrato de efectos con sus pruebas. TransactionEffect o su sustituto debe usarse realmente en el commit. Para conservar la entrega durante la transición, la llamada postcommit debe consumir el registro ya guardado, con estado pendiente si no puede asegurar la entrega. La fase siguiente completará deduplicación del transporte y recuperación. No envíes correo dentro de la transacción. Prueba modificación+intención atómicas y ninguna intención tras rollback, guard refusal o fallo de captura. Prueba al menos dos entradas reales, no solo helpers. Completa esta entrega aunque las fases posteriores aún falten."
  },
  {
    "phase": "Entrega recuperable de avisos",
    "task": "Completa el recorrido del registro de notify creado por la fase anterior hasta mail.sqlite. installation/mail.rs::record_notice genera un UUID nuevo en cada llamada; añade una operación de entrega idempotente con ID de la intención y destinatario, preservando su API actual donde se usa para otros fines. Guarda recibos de transferencia/resultados para que reiniciar entre encolar y confirmar no vuelva a encolar ni a consumir presupuesto. Un error de almacenamiento o transporte deja pendiente recuperable. Resuelve destinatario activo y verificado, filas por destinatario y límites usando las reglas de notify. No conserves autoridad de una sesión como permiso general. La intención portable debe poder reconstruir una entrega en otro Kernel, conservando estados terminales y estado incierto cuando se pierde el ACK de SMTP. Define y prueba qué debe pausarse al entregar responsabilidad entre instalaciones. Reutiliza recuperación de arranque/import/resume y temporizador de reintento de la forma más pequeña; el dispatcher de capacidad común se extraerá después. Prueba corte antes/después de la transferencia, repetición del mismo ID, destinatario revocado, testbox y export/import sin origen. El correo de autenticación sigue su contrato transaccional existente. No dupliques las entregas antiguas al mantener compatibilidad temporal."
  },
  {
    "phase": "Cola portable de Calls",
    "task": "Mueve los nuevos trabajos de Calls al estado SQLite de la app como fuente de verdad. Conserva callId y contrato público, modos live/test, consultas locales de konae events/test y límites existentes. Cada trabajo guarda declaración aceptada, input, revisión y definición de su continuación, sin copiar secretos de instalación al payload portable. Cambiar la publicación no debe cambiar URL, método, cuerpo o continuación de un trabajo aceptado. Registra tablas y formatos en el inventario de la fase de estado. Cambia enqueue, claim, retry, lectura de estado y proceso para usar el estado de app. Export/import debe llevar filas y contratos. Puedes conservar el lector de la cola vieja para que la fase de migración la convierta o drene, sin dos consumidores del mismo ID. Prueba un Call A pendiente, publicar B, completar el envío usando A y recuperar la cola tras export/import. Ejecuta pruebas de Calls y escenarios test existentes y corrige sus recorridos. La atomicidad de continuación se cierra en la siguiente entrega."
  },
  {
    "phase": "Continuaciones y efectos",
    "task": "Haz que una continuación de Calls confirme su SQL, el recibo de ejecución y cualquier intención de notify en una sola transacción de la app. Un reintento tras confirmar SQL debe ver el recibo y evitar repetir la mutación. Conserva input, declaración y resultado de la llamada fijados por la fase anterior. Reutiliza la conexión y los límites de ejecución común, sin abrir otra transacción ni readquirir bloqueos. Guarda respuesta de proveedor antes de ejecutar la continuación y recupera un corte entre ambos. Conecta la participación transaccional de efectos al contrato de Functions solo si corresponde a la capacidad requerida y con la misma validación Gate/Kernel y pruebas de contrato; ningún runner debe obtener acceso directo a tablas internas, credenciales o red. Prueba una continuación que incrementa un contador y solicita notify, corta tras commit, reintenta y verifica un incremento y una intención. Verifica rollback ante guarda y error SQL. La pérdida de ACK externa mantiene el resultado incierto que corresponda."
  },
  {
    "phase": "Despacho y capacidad",
    "task": "Extrae el reparto común a partir de los consumidores reales de Calls y notify ya implementados. El dispatcher entrega trabajo al handler del módulo; solo un handler que necesite procesos llama al supervisor. Mantén límites globales, por recurso y por app, así como equidad y admisión antes de aceptar pendientes sin límite. Un único propietario de app confirma resultados por ID e intento. Conserva pending hasta confirmar y rechaza resultados de un intento ya sustituido. El aviso tras commit y un temporizador de reintentos despiertan apps listas; arranque/import/resume reconstruyen trabajo y vencimientos. Evita barridos continuos de todas las apps vacías. Integra mantenimiento, apagado, export/copy y límites de SQL sin mantener bloqueos mientras haces red. Mejora la ejecución común si las fases previas solo añadieron estructuras sin unificar las decisiones reales. Prueba dos apps, una saturada y otra lista, recuperación de un aviso perdido al reiniciar y recursos de correo separados del trabajo CPU. No agregues Documents ni Typst para fingir que existe un segundo consumidor: usa los consumidores reales."
  },
  {
    "phase": "Publicación con pendientes",
    "task": "Integra el plan y activación de publicación con los contratos pendientes. Valida el esquema candidato contra SQL retenido de continuaciones y jobs que lo necesitan. Rechaza un cambio incompatible antes de tocar la release viva, con diagnóstico compartido donde corresponda en Gate y Kernel. Completar pendientes o migrarlos de manera explícita desbloquea el cambio. Un cambio de CSS o uno de esquema compatible debe poder publicarse. Retén los recursos realmente necesarios y expórtalos con la app hasta completar el trabajo. Además resuelve una sola revisión por petición: reglas de acceso, bytes y ETag coherentes durante activación y rollback usando los bloqueos/revisiones existentes con el menor tiempo de retención. Prueba pausa de publicación, GET concurrente, rollback y otra app respondiendo. Prueba job A que necesita una columna eliminada por B, rechazo sin modificación de A y publicación posterior al terminar el job."
  },
  {
    "phase": "Migración de la instalación",
    "task": "Implementa y ensaya la migración específica de las colas/formato anteriores al nuevo modelo para la única instalación Konae en producción. Usa copias locales y fixtures, nunca la instalación real. La herramienta hace preflight, comprueba consumidores pausados, crea backup y convierte de forma reejecutable conservando IDs, contenido, cuenta destinataria y estado pendiente/terminal/incierto. Inspecciona cifrado de mail.sqlite y la clave de instalación; no exportes secretos a la app ni asumas que una clave nueva puede descifrar los bytes antiguos. Si no puede reconstruirse una declaración antigua, ofrece una vía de drenaje comprobada usando la release anterior y rechaza una conversión que invente los datos. Prueba interrupción y reanudación, segunda ejecución sin duplicados, conservación de archivos y rollback al backup. Documenta comandos concretos y breve procedimiento de relevo. Conserva los formatos de app que pueden convertirse sin datos perdidos, con errores claros para los que requieren preflight."
  },
  {
    "phase": "Pruebas portables y mapa",
    "task": "Completa las ideas de thr_dd39y2nd7c recogidas en Refactor acceptance. Reutiliza konae test y el replay Club para una prueba de permiso permitido/denegado con fixtures sintéticos que viaje con su revisión fuera de dist. El informe distingue la revisión de fuentes probada y la release activa cuando difieren. Ejecuta sobre datos descartables y sesiones nuevas después de export/import. Diagnostica el primer fallo del replay invitation frente a closed según sus declaraciones y el comportamiento del Kernel, sin debilitar la política ni las assertions. Actualiza el mapa COMPLETO y architecture.md a lo implementado: ejecución común, handlers, inventario de tablas, despacho y compatibilidad de publicación. Mantén todos los módulos existentes en el diagrama y las futuras capacidades marcadas propuestas. Regenera el SVG y verifica lectura/render, sin desplegar."
  },
  {
    "phase": "Revisión y cierre",
    "task": "Audita y corrige la integración de todo el refactor, incluidas las fases de la ejecución anterior que declararon complete. Comprueba que Context aplica límites e identidad, que los módulos y las entradas usan la ejecución común, que los efectos pertenecen al commit, que hay recuperación/export real y que la migración evita duplicados. Revisa la excepción actual de permisos basada en Job con prefijo schedule: y reemplaza cualquier confianza en nombres por autoridad explícita preservando los contratos actuales. Sigue HTTP/MCP, Functions, cron, Calls, notify, export/import y limpieza. Ejecuta las suites pertinentes, build del editor, conformidad de Functions con un worker real si está disponible y docs. Corrige las regresiones. Elimina el formato masivo introducido durante el refactor, especialmente tests.rs, sin perder pruebas nuevas ni los cambios previos de usuario guardados en la copia de referencia. Actualiza resultados con pruebas ejecutadas y omisiones concretas. Solo complete cuando estén cubiertas las garantías aceptadas; no declares cierre por haber pasado cargo check."
  }
];

const results = [];
for (const stage of stages) {
  phase(stage.phase);
  const context = results.length ? "\nEntregas anteriores de esta continuación:\n" + JSON.stringify(results) : "";
  const prompt = shared + "\n" + recovery + "\nTu entrega:\n" + stage.task + context;
  let result = await agent(prompt, {
    provider: "codex", model: "gpt-6-luna", reasoningLevel: "high",
    label: stage.phase, phase: stage.phase, schema: resultSchema
  });
  if (result.status !== "complete") {
    log("La entrega " + stage.phase + " necesita completar: " + result.remaining.join(" "));
    result = await agent(prompt + "\nCompleta los puntos pendientes concretos. El implementador anterior dejó este estado:\n" + JSON.stringify(result), {
      provider: "codex", model: "gpt-6-luna", reasoningLevel: "high",
      label: "Completar " + stage.phase, phase: stage.phase, schema: resultSchema
    });
  }
  results.push({ phase: stage.phase, ...result });
  if (result.status !== "complete") {
    throw new Error("Entrega pendiente en " + stage.phase + ": " + result.summary + " " + result.remaining.join(" "));
  }
}
return results;
