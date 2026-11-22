# Las funciones TypeScript se cargan bajo demanda y usan los servicios del Kernel

Estado experimental. El Kernel ejecuta funciones con un runner configurado por el operador. La adopción en producción sigue pendiente.

Una persona que no escribe código debe poder describir una app y obtener un backend que aplique validación, identidad, autorización y cambios de datos coherentes. Una persona que escribe código debe poder entender y modificar esas mismas funciones. El código generado por un agente no sustituye las restricciones que impone el Kernel.

Las Server Actions actuales expresan consultas y mutaciones declarativas. Algunas apps necesitan lógica que resulta más clara en TypeScript y paquetes npm. Esta propuesta introduce funciones opcionales sin convertir cada app instalada en un proceso o una instancia JavaScript residente.

## Contrato experimental

### La función es una unidad de carga

- Cada función tiene una entrada TypeScript independiente que exporta `handle(request, context)` y devuelve una `Response`. Hono se adapta mediante `app.fetch(request, context)`. Una función Hono puede atender varias rutas relacionadas; no se exige un isolate por endpoint HTTP.
- La app puede compartir código fuente entre funciones. El build resuelve las dependencias y produce los artefactos JavaScript antes de publicar. Las versiones y el lockfile forman parte de la carpeta.
- Hono es una opción de la app. Konae no introduce un framework de rutas ni exige copiar las convenciones de Next.js.
- `functions.json` versión 1 declara nombre, entrada, mount, métodos GET/POST, acceso y acciones permitidas. El acceso omitido exige autenticación. El build genera un `serverArtifact` con ABI `konae-functions/1`; la carpeta guarda su manifiesto y bundles en `server/`, fuera del `dist/` público.

### El módulo no crea instancias para funciones que nadie llama

- El módulo `functions` sigue ADR-0015. El dueño lo gestiona mediante el contrato común de módulos y su configuración viaja con la carpeta.
- Una app con el módulo apagado no crea instancias JavaScript ni tareas de funciones. Declarar o publicar una función tampoco ejecuta su handler.
- La primera llamada que requiere JavaScript carga la función. El Kernel mantiene hasta `KONAE_FUNCTIONS_INSTANCES` procesos, ocho por defecto, y admite hasta `KONAE_FUNCTIONS_ADMISSION` peticiones autenticadas y dentro del límite, 64 por defecto. La admisión empieza después de autenticación y rate limiting; las peticiones admitidas pueden esperar el cuerpo o una instancia. Cada Deployment puede tener hasta `KONAE_FUNCTIONS_APP_SLOTS` procesos, dos por defecto. La caché identifica app, función, versión y bundle mientras el executor use IPC/2. Cada petición evalúa el bundle en un isolate nuevo. Cada proceso atiende una petición a la vez y termina al vencer su TTL, al ceder sitio o al cambiar la versión publicada.
- El supervisor configura 64 MiB de heap y un límite de memoria de 256 MiB por hijo según la política del operador. ADR-0019 fija el límite HTTP actual en 100000000 bytes decimales, entregados por stream al worker. El Kernel conserva el límite de 64 operaciones nativas por petición, hasta 16 KiB de cabeceras de respuesta y hasta 1 MiB para la respuesta codificada. Los 256 MiB se observan por RSS en el modo de desarrollo. El límite de respuesta y el límite de cuerpo son independientes.
- El runner descarta el isolate anterior antes de crear el de la siguiente petición. Las capacidades del actor vencen al terminar su petición. La app guarda el estado duradero mediante Server Actions.
- Una versión nueva no comparte instancias con la anterior. Las llamadas pendientes terminan o se cancelan de forma definida antes de liberar sus recursos.
- El código del motor puede aumentar el tamaño del binario y conservar recursos compartidos. No se promete RSS cero ni devolución inmediata de todas las páginas al sistema operativo.

### El conector TypeScript usa capacidades del Kernel

- El conector inicial expone la identidad verificada y `context.actions.call(name, input)`. Solo admite consultas y mutaciones declaradas sin efectos externos. Archivos, correo, llamadas de red y otros servicios requieren ampliar el contrato y sus pruebas.
- El Kernel fija la app y el actor. Un argumento de JavaScript no permite seleccionar otra app ni suplantar a otra cuenta. Las credenciales de propietario y las herramientas MCP no están disponibles para las funciones.
- Cada función es privada salvo declaración explícita de acceso público. La admisión HTTP no sustituye la autorización de filas que define la app.
- La app valida entradas y aplica sus reglas de negocio. Las operaciones nativas vuelven a comprobar los límites que les corresponden, incluso si el código evita el conector.
- Las consultas con parámetros, los límites de lectura y las transacciones siguen siendo responsabilidad del camino de datos del Kernel. Los errores conocidos mantienen códigos y mensajes compartidos. Un fallo inesperado no devuelve secretos ni una traza interna al cliente.
- Un POST con acciones permitidas exige `Idempotency-Key`. SQLite guarda el recibo junto a los cambios. Si el Kernel pierde la confirmación después de que pueda haber ocurrido el commit, devuelve 409 con `mutation_outcome_unknown`. Si hubo commit, repetir la misma petición con la misma clave devuelve `409 idempotency_replay`. El cliente consulta el estado mediante una lectura con los permisos vigentes.
- No se expone acceso general a archivos, procesos, red o secretos. Las capacidades admitidas pasan por las restricciones de la app y del Kernel.

### El motor queda detrás del conector

`deno_core` con extensiones Web seleccionadas es el candidato del experimento. Aporta V8, operaciones Rust y ejecución asíncrona. No aporta por sí solo toda la compatibilidad Node/npm ni todo el comportamiento de Deno CLI.

La app usa TypeScript y APIs documentadas del entorno. Los paquetes compatibles se resuelven al construir la función. Un paquete que necesita una API no implementada debe fallar con una explicación concreta. No se promete compatibilidad con cualquier paquete npm.

Los snapshots y caches compilados, si se incorporan, los genera la instalación a partir de fuente validada y los liga a la versión del motor. No son el artefacto portable de la app ni se aceptan como estado nativo confiable procedente de una exportación.

La comparación original comparte proceso para medir el coste de integración. La implementación posterior ejecuta JavaScript en un hijo separado y conserva autenticación y SQLite en el padre. Un fallo del hijo no debe terminar el Kernel. El supervisor limita el protocolo y revoca capacidades. Si JavaScript sigue bloqueado tras la cancelación, el runner termina su isolate desde otro hilo. El supervisor mata y recoge al hijo si este no responde.

La política estricta usa un subgrupo cgroup v2 delegado por el operador en Linux. Al arrancar, el Kernel crea y retira un grupo temporal para comprobar que puede escribir los controles requeridos. Cada worker configura memoria, swap y tareas antes de inicializar V8 y rechaza la invocación cuando no puede imponerlos. `memory.max` es un límite del sistema operativo que puede superarse temporalmente; no promete un máximo exacto de RSS en cada instante. La [documentación de cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html) define esa semántica. El modo de desarrollo de macOS usa límite de heap y observación de RSS y no ofrece la misma contención.

El proceso Linux combina cgroup v2 para recursos, UID/GID separados, Landlock para acceso al sistema de archivos y seccomp para llamadas al sistema. El worker no hereda descriptores ni variables del Kernel, y el Kernel conserva solo las capacidades necesarias para crear, terminar y recogerlo. Si el worker no puede instalar Landlock o seccomp, la petición recibe 503 `sandbox_unavailable` y el código no se ejecuta. Esta barrera reduce el acceso del código nativo comprometido, pero no elimina el riesgo de vulnerabilidades del kernel. Las pruebas registradas cubren ambos recorridos HTTP en imágenes ARM64 y builds ARM64/AMD64 de ambos productos. El build Konae AMD64 comprobó el ELF y el hash del runner. Esas rondas Linux preceden al transporte IPC/2 de ADR-0019 y no prueban el streaming de cuerpos bajo Linux. El executor requiere un subárbol cgroup v2 delegado, no systemd; `Delegate=memory pids` permite delegar esos controladores en una instalación basada en systemd; el runtime debe configurar los namespaces por separado. Falta validar la configuración del runtime de despliegue elegido. La adopción sigue experimental.

### Apagado y observación

El handler recibe `Request.signal`. El bundle puede exportar `shutdown(signal)` para liberar recursos al desalojar la instancia. El supervisor concede un margen acotado y después termina y recoge el proceso. El hook no conserva permisos de peticiones anteriores y puede no ejecutarse ante un fallo del motor, agotamiento de memoria o un bucle bloqueado. Matar el hijo no revierte un commit que ya ocurrió.

El Kernel registra latencia por función, versión y presupuesto en Usage. La medida total comienza en admisión y termina al decidir la respuesta; incluye cola, inicialización y operaciones nativas. El panel compara p95 con el límite y muestra percentiles, timeouts, arranques en frío y resultados inciertos. Conserva como máximo 10.000 muestras por app, sin actor, cookies ni cuerpos. La [referencia de Usage](../reference/usage-and-signals.md#function-latency) precisa retención y alcance.

### La salida de Konae admite adaptación limitada

La carpeta exportada conserva código, dependencias, cuentas, datos y reglas según ADR-0001 y ADR-0008. Un Kernel en otra instalación puede ejecutar los bundles importados con su runner local. Las sesiones y secretos quedan fuera del traslado.

Para ejecutar la lógica fuera del Kernel, se permite adaptar el arranque y el conector. Las rutas Hono, esquemas y lógica de negocio deben poder conservarse. La migración de identidad y permisos requiere comprobar equivalencia; que el mismo handler responda en Node no demuestra por sí solo esa equivalencia.

## Relación con las decisiones vigentes

- ADR-0002 seguirá describiendo el camino vigente de Server Actions hasta que esta propuesta se implemente. Una función HTTP sería otra entrada de código de servidor; nunca un endpoint SQL para el navegador.
- ADR-0004 necesita una ampliación explícita antes de publicar funciones. La Gate no puede comprobar estáticamente todas las consultas que construye TypeScript. Los bindings del entorno local y del Kernel deben rechazar las mismas operaciones con los mismos códigos y palabras. El build, los esquemas y las pruebas complementan esa comprobación, pero no la reemplazan.
- ADR-0005 conserva esquema, migraciones y publicación atómica. La selección de versión de funciones debe participar en esa misma publicación.
- ADR-0006 debe incorporar los artefactos de servidor al contrato de publicación. El editor no puede evaluar código de servidor con privilegios de propietario para ofrecer una preview.
- ADR-0011 conserva la ejecución de tareas programadas en el Kernel. Las funciones HTTP de esta ADR no cambian el scheduler ni se ejecutan por temporizador.
- ADR-0013 conserva las reglas de entrada HTTP declarativa y salida de red. Una función HTTP es la excepción experimental para código propio de app en servidor; no sustituye las rutas declaradas ni habilita `fetch` directo.

No se eliminan Server Actions ni se modifica su comportamiento con este ADR. Una sustitución o migración necesita una decisión y pruebas propias.

## Evidencia y condición de adopción

El experimento reproducible vive en [spikes/functions-runtime](../../spikes/functions-runtime/README.md). Su [especificación](../../spikes/functions-runtime/SPEC.md) fija el comportamiento común, la autenticación de prueba y los límites de la comparación.

La decisión de producción exige medir el camino completo con HTTP, autenticación, validación, permisos y SQLite. También exige verificar carga bajo demanda, reutilización, cambio de versión, desalojo y separación entre peticiones. Las cifras antiguas de V8 puro y los bypasses del Kernel no prueban el rendimiento de esta propuesta. Esta ADR sigue en evaluación hasta que se cumplan las condiciones Linux que siguen.

El informe identifica las versiones probadas. `Dockerfile.kernel` y `Dockerfile.konae` ahora empaquetan el runner. El build embebe los JS de las extensiones de Deno desde las crates fijadas en `Cargo.lock`; no necesita el registro ni las fuentes de Cargo al arrancar. El operador aún debe configurar la delegación cgroup y las capacidades del proceso.

### Resultado del experimento

El [informe del 20 de noviembre de 2026](../../spikes/functions-runtime/results/2026-11-20/README.md) conserva cinco rondas alternadas por combinación y 360.000 peticiones medidas sin errores en las rutas válidas. Los p50 HTTP en caliente, como medianas entre rondas con una conexión, fueron los siguientes.

| Integración | GET, ms | POST con transacción y auditoría, ms |
| --- | ---: | ---: |
| Rust nativo | 0,064 | 0,100 |
| Rust + deno_core + Hono | 0,087 | 0,123 |
| Node + Hono | 0,081 | 0,166 |
| Next production | 0,256 | 0,369 |

Deno añadió unos 0,023 ms al camino nativo. Recargar una instancia sin snapshot costó 27,36 ms de mediana y entre 26,08 y 33,94 ms. La reutilización reduce ese coste para llamadas posteriores. El proceso Deno ocupó 4,2 MiB RSS antes de llamar funciones y unos 97 MiB después de las series. La memoria incluye recursos conservados tras desalojar instancias; no es un presupuesto por función.

Estos datos apoyan continuar con `deno_core` detrás del conector, sin fijarlo como requisito de la app. No demuestran superioridad general frente a Node ni una capacidad máxima del servidor. El informe registra variación, CPU, memoria, límites del cliente, versiones SQLite distintas entre Rust y Node y carga ajena en la máquina. Tampoco representan el Kernel de producción ni código de negocio intensivo en CPU.

La [verificación independiente](../../spikes/functions-runtime/checks/review.md) pasa 200 casos en Rust nativo, Deno y Node; Next pasa 196 y falla cuatro por una URL malformada que su router convierte en 500 antes del handler. Los 25 casos de ciclo de vida Deno verifican carga bajo demanda, módulo apagado, reutilización, capacidad, TTL y recuperación después de cortar un bucle síncrono. El cambio de versión probado recrea el mismo bundle con otra clave, no una publicación de código distinto.

### Verificación de la integración

La [prueba original de SQLite](../../spikes/functions-runtime/checks/results/runtime-ops-probe.json) confirmó un pedido y su auditoría después de devolver un error por timeout. En la integración, un presupuesto absoluto cubre la espera por publicación, el cuerpo de la petición, la cola, la inicialización, JavaScript y las operaciones SQLite. Si la petición vence o se abandona antes del commit, SQLite revierte lo pendiente. Si el commit pudo ocurrir, el Kernel conserva la fase incluso si falla la tarea bloqueante y responde con un resultado incierto.

La ronda del Kernel y Konae ejecutó el worker externo en macOS y pasó 23 pruebas. Comprobó dos actores, repetición idempotente entre versiones, rollback tras un error de JavaScript, vencimiento bajo bloqueo SQLite, cancelación al abandonar el servicio y desalojo. Usage contó una muestra por respuesta y distinguió el vencimiento en cola, durante la inicialización y durante la ejecución. [Registro de pruebas](../../spikes/functions-runtime/evidence/2026-11-20-kernel-functions/logs.txt).

El traslado entre dos instalaciones ejecutó el mismo bundle antes y después. La sesión antigua recibió 401 y una sesión nueva de la cuenta importada volvió a ver la misma fila bajo la regla publicada. Al repetir el POST, el Kernel recuperó el recibo importado sin tocar la fila por segunda vez. El borrador adelantado siguió separado. La prueba MCP de `project_import` comprobó esa separación y la restauración desde backup. Otras 28 pruebas cubrieron las rutas, el drenaje para copias, mantenimiento y rollback de publicación. [Registro de regresiones](../../spikes/functions-runtime/evidence/2026-11-20-kernel-functions/regressions.txt).

La [ronda final del runner](../../spikes/functions-runtime/evidence/2026-11-20-worker-final/logs.txt) pasó 19 casos HTTP y 17 del worker. Probó límites de respuesta, memoria de desarrollo y operaciones, revocación de capacidades, cancelación y el hook de apagado. La [ronda de TypeScript](../../spikes/functions-runtime/evidence/2026-11-20-ts-artifact-parity-v2/README.md) pasó 485 pruebas. El Kernel pasó los mismos 25 casos de declaraciones y 58 de artefactos con los mismos códigos y mensajes. La [ronda de Usage](../../spikes/functions-runtime/evidence/2026-11-20-latency-ui/README.md) verificó el panel con datos de prueba en escritorio y móvil.

### Condiciones pendientes para producción

The [earlier Linux ARM64 run](../../spikes/functions-runtime/evidence/2026-11-20-linux-v8-final/README.md) ran a real V8 worker under delegated cgroup v2. The [hardening run](../../spikes/functions-runtime/evidence/2026-11-20-production-hardening/README.md) ran the same runner from a clean image without a checkout or Cargo. It checked Landlock, seccomp, separate worker IDs, empty worker capabilities, a clean environment and closed inherited descriptors. The runner cannot open files, connect to the network, change cgroups or signal the supervisor. Linux requires Landlock ABI 2 and cgroup v2. macOS uses development policy only.

El supervisor compara `oom_kill` y `oom_group_kill` en `memory.events` antes de atribuir un EOF a memoria. Con el runner empaquetado, un OOM real del cgroup produjo `worker_memory_limit`; un SIGKILL ordinario produjo `worker_failed` sin incremento OOM. Ambas rutas recogieron el proceso y retiraron su subgrupo. La ronda también forzó `pids.max=64`, mató un supervisor con un worker activo, comprobó `PDEATHSIG`, reaped el subgrupo huérfano al iniciar el siguiente worker y preservó un grupo hermano. El registro y los hashes están en la [evidencia de hardening](../../spikes/functions-runtime/evidence/2026-11-20-production-hardening/README.md).

Una ronda anterior de integración Linux ejerció KernelHTTP, sesión, autorización SQLite, transacciones, idempotencia, publicación, timeout, OOM y recuperación con su runner y revisión de código identificados en los registros. No corresponde al transporte IPC/2 de ADR-0019. La prueba de traslado de esa ronda ejecutó el bundle importado con identidad restaurada, recuperó el recibo sin duplicar la mutación y rechazó la sesión antigua.

En una comprobación posterior, un runner candidato ARM64 empaquetado arrancó en una imagen Kernel bajo cgroup delegado y pasó health sin apps. Dos pruebas HTTP separadas, ejecutadas por un arnés con ese runner empaquetado, pasaron casos de autenticación, SQLite, OOM y cuerpos de 100.000.000 bytes. El arnés no envió peticiones de una app a través de la imagen final. La configuración de un servicio de destino tampoco se probó. Consulta [Functions research](../architecture/functions-research.md#linux-checks-and-the-current-branch) para separar revisiones y alcance.

The [report](../../spikes/functions-runtime/evidence/2026-11-20-production-hardening/README.md) records small authenticated V8 request samples, each with one cold call and five warm calls. They are not comparable benchmarks or production measurements. Adoption remains experimental. The operator procedure is [Operate an installation](../operate.md#strict-linux-deployment-check). It requires Linux x86-64 or ARM64, cgroup v2, Landlock ABI 2 (Linux 5.19) or later, seccomp, a delegated subtree, and private cgroup and PID namespaces. systemd can delegate `memory` and `pids` with `Delegate=`, but the executor does not depend on systemd. Before enabling traffic, verify the contract inside the target runtime with its architecture-matched image and real entrypoint. Send an authenticated SQLite read, confirm a sandbox refusal before bundle delivery, and confirm cleanup stays within that installation. The evidence tests the cgroup contract in a disposable environment. It does not prove a production installation or every runtime configuration.

## Alternativas

- `rusty_v8` directo deja en Konae el mantenimiento del bucle asíncrono, recursos y APIs que `deno_core` ya ofrece. Se conserva como referencia de coste, no como primera integración.
- QuickJS merece otro experimento si el presupuesto de memoria impide adoptar V8. Esta comparación HTTP no lo mide; hay que comprobar su coste de cálculo y compatibilidad antes de elegirlo.
- QuickJS u otro motor dentro de Wasmtime añade una barrera de aislamiento alrededor del invitado, con otro coste de integración y ejecución. Sigue siendo una alternativa si la contención manda sobre la compatibilidad o el rendimiento.
- `deno_runtime` reutiliza más APIs, pero incorpora más capacidades y servicios de resolución que configurar. Puede ser preferible si la demanda de paquetes Node supera el conjunto Web elegido.
- workerd aporta un runtime Workers existente. Exige integrar otro ejecutable y sus bindings con los servicios de la app.
- Node o Bun externos ofrecen un entorno más completo. Añaden gestión de procesos y comunicación con el Kernel; no se descartan si mantener compatibilidad propia resulta más costoso.

## Consecuencias

El SDK, la plantilla del agente y las pruebas deben enseñar y comprobar validación, autorización y coherencia de datos. Una persona sin conocimientos de backend no tiene que pedir esas propiedades. Las preguntas al usuario se centran en reglas de negocio, como quién puede cancelar un pedido.

Mantener instancias calientes cambia el coste de memoria y la duración de los globales. Las funciones deben seguir siendo correctas después de una carga en frío, un desalojo o una publicación. El estado duradero pertenece a los datos de la app.

El alcance inicial de APIs y paquetes se documentará y probará. La calidad del backend y su portabilidad se evalúan por comportamiento verificado, no por el aspecto del código generado.
