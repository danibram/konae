# Prototipos con un mini kernel portable

Propuesta revisada el 11 de octubre de 2026. No cambia los ADR aceptados ni implementa funciones nuevas.

## Objetivo y recomendación

El usuario describe una app al agente, prueba el resultado en el navegador y publica una URL pública. La app puede tener login, zonas internas y datos privados. Cada app tiene su propia base, usuarios y permisos. El creador puede seguir usando Littlebox, autoalojar el mismo mini kernel o sustituir sus capacidades por código propio.

Recomiendo un kernel compartido que ejecute acciones sobre una SQLite activa por app y entorno, guardada en un volumen persistente. El frontend es un artefacto estático. Los backups y la exportación materializan la base como objetos portables. El almacén de objetos admite filesystem, S3 y posteriormente GCS sin intervenir en cada escritura.

El formato portable de la base y su ejecución son decisiones distintas. Una SQLite puede exportarse como un objeto y, mientras está activa, recibir transacciones normales del kernel. No hace falta subirla en cada mutación ni convertir el almacenamiento de objetos en el coordinador de transacciones.

## Crítica de las opciones

Los criterios son tiempo hasta el primer prototipo, carga de operación, facilidad de desarrollo para el agente y coste de salir de Littlebox.

| Opción | Qué resuelve | Qué complica | Decisión |
| --- | --- | --- | --- |
| SQLite activa y backups separados | Escrituras normales, kernel pequeño y exportación de un archivo estándar | Requiere volumen persistente; perderlo puede perder cambios posteriores al backup | Elegir para v1 |
| SQLite activa y replicación incremental | Reduce el retraso de la copia remota sin subir toda la base por escritura | Añade gestión del log, restauración y monitorización | Evaluar cuando exista una necesidad de recuperación medida |
| Snapshot completo y head remoto por mutación | Hace del object store la autoridad de cada estado confirmado | Coste proporcional al tamaño de la DB, latencia remota e incertidumbre al confirmar | Descartar para este producto inicial |
| Backend arbitrario por app | Permite cualquier código y librería de servidor | Procesos, aislamiento, dependencias y despliegues por app | Reservar como salida externa |
| Base pública descargada en el navegador | Permite servir un dataset de lectura con archivos estáticos | No impone permisos por fila sobre los datos descargados ni coordina usuarios que escriben | Solo para datasets deliberadamente públicos |

La primera opción tiene límites reales. Una máquina caída deja temporalmente indisponibles las acciones y el login. Un disco perdido exige restaurar. Una app con consultas lentas puede consumir recursos compartidos. Estos límites se afrontan con backups verificables, límites de consulta y una instancia responsable por base; no con un protocolo distribuido en la primera versión.

La replicación incremental puede apoyarse en herramientas existentes. Litestream replica cambios de SQLite en segundo plano y mantiene material de recuperación. No convierte la aplicación en un sistema de múltiples escritores ni garantiza por sí solo que todo commit local ya esté en el destino remoto. [Funcionamiento de Litestream](https://litestream.io/how-it-works/).

## Camino de una petición

```text
Navegador -> acción + input -> kernel compartido -> SQLite de esa app
                                  |
                                  +-> sesión y permisos de esa app

Servidor estático / CDN -> HTML, JS, CSS y assets
Proceso de backup -> snapshot consistente -> filesystem / S3 / GCS
```

1. El navegador envía un nombre de acción y sus parámetros.
2. El kernel resuelve la app desde el dominio o una ruta configurada y el usuario desde su sesión.
3. Comprueba la declaración, permisos, input y límites.
4. Ejecuta SQL parametrizado dentro de una transacción de la base correspondiente.
5. Responde después del commit. No espera a S3 ni copia toda la base.

El navegador no recibe credenciales de base ni puede elegir otro usuario o archivo SQLite. El estado anónimo debe estar representado de forma explícita.

Una cola por base puede coordinar mutaciones, migraciones y restauraciones sin bloquear otras apps. SQLite WAL permite lectores concurrentes con un escritor. El volumen y la configuración de sincronización deben sostener la garantía del commit local. No se abre la misma base desde réplicas independientes en varias máquinas. [SQLite WAL](https://www.sqlite.org/wal.html).

## Persistencia y objetos

| Elemento | Mientras funciona la app | Objeto portable |
| --- | --- | --- |
| Frontend | Servidor estático, disco o CDN | Artefacto inmutable |
| Datos | SQLite activa en volumen persistente | Snapshot SQLite consistente |
| Auth y permisos | Almacén de identidad aislado por app | Exportación de usuarios y permisos de esa app |
| Archivos | Disco o almacén de objetos | Blobs más inventario |
| Modelo y cambios | Esquema aceptado e historial de migraciones | Fuente y SQL versionado |

El backup se ejecuta por petición del propietario, antes de migraciones y con una frecuencia configurada. El panel muestra la fecha y revisión del último backup completado. Si un backup falla, aumenta la antigüedad del punto recuperable; la escritura ya confirmada no se convierte en error.

Un intervalo programado no garantiza ese mismo límite de pérdida si las copias fallan. El punto recuperable real es el último backup válido. Un reinicio del proceso conserva la base en el volumen; la pérdida del volumen es un fallo distinto. Un contenedor con disco efímero no cumple esta propuesta.

El snapshot usa una operación consistente de SQLite, como Online Backup API o VACUUM INTO. No se copia a ciegas solo el archivo principal de una base activa con WAL. Los backups tienen retención y una prueba de restauración. [SQLite Backup API](https://www.sqlite.org/backup.html).

La primera exportación completa puede pausar brevemente las escrituras de la app y sus cambios de identidad para copiar datos, usuarios, permisos y referencias de archivos en un punto coordinado. Restaurar exige cerrar conexiones anteriores, validar la copia y activar el estado restaurado. No hay que restaurar automáticamente una copia remota antigua sobre un volumen sano.

## Contrato pequeño para el agente

El agente trabaja con archivos de la app, un modelo de datos, acciones declaradas y permisos. No necesita elegir drivers, configurar almacenamiento ni instalar un backend nuevo para cada prototipo.

```text
index.html
styles.css
app.js o src/main.ts
schema.ts
actions.json
littlebox.manifest.json
migrations/                # resultado revisable del lifecycle
```

La propuesta conserva Drizzle como modelo, según ADR-0004. Volver ahora a schema.sql como segunda autoridad cambiaría otra decisión sin resolver el problema de persistencia. El SQL generado queda disponible para inspección y exportación. Las primeras plantillas y herramientas del agente deben cubrir un subconjunto pequeño del modelo.

El agente dispone de un SDK pequeño. Estos métodos son el contrato propuesto; hoy algunas capacidades aún difieren entre preview y kernels.

```js
await app.auth.session();
await app.auth.signIn();
await app.auth.signOut();
await app.actions.call('notes.create', { title: 'Compra' });
await app.files.upload(file);
```

El editor ofrece el formulario de login inicial, la simulación de usuarios y plantillas de acciones comunes. El agente modifica el código de la app y propone cambios de modelo y permisos. El gate comprueba build, esquema, migración y acciones antes de aplicar. La plataforma realiza la administración; el agente no adquiere una clave administrativa dentro del código generado.

Para acceso a datos hay tres diseños posibles.

| Diseño | Ventaja | Crítica |
| --- | --- | --- |
| Acciones nombradas con SQL parametrizado | Ya existe, concentra cada operación y permite sustituirla por nombre | Tiene límites para lógica compleja; el SQL debe comprobar acceso a las filas |
| CRUD genérico desde el cliente | Reduce código en demos simples | Obliga a diseñar filtros, joins y políticas generales; cambia la plataforma a un motor de acceso a tablas |
| Funciones JS de servidor libres | Permite lógica convencional | Ejecutar código del usuario exige aislamiento y operación adicionales |

Mantendría las acciones existentes. Para un patrón frecuente que resulte verboso, añadiría generación o una plantilla revisable. No ampliaría actions.json hasta convertirlo en un lenguaje general. Una operación que necesita varias escrituras debe ser atómica en el backend; si el formato inicial no la soporta, se declara esa limitación y no se simula con varias llamadas del navegador.

## Permisos y auth

La entrada de la app es pública. Las zonas internas solicitan sesión y todas las operaciones protegidas se autorizan en el servidor. El frontend público nunca incluye datos privados ni secretos.

Cada app tiene usuarios y sesiones propios, separados de las cuentas del editor. El kernel comparte la implementación de auth sin compartir identidad entre apps. Los roles iniciales pueden limitarse a propietario de app y miembro, con acciones públicas, autenticadas y restringidas.

La sesión identifica al actor, pero no convierte cualquier consulta en segura. Una acción que devuelve notas privadas debe filtrar por el actor obtenido del kernel. Una mutación debe aplicar la condición de acceso al registro que modifica. La validación necesita casos de dos usuarios y dos apps, además de SQL válido. No se anuncia aislamiento de filas automático solo por usar parámetros.

El almacén de auth no debe ser accesible mediante el SQL de las acciones. Empezaría con un archivo de identidad separado por app, gestionado por el kernel. Eso simplifica el aislamiento y exige coordinarlo con la base de datos al exportar. El mecanismo inicial de login debe reutilizar una implementación mantenida; no construiría varios proveedores ni flujos distintos antes de cerrar uno completo.

## Abstracciones y salida

Dos entregables públicos bastan al principio.

- Un SDK JavaScript importable, con módulos de auth, acciones y archivos.
- El mismo mini kernel en una distribución autoalojable con configuración, volumen y herramientas de administración.

Modelo, migraciones, exportación y adapters pueden empezar como módulos internos. No hace falta publicar una librería por cada concepto. El editor y la distribución propia deben usar el mismo contrato probado.

El SDK puede concentrar la configuración en un archivo de la app. El ejemplo ilustra una interfaz futura.

```ts
// src/platform.ts
import { createClient } from '@littlebox/sdk';

export const app = createClient({ baseUrl: '/_littlebox' });
```

Un proxy del mismo origen puede dirigir ese prefijo al kernel alojado o al propio. No basta con cambiar una URL entre dominios y suponer que las cookies funcionarán; la distribución autoalojada incluye esa configuración.

| Paso | Trabajo | Qué sigue funcionando |
| --- | --- | --- |
| Exportar código | Generar un proyecto ejecutable con dependencias fijadas y scripts | La app puede seguir consumiendo Littlebox |
| Autoalojar mini kernel | Importar datos, usuarios, permisos y archivos; cambiar la conexión de runtime | Mismos nombres de acciones y llamadas del frontend |
| Sustituir una acción | Implementar su input, output y permisos en un handler propio | El frontend conserva esa llamada |
| Sustituir auth | Conectar otra validación de sesión y login, manteniendo IDs de usuario | Los datos mantienen su propietario si la vinculación es correcta |
| Sustituir SQLite | Migrar datos y revisar SQL, modelo y transacciones | Se pueden conservar los contratos de acciones |
| Salir completamente | Servir frontend y backend propios | No se requiere una cuenta ni conexión con Littlebox |

La sustitución de handlers es una capacidad del backend externo, no autorización para ejecutar JS arbitrario dentro del kernel compartido. Una primera implementación puede usar un router externo que atienda acciones reemplazadas y derive las demás al mini kernel, manteniendo identidad validada y controles equivalentes. No acepta un actor declarado por el navegador. Si una operación necesita datos de dos backends, no se promete una transacción distribuida.

Cambiar de auth requiere vincular identidades de forma verificada. Puede obligar a iniciar sesión de nuevo o restablecer contraseñas. Cambiar a PostgreSQL puede requerir reescribir SQL. La promesa es localizar las dependencias y conservar la app donde el contrato siga siendo válido, no hacer gratuitas todas las migraciones.

Autoalojar no obliga a sustituir nada. Debe ser la salida más fácil. Un Project Managed puede seguir editándose en Littlebox usando un mini kernel propio compatible. Tras eject de la autoridad del código, el repositorio pasa a ser la fuente y Littlebox puede seguir publicando artefactos. No debe haber dos escritores independientes del mismo código.

## Qué aprovechar del código actual

- Conservar editor, agente sobre copia de trabajo, gate, parser de acciones, runtime bridge, build WASM y SQL Studio.
- En kernel-server/index.ts ya existe el recorrido acción, autorización, transacción SQLite y respuesta sin upload de snapshot por mutación.
- El kernel Rust desplegado por Docker añade una subida completa después del commit en kernel-rs/src/main.rs. Esa subida debe salir del camino de la petición. Hoy un fallo remoto puede devolver error después de haber escrito.
- El lock global de Rust debe dejar de serializar escrituras de apps independientes. La coordinación se hace por base.
- Separar identidad de app y publicación. Cambiar código no debe crear otra base ni otros usuarios.
- Extraer un SDK del miniapp inyectado conservando compatibilidad durante la transición.
- Convertir la exportación JSON actual en un paquete ejecutable más un paquete autorizado de datos.
- Completar usuarios persistentes por app. El login Bun actual usa un código compartido y genera otro actor en cada entrada; no es todavía ese modelo.

No reescribiría el kernel ni cambiaría de lenguaje como consecuencia de esta decisión. El primer cambio debe demostrar el recorrido correcto sobre el kernel que ejecuta Docker. Las pruebas de comportamiento de Bun ayudan a completar los huecos, pero mantener dos kernels con funcionalidades distintas no es el destino final. Consolidar la implementación se decide con el coste concreto de completar ese recorrido, separado del contrato de la app.

## Límites que aceptaría en v1

- Apps personales con pocos usuarios y tráfico acotado.
- Una instancia activa responsable de cada base y volumen persistente obligatorio.
- Frontend estático, acciones declaradas y paquetes de navegador admitidos.
- Límites de tiempo de consulta, filas, input, tamaño de base y archivos.
- Sin ejecución arbitraria de backend, procesos por app, failover automático ni escrituras multi-región.
- Una forma de login completa y permisos sencillos; no un sistema de organizaciones.
- Backups con antigüedad visible; no promesa de pérdida cero ante destrucción del volumen.
- Migraciones pequeñas, previsualizadas y validadas, con recuperación antes de aplicar.

## Secuencia y pruebas de aceptación

| Entrega | Trabajo acotado | Prueba que decide si está terminada |
| --- | --- | --- |
| 1. Publicar e iterar sin perder datos | Base estable por app/entorno, transacción local y backup separado | Crear una fila, publicar v2 y reiniciar kernel conserva esa fila; un fallo del backup remoto no convierte su escritura en error |
| 2. App útil con auth | Usuario persistente, zona pública e interna, acciones por actor | Dos usuarios y dos apps no acceden a datos ajenos; login y revocación funcionan |
| 3. Cambiar modelo y recuperar | Un modelo Drizzle, migración validada, backup y restore | Una migración conserva filas; una fallida deja el estado definido; una copia restaura en un directorio limpio |
| 4. Ejecutar fuera | SDK y paquete de app, mini kernel autoalojable e importación | La app funciona en otra instalación con Littlebox inaccesible y conserva usuarios y permisos |
| 5. Sustituir una capacidad | Implementación externa de una acción con el mismo contrato | Reemplazar notes.create mantiene el frontend y las pruebas de acceso; el resto sigue usando el kernel |

La demostración completa empieza con la petición al agente de crear un cuaderno con portada pública, login, notas privadas y una lista compartida. Continúa con una edición del modelo, una segunda publicación y la exportación. Termina con ese mismo cuaderno funcionando en el mini kernel propio. La publicación y el eject se diseñan para ese recorrido antes de añadir más proveedores de infraestructura.

## Delegación y decisiones pendientes

El agente principal fija contratos y revisa integración. Agentes de menor coste pueden extraer el SDK, crear el exportador y añadir pruebas de comportamiento en tareas separadas. Los cambios de identidad, migración y persistencia requieren revisión conjunta de sus invariantes. No se delega una reescritura abierta del backend.

Los ADR deben recoger base estable por app y entorno, commit local separado de backup remoto, auth por app, autoalojamiento del mismo runtime y sustitución externa de capacidades. Este documento propone esas decisiones. No cambia las garantías del código existente por describirlas.
