# Monitor de uso por feature y app

Plan del 7 de noviembre de 2026. La captura, las lecturas administrativas y las vistas por app y cuenta están implementadas. La segunda ronda de revisión visual dio el veredicto "ready" sin P0 ni P1 abiertos.

El propietario debe poder ver qué funciones tiene cada app, cuáles se usan y cuáles fallan. La medición pertenece al Kernel y el historial debe acompañar a la app cuando cambia de máquina.

## Alcance

La primera entrega trata una feature como una capacidad del Kernel que la app usa. Incluye acciones de datos, autenticación, archivos, rutas HTTP, tareas programadas, llamadas a servicios y avisos por email.

Las funciones propias de una app, como reservar una cita o completar una tarea, se muestran mediante sus acciones y Signals declarados. No se deduce una reserva completada contando consultas o clics. El MVP no añade un catálogo obligatorio de features ni un SDK de tracking en el navegador.

## Punto de partida

- `kernel-rs/src/usage.rs` guarda contadores diarios y Signals en `deployments/<id>/usage.sqlite`.
- `usage_read` ofrece totales por operación y nombre, con hasta 400 días por consulta. `signals_read` pagina eventos, con un máximo almacenado de 50.000 Signals.
- El Kernel cuenta acciones ejecutadas, rutas, sesiones, altas, accesos, archivos, rechazos de guards, avisos y llamadas encoladas. También guarda resultados de tareas programadas. Los fallos generales de acciones todavía no forman parte de esos contadores.
- El editor no tiene una vista de Usage. Su preview no registra uso; el Kernel de desarrollo sí.
- Los paquetes con estado incluyen `usage.sqlite`. El export de código por sí solo no contiene ese historial.
- La cola de Usage puede perder entradas cuando se llena o falla la escritura. Las cifras actuales son métricas operativas aproximadas, no registros de facturación.

La propuesta amplía [ADR-0010](../adr/0010-kernel-owns-usage-and-signals.md).

## Qué verá el propietario

Una página "Usage" accesible desde el proyecto, con detalle por Deployment y periodos de 7, 30 y 90 días. La misma página ofrece un resumen de los proyectos de la cuenta, incluidos los externos y los borradores. La tabla muestra la feature, su configuración actual, actividad durante el periodo, resultado y última actividad conocida.

| Feature | Actividad principal | Resultado que se muestra |
| --- | --- | --- |
| Acciones de datos | Ejecuciones por acción | Correctas, rechazadas por guard y fallidas |
| Autenticación | Altas y accesos correctos | Totales separados; una consulta de sesión no equivale a una visita única |
| Archivos | Subidas y lecturas | Totales separados |
| Rutas HTTP | Ejecuciones por ruta y acción | Correctas, rechazadas y fallidas |
| Tareas programadas | Ejecuciones por acción | Con cambios, sin cambios y fallidas |
| Llamadas a servicios | Llamadas encoladas | Entregadas, fallidas y pendientes |
| Avisos por email | Avisos generados por acciones | Encolados, entregados y fallidos según el registro de correo |
| Funciones de negocio | Signals declarados, como `booking.created` | Ocurrencias por nombre y día |

La vista distingue "configurada", "con actividad en el periodo" y "sin datos". Autenticación y archivos están disponibles por defecto, lo que no demuestra que la app los use. Una acción eliminada sigue apareciendo si tiene actividad histórica.

Los contadores de llamadas y correo distinguen la fecha de creación de la operación y su estado actual. Una llamada encolada no cuenta como entregada. Los reintentos no crean otra operación lógica.

## Plan de implementación

1. Definir el contrato de métricas y su correspondencia con cada punto de ejecución. Fijar qué operación cuenta cada fila, qué fecha usa y cómo evita duplicados. Registrar la extensión de ADR-0010 antes de cambiar el formato. Las rutas deben conservar el nombre de su acción para permitir agrupaciones sin contar dos veces una ejecución.

2. Completar la captura en el Kernel. Mantener los contadores actuales y añadir agregados diarios por acción, origen y resultado donde falten. Los orígenes incluyen runtime, ruta HTTP, tarea programada y continuación de una llamada. Guardar también la última actividad. Contar fallos después de identificar una acción y admitir al llamante; los intentos de acceso rechazados por la plataforma quedan fuera del uso de producto del MVP.

3. Guardar totales diarios de Signals al registrarlos. El límite de eventos detallados no debe truncar los totales del periodo. El histórico anterior que solo conserva parte de sus Signals se presenta como parcial. Los resultados de llamadas y avisos necesitan agregados propios o una lectura de sus registros existentes que exponga su ventana de retención. No se calculan totales completos recorriendo únicamente la página más reciente.

4. Añadir una lectura administrativa por Deployment y periodo. Reutilizar `usage_read` cuando baste y añadir una respuesta de resumen para las agrupaciones nuevas. Incluir fechas de cobertura, granularidad, última actualización y estado de captura. Mantener el permiso `read` y el grant del proyecto. Ninguna lectura administrativa se ofrece al runtime.

5. Construir la pestaña "Uso". Empezar con la tabla, una serie diaria y el detalle por acción o Signal. Mostrar configuración desde el artefacto publicado, no desde archivos aún sin publicar. Ofrecer actualización manual y refresco cada 60 segundos mientras la pestaña esté visible. Distinguir un cero registrado de una fecha sin cobertura y de un error de lectura.

6. Verificar el traslado y documentar la medición. Migrar `usage.sqlite` sin borrar datos anteriores y conservar lectura de paquetes antiguos. Probar exportación con estado, importación y continuidad del historial. Actualizar la guía del agente para usar Signals existentes cuando una app necesite medir una tarea de negocio completada.

## Límites del MVP

- Los agregados diarios conservan la ventana actual de hasta 400 días, con límites de filas revisados para las nuevas dimensiones. El monitor indica si la retención recortó el periodo solicitado.
- La captura sigue fuera de la respuesta de la app. Exponer pérdidas de cola y errores de escritura con un alcance explícito. Si el Kernel solo conoce una pérdida global, el panel no la atribuye a una app concreta. No prometer recuentos exactos.
- No guardar cuerpos de peticiones, SQL, emails ni nuevas propiedades personales para estos contadores. El resumen de Signals agrupa por nombre, sin devolver sus propiedades.
- No calcular usuarios únicos a partir de sesiones. Tampoco sumar consultas, llamadas y Signals como si fueran usos independientes de una función de negocio.
- El historial anterior mantiene sus categorías actuales. Los nuevos resultados y marcas de última actividad empiezan al desplegar la instrumentación; no se reconstruyen cifras que nunca se registraron.
- Quedan fuera las alertas, embudos, métricas por usuario y latencias p95. El resumen conjunto de las apps de la cuenta forma parte de la entrega por petición del propietario.

## Criterios de aceptación

- Una app con datos, archivos y una tarea programada muestra qué capacidades registraron actividad en el periodo.
- La misma acción llamada por SDK o HTTP cuenta una vez por ejecución. Una continuación o ejecución programada conserva su origen.
- Un guard rechazado y un fallo de ejecución aparecen separados de una ejecución correcta. Una mutación sin cambios no genera un Signal de negocio.
- Una llamada encolada no aparece como entregada hasta que el Kernel registra ese resultado. Los reintentos no duplican el total lógico.
- El resumen de Signals sigue siendo correcto aunque el detalle haya superado 50.000 eventos desde que existen los agregados.
- Otro propietario y un usuario de la app no pueden consultar las métricas del proyecto.
- Una app importada conserva las métricas y continúa sumando en la máquina de destino.
- Un periodo anterior a la captura, una cola con pérdidas y una lectura fallida muestran su limitación sin presentarse como actividad cero.

La validación requiere pruebas de integración del Kernel para conteos, permisos y traslado, pruebas de contrato de los nuevos agregados y comprobación de la vista con apps de ejemplo. No hace falta instrumentar las apps a mano para medir las capacidades del Kernel.


## Registro de implementación

- `/usage` consulta hasta 50 proyectos accesibles por página. El resumen distingue operaciones, resultados de ejecución, capacidades con actividad y último día registrado.
- `/usage?deployment=<id>` muestra la configuración publicada, resultados por acción y origen, totales diarios de Signals y estados de las llamadas y avisos conservados.
- El Kernel conserva las métricas en el componente Usage de los paquetes existentes. La prueba de importación comprueba que el historial continúa sumando en destino.
- Los resultados de llamadas y correo proceden de sus registros retenidos. No se presentan como totales históricos completos.
- La cobertura expone el inicio de la captura, el recorte por retención, el estado del vaciado de la cola y las pérdidas globales desde el último reinicio.

## Rondas de crítica

| Ronda | Inteligencia | UX | Diseño | Paridad oscura | Veredicto |
| --- | --- | --- | --- | --- | --- |
| 1 | 6 | 4 | 6 | 2 | not yet |
| 2 | 8 | 8 | 7 | 9 | ready |

La primera ronda encontró texto sin contraste en oscuro, tablas comprimidas en móvil, resultados ausentes del resumen, un gráfico de volumen coloreado como éxito, subtotales sin etiqueta y navegación incorrecta para proyectos externos. La segunda versión corrige esos hallazgos. El enlace público del entorno de revisión también apuntaba al puerto del editor; la configuración y los registros locales se corrigieron conservando los datos.


La ronda 2 verificó escritorio y móvil, claro y oscuro, apps con actividad y una app sin historial, los controles de periodo y actualización, y el enlace al artefacto publicado. No quedaron hallazgos P0 o P1. La revisión se ejecutó con un subagente de solo lectura dentro de la sesión propietaria porque BB impidió compartir el navegador con un segundo hilo.

La validación pasó con 257 pruebas del Kernel, 64 pruebas de integración de MCP, rutas, paquetes y Usage, y la compilación de producción. Las pruebas de Usage cubren también la agrupación del resumen de cuenta y las fechas UTC.
