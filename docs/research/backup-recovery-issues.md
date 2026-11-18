# Plan de entrega de backups

[PRD y matriz de recuperación](./backup-recovery-prd.md). Las claves B01 a B12 identifican las issues y sus dependencias. Cada responsable es un rol propuesto. P0 indica trabajo que debe preceder al cambio de formato. Las pruebas de destrucción, rotación, poda y pérdida de volumen se ejecutan sobre destinos desechables. Para evidencia de instalaciones reales se publican resultados agregados y se custodian los detalles operativos fuera del tracker.

| Hito | Entrega | Decisión que habilita |
| --- | --- | --- |
| A. Proteger lo que existe | B01, B02, B03 y B04 | El operador tiene un camino de recuperación ensayado y sabe reconocer incidencias en las copias actuales. |
| B. Capturar un corte completo | B05, B06 y B07 | El crate se puede integrar por separado y ambos binarios capturan un estado con un inventario verificable. |
| C. Recuperar el formato incremental | B08, B09 y B10 | El candidato supera fallos, concurrencia y retención antes de escribir junto al motor actual. |
| D. Cambiar el escritor | B11 y B12 | Se autoriza el cambio con evidencia de restauración, costes y vuelta atrás. |

### B01. Conservar los backups internos y detectar registros sin contenido [#35](https://github.com/danibram/konae/issues/35)

P0 · Kernel e importación · Arranque inmediato.

El import guarda contenidos en `.konae/backups/<id>/`. La copia de instalación recoge los registros JSON y los backups planos. Se ha observado en una copia de directorio un registro cuyo directorio de contenidos falta. Primer paso operativo: inventariar solo los identificadores necesarios bajo acceso restringido y preservar los contenidos que sigan en el origen en un destino independiente. Generar el informe de puntos afectados en modo lectura. Una copia nueva se distingue de los puntos históricos que continúan incompletos.

- [ ] Un fixture crea backups internos de datos, de fuentes y uno plano antiguo. Tras recuperar toda la instalación desde directorio y desde S3, el Kernel restaura cada punto mediante la interfaz normal y se comparan datos, archivos y revisión de fuentes.
- [ ] Captura falla si un registro que promete contenido carece de él. En un punto antiguo, el verificador informa del ID y del alcance afectado, distingue el estado actual de la app de su historial y documenta las opciones del operador.
- [ ] El informe distingue contenido recuperado, ausente y pendiente de comprobar. Un intento de reparación escribe un punto nuevo y mantiene identificable el original. Adjuntar resultados agregados de una restauración anidada en aislamiento.

### B02. Clasificar todos los almacenes y todos sus escritores [#36](https://github.com/danibram/konae/issues/36)

P0 · Kernel, identidad y operaciones · Arranque inmediato.

- [ ] Entregar una matriz versionada por binario, versión, ámbito, componente, escritor, presencia obligatoria u opcional, regla de captura, tratamiento en restore y prueba. Incluir operaciones MCP/editor, acciones, archivos, cuentas, publicación/import, schedules, correo, calls, Usage, administración y mantenimiento offline.
- [ ] Comparar el catálogo con dos instalaciones reales en solo lectura. Clasificar diferencias como `google.sqlite`, payloads internos, material de claves, fuentes antiguas y borradores sin `dist/`. Abrir un recurso opcional ausente no lo crea. Un componente persistente desconocido bloquea la certificación y deja diagnóstico.
- [ ] Añadir una prueba que falle cuando un nuevo almacén o módulo declarado carezca de política. Mantener perfiles `konae` y `kernel` y verificar que el operador puede distinguir captura de instalación, export/import y backup interno.

### B03. Ensayar la pérdida del volumen con una copia independiente [#37](https://github.com/danibram/konae/issues/37)

P0 · Operaciones · Arranque inmediato, coordinar la protección de B01.

- [ ] Separar credenciales de escritura y borrado y usar un destino que sobreviva a la pérdida del volumen y de la cuenta de operación habitual. Documentar custodia, rotación y acceso de emergencia a claves de descifrado y al registro de revocaciones. Verificar con permisos efectivos que la credencial cotidiana no puede borrar esa copia.
- [ ] Levantar un host limpio, restaurar una copia real con credenciales de lectura y sin acceso a archivos vivos. Probar también la pérdida del destino principal y la recuperación desde la copia independiente con el registro posterior de revocaciones.
- [ ] El entorno de ensayo bloquea correo, llamadas externas, schedules con efectos y tráfico de usuarios antes de arrancar. Usa almacenamiento cifrado temporal y acceso restringido. Registrar versión, punto, duración y resultados agregados, y confirmar la retirada del volumen temporal. Ensayar también el caso de clave no disponible.

### B04. Detectar una copia incompleta, demasiado antigua o imposible de leer [#38](https://github.com/danibram/konae/issues/38)

P0 · Kernel y operaciones · Arranque inmediato.

- [ ] El escritor contrasta inventario e índice con los bytes. Un blob requerido ausente, truncado o sustituido hace fallar ese destino con diagnóstico. Un borrado legítimo ya anterior al corte queda fuera. El éxito anterior permanece identificable.
- [ ] Probar manifiestos justo por debajo, en el límite y por encima del máximo que acepta el lector, incluido el tamaño final firmado. El escritor rechaza un punto que el lector no podría abrir.
- [ ] Informar por destino de fecha de corte, último intento, último punto completo y último restore verificado. Probar worker muerto, reloj que avanza sin nuevo intento, fallo de un destino, reinicio y recuperación posterior. Medir RPO usando la fecha del corte.

### B05. Extraer el crate con compatibilidad v1/v2 [#39](https://github.com/danibram/konae/issues/39)

Kernel y motor de backup · Depende de B01 y B02.

- [ ] Una segunda aplicación Rust captura una SQLite y assets, verifica y restaura sin `AppState`, HTTP ni configuración de Konae. El adaptador del Kernel decide el alcance y las políticas de identidad.
- [ ] Mantener fixtures mínimos de directorio v1, S3 v1 y v2 cifrados, directorio con hardlinks y referencia directa a un objeto cuyo manifiesto de origen ha caducado. Los lectores verifican y restauran desde un host sin caché.
- [ ] En el mismo formato, comparar el comportamiento anterior y posterior de ambos binarios, CLI, salidas operativas y errores observables. Probar clave errónea, firma ausente y copia explícita. La extracción se entrega por separado del nuevo escritor.

### B06. Fijar un corte común para instalación, apps, archivos y publicaciones [#40](https://github.com/danibram/konae/issues/40)

Kernel, almacenamiento y motor · Depende de B02 y B05.

- [ ] Introducir el permiso compartido para operaciones lógicas y la pausa exclusiva de captura. Inventariar procesos externos y usar la exclusión de volumen correspondiente. Probar creación y borrado de proyecto, alta/suspensión, mutación que encola trabajo, import, publicación y sustitución de SQLite.
- [ ] Abrir en modo solo lectura, fijar cada SQLite WAL con `BEGIN` y una lectura real y copiar desde la misma conexión. Fijar versiones de blobs, índices, fuentes y artefactos. Un borrado o un import posterior con el mismo ID conserva los bytes anteriores hasta acabar.
- [ ] Probar el caso sin WAL o sin versiones de objetos, operaciones a medio confirmar y cancelación durante drenaje o staging. Liberar permisos y lectores al agotar tiempo, espacio o descriptores. Probar que la app sigue escribiendo durante una subida remota lenta.

### B07. Recuperar las reglas del Kernel y activar la instalación completa [#41](https://github.com/danibram/konae/issues/41)

Kernel, identidad y operaciones · Depende de B02 y B05.

- [ ] Validar integridad SQLite, vínculos catálogo/app, índice/bytes y relaciones de pertenencia. Verificar borrador frente a publicación, direcciones, módulos opcionales y mantenimiento previo. Comparar los datos del corte antes de aplicar las transformaciones de restore.
- [ ] En el Kernel recuperado, iniciar sesiones nuevas, probar una acción y abrir un asset con usuario autorizado y con usuario sin permiso. Probar API key vigente y revocada, cuenta suspendida tras la copia, sesión vieja y enlaces caducados. Registro de revocaciones inaccesible o inválido lleva a mantenimiento. Ensayar reconciliación.
- [ ] Comprobar por estado correo pendiente, transfers, OAuth, planes, outbox y schedules. Un envío de resultado incierto queda retenido o usa idempotencia verificada. Cortar el proceso durante activación, reiniciar, y comprobar que sirve un árbol completo o sigue cerrado. Una carpeta destino ocupada conserva sus datos.

### B08. Guardar SQLite por bloques en un formato versionado [#42](https://github.com/danibram/konae/issues/42)

Motor y almacenamiento · Depende de B05 y B06.

- [ ] Definir vectores de formato, orden, longitud, tamaño de página, identidad de repositorio, autenticación del manifiesto y de cada bloque. Revisar cifrado, nonces, separación de claves, rotación y relectura de copias antiguas. Aislar la deduplicación entre repositorios y preservar el tratamiento de datos borrados en bases sensibles.
- [ ] Medir grupos de páginas y agrupación de objetos con cero cambios, cambio de una fila, crecimiento, truncado, migración y escrituras dispersas. Registrar bytes locales leídos/escritos, peticiones por tipo, subida y descarga reales, memoria máxima y latencia de restore.
- [ ] Restaurar cada punto desde su manifiesto y objetos en un host vacío. Interrumpir una subida nunca publica un punto parcial. Reintentar sin sobrescribir objetos válidos. Un objeto alterado, ajeno o ausente impide activar la instalación.

### B09. Podar sin perder objetos de puntos retenidos ni en uso [#43](https://github.com/danibram/konae/issues/43)

Motor y almacenamiento · Depende de B08.

- [ ] Marcar referencias directas desde todos los manifiestos retenidos y proteger publicaciones, lectores y restauraciones en curso. Simular dos escritores, poda concurrente, crash tras marcar y pérdida de caché. Una raíz ilegible o un listado incompleto impiden el borrado.
- [ ] Con reloj simulado conservar 24 recientes y un punto diario durante 30 días. Tras caducar el primer manifiesto, restaurar el punto más antiguo retenido y el último. El siguiente ciclo estable reutiliza objetos y permite recuperar la copia nueva.
- [ ] Ensayar límites del destino, paginación y pérdida parcial. Los contadores proceden del backend. Verificar que quedan todos los objetos alcanzables, que los huérfanos caducados se recogen y que ningún limpiador v1/v2 alcanza el espacio v3.

### B10. Someter el sistema a caídas y restauraciones de larga duración [#44](https://github.com/danibram/konae/issues/44)

Verificación y Kernel · Depende de B06, B07 y B09.

- [ ] Generar un año simulado de instalaciones con escrituras, altas, borrados, importaciones, assets reutilizados, revocaciones, rotación de claves y poda. Comparar restauraciones con un modelo independiente del manifiesto del escritor, incluyendo permisos y versiones de archivos.
- [ ] Inyectar disco lleno, escritura corta, fallo de sincronización, timeout, listado parcial, respuestas S3 fallidas y terminación entre pasos duraderos. Reproducir con semilla e identificar el primer estado divergente. Cero activaciones parciales o puntos anunciados como completos que hayan perdido contenido.
- [ ] Ejecutar contra directorio y S3 compatible, con los dos binarios. Incluir un backend real desechable para confirmar el comportamiento del proveedor. Añadir a CI un conjunto corto determinista y programar la campaña extensa con conservación de evidencias agregadas.

### B11. Comparar ambos escritores en instalaciones representativas [#45](https://github.com/danibram/konae/issues/45)

Operaciones y verificación · Depende de B03, B04 y B10.

- [ ] Medir el motor vigente y el candidato con una captura compartida en repositorios aislados. Incluir perfil de producción con archivos privados, perfil de borradores y módulos opcionales, y Kernel sin propietarios. Incluir datos reales solo en el perímetro autorizado.
- [ ] Restaurar ambos resultados en destinos vacíos. Comparar el inventario externo al escritor, filas y hashes antes de invalidaciones, y comportamiento de acceso después. Registrar RPO, RTO, latencia p99 de escritura, WAL máximo, espacio temporal, peticiones y coste por destino.
- [ ] Publicar un informe de cumplimiento de los objetivos del PRD y motivos de cada diferencia. Si el impacto supera el presupuesto o la prueba funcional diverge, mantener el escritor vigente y abrir incidencia con un caso mínimo.

### B12. Cambiar el escritor y ensayar la vuelta atrás [#46](https://github.com/danibram/konae/issues/46)

Responsable de release y operador de guardia · Depende de B11.

- [ ] Desplegar el lector antes del escritor. Ensayar rollback con la versión exacta que volvería a servir y permisos que impidan a sus limpiadores tocar v3. Documentar selección de la última copia compatible, tiempo y pérdida de datos potencial de ese camino.
- [ ] Ejecutar simulacros diarios recientes y semanales antiguos, alternando destinos. Antes de retirar el escritor anterior, completar una ventana real de 30 días y restaurar desde los extremos retenidos tras una poda. Probar también la copia independiente sin las credenciales habituales.
- [ ] Operaciones y mantenimiento firman con IDs de copia de acceso restringido, versión, resultados y tiempos. Detener la promoción ante un fallo de restore, un componente sin clasificar, un exceso de RPO/RTO o una alerta de integridad. Mantener el lector histórico y sus claves durante toda la retención aplicable.
