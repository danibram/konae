# Recuperación de una instalación de Konae

PRD propuesto el 18 de noviembre de 2026. Destinatarios: mantenimiento del Kernel, operadores de instalaciones y responsables de las pruebas de recuperación. [Plan e issues](./backup-recovery-issues.md).

## Resultado que debe poder demostrar un operador

Tras perder la máquina original, el operador elige una copia retenida, recupera en otro volumen la instalación completa y demuestra que el propietario puede abrir el proyecto, que la app aplica sus reglas y que sus usuarios pueden leer exactamente los archivos que les corresponden. Puede recuperar también un backup interno que la app ya ofrecía en la fecha elegida. La máquina de recuperación necesita el repositorio de copias y las claves custodiadas, sin acceso al almacén de archivos vivo.

El producto conserva tres contratos distintos: la copia de instalación, el backup interno previo a una importación o restauración y la exportación portátil de una app. Cada uno tiene su propio alcance y sus reglas de identidad. El crate propuesto comparte captura, almacenamiento y verificación. El Kernel decide qué información entra en cada operación y quién puede restaurarla. El despliegue del motor es trabajo de operador.

## De dónde salen los requisitos

Inspección de solo lectura del 18 de noviembre. Se contaron archivos, elementos de manifiesto y destinos. No se consultaron filas, contenidos de archivos de usuarios ni secretos. La inspección de esta sesión no fue un simulacro de recuperación. El simulacro S3 del hilo anterior es evidencia histórica.

| Instalación observada | Hecho observado | Consecuencia para el diseño |
| --- | --- | --- |
| Konae en producción, con directorio y S3 | 6 apps, con las cuatro SQLite de app en cada una. La copia de directorio observada tiene 637 entradas, 255 blobs y 291.988.147 bytes lógicos. El servicio informa de un destino S3 cifrado y una cadencia de 3.600 s. | Ensayar ambos destinos por separado, incluidas descargas con permisos, y medir solicitudes y bytes reales. Medir el tiempo hasta poder usar la app restaurada. |
| Mismo servidor | Datos y directorio de copias tienen el mismo identificador de sistema de archivos. Hay seis SQLite en la raíz viva y cinco en el manifiesto. La sexta, `google.sqlite`, guarda flujos de acceso en curso. | Probar la pérdida de todo el volumen. Declarar el tratamiento de cada componente, incluidos los que deben caducar. Una diferencia de inventario exige clasificación. |
| Mismo servidor | Para tres registros de backup interno presentes en la copia de directorio, la carpeta correspondiente existe en el origen y falta en la copia. Las tres carpetas del origen suman doce archivos. El escritor actual selecciona archivos planos bajo `.konae/backups/`, mientras que el import guarda contenido en subdirectorios. | Prioridad P0. El operador debe conocer qué puntos históricos están afectados y proteger los contenidos existentes antes de que desaparezcan del origen. La copia posterior al arreglo debe permitir una segunda restauración, la del backup interno. |
| Instalación local persistente | 4 carpetas de app con fuentes, 3 con artefacto publicado. Las cuatro tienen secretos, tres tienen base de datos, identidad y uso. Ninguna tenía índice de archivos en el muestreo. | Probar proyectos en borrador, componentes ausentes y recuperación del editor. Abrir para inspección una ruta ausente no debe crear una SQLite vacía. |

La referencia de código es `af1a897`, que incorpora v0.10.0. El binario `kernel` sin propietarios se verificará como perfil adicional en laboratorio. [Inventario de instalación](../../kernel-rs/kernel/src/installation.rs) · [Copias](../../kernel-rs/kernel/src/copies.rs) · [Backup anterior al import](../../kernel-rs/kernel/src/import.rs) · [Contrato de operación](../operate.md).

## Contrato de recuperación

| Componente | Comportamiento que debe comprobar el simulacro |
| --- | --- |
| Catálogo y propietarios | Recuperar proyectos, propietarios, auditoría y relaciones con las carpetas correspondientes. Cerrar sesiones de propietario y retirar credenciales MCP, códigos OAuth y planes abiertos según el contrato de restore. |
| Datos y acceso por app | Recuperar filas, esquema, cuentas, pertenencia, roles, claves de API vigentes según el contrato, Signals y Usage. Comprobar acceso permitido y denegado a la misma fila o archivo. Mantener las suspensiones y revocaciones posteriores registradas. |
| Fuentes, publicación y direcciones | Mantener el borrador y la revisión publicada aunque sean distintos. Verificar acciones, esquema efectivo, artefacto, políticas, módulos, hostname, dominio, ruta y mantenimiento. Un borrador sin Deployment sigue siendo editable. |
| Assets | El índice capturado determina los archivos, sus propietarios y visibilidad. Cada entrada debe resolver a los bytes exactos aun si después se borra o se reutiliza el identificador. Restaurar bytes locales desde una copia hecha con file store S3. |
| Historial interno | Restaurar registros planos antiguos y backups compuestos de datos o fuentes con sus payloads. Comprobar un restore interno tras el restore de instalación. Identificar expresamente el historial incompleto en copias antiguas. |
| Correo, llamadas, schedules y transfers | Conservar historial y aplicar las reglas de invalidación de transferencias, enlaces y correo pendiente. Especificar por estado qué hacer con llamadas o ejecuciones en curso. Si el destino externo pudo recibir una operación, exigir una clave de idempotencia o retenerla para conciliación. El simulacro captura todo efecto saliente. |
| Google, secretos y material externo | Conservar la configuración de Google de la app y sus secretos según el contrato de instalación. Descartar flujos OAuth abiertos. Clasificar claves de correo generadas localmente y credenciales suministradas por el operador. Ensayar su recuperación desde custodia externa. |
| Estado de operación | Derivar de nuevo cachés, locks y staging. Conservar el mantenimiento previo. Si no se puede autenticar o leer el registro de revocaciones, arrancar en mantenimiento y probar la reconciliación antes de reabrir. |

## Protocolo propuesto

1. El Kernel registra los escritores de estado duradero, incluidos workers, lifecycle de proyectos, restauración interna y operaciones de mantenimiento. Cada operación conserva un permiso compartido hasta que sus cambios relacionados alcanzan un estado recuperable. Una operación que confirma en dos almacenes necesita compensación o un registro de recuperación ante caída entre ambos commits. El arranque resuelve esos registros antes de admitir una captura. Los procesos con acceso al volumen deben respetar la misma exclusión.
2. El coordinador cierra temporalmente la entrada de mutaciones, drena las admitidas y enumera las bases y apps existentes. Abre cada SQLite en modo de solo lectura sin crearla, verifica el modo de journal, inicia una transacción y ejecuta una lectura real. Fija también las versiones de fuentes, artefactos, índices y blobs. Los reemplazos de base conservan la generación completa y sus ficheros auxiliares hasta que terminen los lectores. En modos de journal o almacenes sin una vista estable, usa staging bajo exclusión dentro del presupuesto o cancela la captura.
3. El coordinador reabre la entrada. El crate materializa las imágenes desde esas mismas conexiones en staging local y libera las vistas. Divide SQLite en grupos de páginas y reutiliza objetos de contenido ya verificados en ese repositorio. Se mantiene la captura compactada para identidad y secretos hasta demostrar cómo se trata el contenido borrado de páginas libres. El directorio puede seguir ofreciendo una carpeta completa. S3 almacena objetos inmutables y manifiestos con referencias directas. Cada destino confirma su propio resultado.
4. El motor publica el manifiesto autenticado al final en el destino cifrado. Antes de publicarlo valida con las reglas del lector el formato, los límites y la cobertura del inventario. La poda comparte exclusión o reservas con capturas y restauraciones. Si no puede leer o autenticar una raíz retenida, suspende los borrados. La pérdida de caché local solo debe afectar al coste.
5. Restore reconstruye en un volumen temporal, verifica tamaños, hashes, relaciones e integridad SQLite, aplica las políticas de identidad y activa el resultado con recuperación ante caídas. Un reinicio solo sirve una instalación completa. La exportación por app conserva su formato y las exclusiones de secretos y sesiones. [Transacciones SQLite](https://sqlite.org/lang_transaction.html) · [Replicación WAL por base en Litestream](https://litestream.io/how-it-works/).

## Objetivos operativos propuestos

| Medida | Criterio para el piloto |
| --- | --- |
| Pérdida máxima de datos (RPO) | Mantener la cadencia horaria observada. Medir la edad del **corte** de la última copia remota completa, incluido el retraso de publicación. Objetivo inicial de 65 min en el perfil horario, con alerta al superarlo aun si el worker muere. Registrar por separado las revocaciones confirmadas. |
| Tiempo de recuperación (RTO) | Objetivo inicial de 30 min para el perfil observado de unos 292 MB, desde máquina vacía con acceso a las claves hasta login nuevo, acción y lectura privada. Medir descarga, reconstrucción, reconciliación y arranque. |
| Impacto en la app | Ensayar primero con p99 de espera adicional de escritura de hasta 250 ms y límite de 2 s para la barrera, drenaje incluido. Cancelar una captura excedida, liberar recursos y medir la antigüedad resultante. Medir descriptores, bytes de WAL y disco temporal con presupuestos explícitos por instalación. |
| Recuperabilidad | Cada día, restaurar en aislamiento un punto reciente. Cada semana, uno de los más antiguos retenidos alternando destinos. Tras una poda real, restaurar ambos extremos. Registrar edad de la última restauración que pasó y emitir alerta ante fallo o simulacro vencido. |

Estos números son objetivos para evaluar el candidato. El operador fijará los presupuestos finales con mediciones del motor vigente y del candidato sobre el mismo hardware, almacenamiento, número de apps y carga. Documentará cualquier cambio antes del piloto.

## Publicación y criterio de salida

El primer envío corrige la pérdida de historial interno sobre el formato vigente. Se protegen los contenidos recuperables con otra credencial y otro dominio de fallo, incluido el registro de revocaciones. Se mantienen lectores de v1 y v2 y copias conocidas para volver atrás. El formato nuevo usa un prefijo y un directorio propios, fuera del alcance de los limpiadores antiguos.

La escritura en paralelo consume la misma captura estable para ambos formatos. Antes de elegir el candidato como principal se exige restauración desde cada destino en una máquina aislada, comparación con el inventario independiente del corte, restauración de un punto anterior y otro posterior a la poda, y ensayo de rollback. La política actual conserva los 24 puntos más recientes y el último de cada día durante 30 días. Antes de retirar el escritor anterior debe haber transcurrido una ventana real de retención, con una recuperación del extremo antiguo. El lector histórico y las claves se conservan mientras existan copias que los necesiten. Los operadores de backup, identidad y despliegue adjuntan evidencia al hito correspondiente. La decisión de despliegue es independiente de este PRD.
