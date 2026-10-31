# Definir acciones y revisar su acceso

Fecha de revisión 2026-11-08. Decisión de producto propuesta tras revisar el código, recibir una crítica independiente y contrastar casos de uso. No describe una funcionalidad implementada.

## Decisión

La persona define qué quiere que haga su app y quién debe poder hacerlo. El chat recoge esa intención y una UI pequeña mantiene visible el comportamiento comprobado. MCP permite que el editor y los agentes externos operen sobre las mismas reglas. Mantener la edición de archivos para quien escribe código. La persona no necesita elegir entre UI, chat y MCP.

No crear un editor visual de SQL y bindings. No hacer que una persona tenga que conversar para consultar quién puede leer sus datos. No convertir una descripción en una autorización.

## Qué existe

| Elemento | Qué hace hoy | Referencia |
| --- | --- | --- |
| `actions.json` | Declara operaciones, SQL, bindings, acceso y rutas que comprueban el editor y el Kernel | `src/kernel/actions.ts`, `kernel-rs/src/action_shape.rs` |
| Action Contract | Se genera desde acciones y esquema, describe entradas, resultados, llamadas, errores y límites y acompaña la exportación | `src/lib/action-contract.ts`, `src/components/useActionContract.ts` |
| Rules | Cambia el acceso declarado, muestra reglas de rutas y permite elegir un usuario de prueba | `src/components/CapabilityWorkspace.tsx` |
| Integrations | Administra secretos y claves, pero remite a editar JSON para añadir una ruta | `src/components/IntegrationsView.tsx` |
| MCP de administración | Lee y guarda fuentes y prepara y aplica publicaciones | `kernel-rs/src/mcp.rs` |
| MCP de ejecución | Ofrece `actions_invoke` con un nombre y un objeto de entrada genéricos | `kernel-rs/src/mcp.rs` |

El contrato actual es más que documentación. Las pruebas lo reconstruyen como un programa equivalente y lo usan para comprobar respuestas de otro backend. El Kernel ejecuta las declaraciones de la app; no interpreta el Markdown ni una frase libre para decidir permisos.

Una `description` por acción todavía no forma parte de `ServerActionDefinition`. Añadir ese campo sin conectar sus lectores no ayuda al agente ni publica una herramienta MCP nueva. Hoy `tools/list` no genera una herramienta con el esquema específico de cada acción.

## Primer pase de crítica

La hipótesis inicial era crear acciones por chat y ofrecer un interruptor "Pública". Falla en cuatro decisiones distintas.

- La app puede exigir sesión aunque la acción declare `anonymous`.
- Una acción puede aceptar HTTP con una firma o clave sin aceptar visitantes ni llamadas por `actions_invoke`.
- Exigir sesión no limita las filas a su propietario. Esa restricción vive en el SQL y los bindings.
- Describir una acción o incluirla en un catálogo no concede permiso para ejecutarla.

La UI actual agrava la ambigüedad. El selector ofrece "Anyone" incluso con la app privada. Las etiquetas convierten nombres como `contacts.list` y `tasks.list` en "List the rows". El contrato está en Files, las reglas en Auth y la conexión de programas en Integrations.

La crítica independiente coincide en conservar una UI de revisión y rechaza duplicar todos los campos de `actions.json` en formularios.

## Comparación de alternativas

Esta tabla expresa juicio de diseño, no resultados de un estudio con usuarios.

| Alternativa | Persona que no escribe código | Revisión de acceso | Agente externo | Coste y decisión |
| --- | --- | --- | --- | --- |
| Solo MCP | Depende de tener y entender un cliente | Depende de la explicación del agente | Puede operar por protocolo | No cumple el primer trabajo por sí solo |
| Solo chat | Describe intención con facilidad | El estado queda repartido entre mensajes y puede quedar obsoleto | Necesitaría igualmente una API | Útil para crear, insuficiente para inspeccionar |
| UI completa de contratos | Obliga a aprender SQL, bindings y casos de rutas | Puede mostrar todo | Sigue necesitando MCP | Demasiado mantenimiento y vocabulario técnico |
| Chat y UI de revisión sobre MCP | Describe intención y verifica un resultado persistente | Consulta el acceso efectivo y los cambios pendientes | Usa las mismas operaciones y comprobaciones | Elegida |

MCP es un protocolo que puede usarse desde distintas interfaces. Su especificación distingue `description`, `inputSchema` y `outputSchema`; no exige una interfaz de usuario concreta. Ver [MCP Tools, revisión 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).

## Segundo pase de crítica

La propuesta revisada reúne las acciones en una vista "Acciones" del proyecto. La lista muestra nombre completo, efecto y acceso efectivo. Un detalle permite revisar entradas, resultado, límites, ruta y condiciones antes de abrir código.

| Caso | Qué debe poder comprobar la persona | Qué debe evitar el diseño |
| --- | --- | --- |
| Visitante lee un catálogo | La consulta devuelve campos concretos, tiene LIMIT y admite visitantes en la versión pública | Llamar pública a una acción bloqueada por la visibilidad privada de la app |
| Alice lee sus notas | La consulta filtra por la identidad de Alice y Bob no recibe esas filas | Etiquetar "solo sus datos" por encontrar `authenticated` o un binding de actor |
| Programa llama con clave | La ruta, método y clave admiten esa acción y la clave no abre herramientas de propietario | Ofrecer "Pública" cuando la intención era permitir un programa identificado |
| App cambia de privada a pública | La revisión identifica también las acciones anónimas y rutas sin callers explícitos cuyo acceso efectivo cambia | Mostrar solo el cambio de visibilidad global |
| La app sale de Konae | El paquete conserva declaraciones, cuentas, datos y reglas; se vuelve a generar el mismo contrato | Guardar permisos solamente en estado de UI o en el historial del chat |

Estos casos revisan el diseño contra las reglas existentes. No son pruebas de una UI nueva ni de comprensión de usuarios. En una condición SQL que la plataforma no pueda explicar con certeza, el detalle debe reconocerlo y ofrecer ver la regla y probarla con dos cuentas.

## Flujo elegido

1. La persona pide "Quiero que cualquiera consulte el catálogo y que cada cliente vea sus pedidos". El chat pregunta solo por ambigüedades que cambien el comportamiento o el acceso.
2. El agente propone cambios en las fuentes. La UI muestra las acciones creadas o modificadas, campos devueltos, límites y acceso efectivo. El contrato se regenera.
3. La persona prueba una acción como visitante, Alice o Bob con datos de prueba. Los casos de claves y firmas necesitan el Kernel de desarrollo; no se finge una comprobación desde el selector de personas.
4. "Permitir visitantes" cambia una decisión concreta de acceso. "Conectar otro programa" inicia la definición de ruta y credencial. La UI explica cuando la visibilidad de la app impide el resultado solicitado.
5. La UI distingue "Borrador" y "Publicado". Guardar una regla modifica las fuentes; publicar aplica una versión. La revisión de publicación muestra las diferencias de acceso efectivo.
6. Un rechazo nombra la acción, la regla y el siguiente paso. La persona puede volver a editar o pedir al chat que corrija el cambio.

La lista y el detalle son el mismo punto de consulta tanto si el cambio nació en chat, en una UI o en archivos. Las condiciones de SQL complejas se cambian mediante chat o código; los controles directos se limitan a decisiones que la plataforma puede representar y comprobar sin inventar una segunda política.

## Papel de la descripción

Tiene sentido incorporar una descripción opcional que explique para qué sirve la acción y cuándo usarla. Por ejemplo, "Lista los productos disponibles para preparar un pedido". El agente puede proponerla y la persona puede corregirla.

El contrato, la UI y un futuro catálogo de acciones pueden leerla desde una única declaración persistente. El resumen de permisos se deriva de las reglas y se muestra por separado. Cambiar la descripción nunca cambia SQL, acceso, visibilidad, callers ni credenciales.

Si se implementa, requiere soporte explícito en los parsers de TypeScript y Rust, contrato, esquema del contrato, exportación y pruebas de equivalencia. No conviene un campo de texto que desaparezca al generar o exportar.

## Tercer pase de crítica, reducir decisiones

La segunda propuesta todavía pide demasiado. Una lista con entradas, resultados, límites, rutas y permisos puede reproducir el JSON en una pantalla. Pedir que la persona pruebe cada acción como Alice y Bob también le asigna el trabajo de comprobar la implementación. Este pase sustituye ese recorrido obligatorio por una revisión del comportamiento de la app.

El punto de partida es una tarea, por ejemplo "Quiero recibir reservas". Konae propone un resultado concreto y pregunta solo lo que falta para decidir quién recibe información o puede cambiarla. Si la persona ya dijo "cualquiera puede reservar y solo yo veo las reservas", no vuelve a preguntar lo mismo mediante selectores.

La primera vista muestra frases propias de la app.

| Lo que se puede hacer | Quién puede hacerlo |
| --- | --- |
| Consultar horarios disponibles | Cualquiera, sin iniciar sesión |
| Reservar una cita | Cualquier persona que haya iniciado sesión |
| Ver una reserva | La persona que la hizo y tú |
| Cancelar una reserva | La persona que la hizo y tú |

Esta tabla es un ejemplo de propuesta, no una política predeterminada. Las condiciones las decide la persona y las aplica el Kernel. "Ver una reserva" no se muestra como verificada por encontrar un binding de identidad. Konae debe comprobar las condiciones y los casos de acceso pertinentes; si no puede demostrar una regla, debe decir qué queda por comprobar.

Cada fila ofrece "Cambiar". El cambio abre una pregunta contextual o permite describir otra regla. Los nombres técnicos completos, parámetros, límites y contrato quedan en un detalle para quien los necesita. El nombre visible conserva el objeto de la acción, como "Ver reservas", y evita títulos indistinguibles como "Listar filas".

"Conectar otra app" aparece cuando la persona quiere una integración. El flujo pregunta qué app y qué necesita hacer, y prepara el permiso mínimo correspondiente. Un ejemplo de revisión es "Contabilidad podrá leer las facturas. No podrá cambiarlas ni leer reservas". Los permisos deben corresponder a capacidades implementadas; si el Kernel no puede limitar el acceso como se pidió, el flujo explica esa limitación antes de conectar.

La conexión muestra su estado y permite retirarla. HTTP, MCP, claves y firmas aparecen solo cuando la conexión exige un paso manual o cuando la persona abre detalles técnicos. Conectar una app identificada no convierte sus operaciones en accesibles para todo internet. Tampoco hereda la identidad ni los privilegios de un usuario por ser una app conectada.

Konae prepara comprobaciones con datos de prueba para visitante, propietario, otra persona y conexión autorizada según el caso. El resumen presenta qué comprobó. "Probar como otra persona" queda disponible, sin ser un paso obligatorio para cada operación. Comprobar una reserva no demuestra todas las políticas de una app; el resumen delimita los casos ejecutados.

Publicar muestra los cambios relevantes, por ejemplo "Los horarios serán visibles sin iniciar sesión. Las reservas seguirán visibles solo para quien las hizo y para ti". Guardar deja el cambio pendiente. Las confirmaciones adicionales se reservan para ampliaciones de acceso o efectos que las necesitan, sin repetir la misma decisión en varias pantallas.

Se descarta "Pública/privada" como selector universal de una acción. La interfaz responde "quién puede hacer qué y sobre qué datos" con el contexto de la app. El acceso a la app y los permisos de sus operaciones siguen siendo reglas distintas, pero la persona revisa el resultado conjunto.

El criterio de aceptación es que una persona pueda responder sin abrir código quién ve una reserva, quién puede cancelarla y qué puede hacer Contabilidad. Debe poder cambiar una de esas decisiones sin configurar las demás ni aprender cómo funciona MCP. Queda pendiente probar esta comprensión con personas sin conocimientos técnicos.

## Alcance recomendado

### Permisos por acción acordados con el usuario

Cada acción debe permitir decidir quién puede ejecutarla. El modelo propuesto incluye acceso público, usuarios de la aplicación, roles definidos en esa aplicación, conexiones con API key y aplicaciones concretas de un ámbito propio. Estos permisos son requisitos de producto; el Kernel actual no implementa todo este modelo.

La UI empieza por "Quién puede ejecutar esta acción" y ofrece "Cualquiera" o "Solo quienes autorices". Elegir "Cualquiera" significa que no se exige identidad, aunque se conservan validaciones, límites y condiciones de datos. La UI debe resolver cualquier bloqueo de la visibilidad global antes de describir la acción como pública.

Dentro de "Solo quienes autorices", la persona añade únicamente los destinatarios que necesita.

| Destinatario | Decisión de la persona | Regla exigible |
| --- | --- | --- |
| Usuarios de esta app | Todos los que hayan iniciado sesión | Sesión válida de esta aplicación |
| Roles de esta app | Seleccionar roles, como Recepción o Administración | Pertenencia actual a uno de los roles elegidos |
| Una conexión con API key | Nombrar la conexión y autorizarla para esta acción | Clave válida y no revocada con permiso para esta acción |
| Aplicaciones concretas del ámbito propio | Seleccionar las apps por nombre | Identidad de aplicación comprobada y autorización explícita de la app receptora |

Se pueden combinar destinatarios. "Recepción o Contabilidad" admite a una persona con ese rol o a la app identificada; no obliga a Contabilidad a simular una cuenta humana. Elegir todos los usuarios hace redundante añadir un rol de esos mismos usuarios. Elegir acceso público hace redundantes los destinatarios restringidos, por lo que la UI evita mostrar una protección que ya no limita el acceso.

Los roles pertenecen a la aplicación y agrupan a sus usuarios. Se gestionan una vez y se seleccionan en cada acción. Quitar una pertenencia debe retirar el acceso correspondiente sin republicar el código. Tener un rol que ejecuta una acción no concede por sí mismo permiso para administrar roles o conexiones.

"Dominio" queda pendiente de precisar como límite de producto. Una coincidencia de hostname, Origin o Referer no prueba la identidad de otra app. La propuesta permite seleccionar aplicaciones de un ámbito administrado por el propietario y exige comprobar su identidad; no autoriza automáticamente todas las apps que comparten un sufijo DNS. Al exportar, una identidad de app que el destino no pueda comprobar queda sin acceso hasta restablecer la confianza.

El permiso para ejecutar y la condición sobre los datos permanecen separados. Recepción puede tener permiso para cancelar reservas y seguir limitada a las de su sede. La revisión final muestra ambos efectos en una frase de negocio. Las comprobaciones se aplican en el Kernel por cualquier vía de invocación.

Hoy `access` admite `anonymous` y `authenticated`, las rutas tienen `callers` y las claves autorizan acciones. El binding de rol de sesión distingue `owner` y `member`; los roles propios de una app pueden expresarse en sus tablas y SQL. Una administración común de roles configurables y autorizaciones entre identidades de aplicaciones requiere trabajo de modelo y Kernel, además de UI.

Primero, corregir identidad de acciones y mostrar el comportamiento efectivo con frases de la app, estado borrador/publicado y cambios antes de publicar. Después, añadir el recorrido contextual de "Cambiar" y "Conectar otra app". El contrato existente alimenta los detalles de consulta. Las credenciales quedan en el detalle técnico de la conexión cuando sean necesarias. El tercer pase define el recorrido principal; los campos técnicos del segundo pase son información de detalle.

No crear una herramienta MCP por acción en esta fase. Para un agente que consume la app, el hueco es descubrir las acciones y sus esquemas sin obtener permisos de administración ni SQL interno. Evaluar después un catálogo de ejecución limitado a la credencial y al Deployment publicado, conservando `actions_invoke` y todas sus comprobaciones. Ocultar una acción del catálogo nunca debe ser la comprobación de autorización.

## Verificación y límites

Se ejecutaron 72 pruebas existentes de `actions.test.ts`, `action-contract.test.ts`, `action-readiness.test.ts` y `auth-heading.test.ts`, con 489 aserciones y ningún fallo. Cubren parsers, contratos, callers, mensajes de acceso y generación. No prueban que la propuesta esté implementada.

El servidor de `http://localhost:4173` respondió HTTP 200. El host de automatización visual no estuvo disponible. No se inspeccionó la pantalla, no se borró almacenamiento y no se publicó ni se cambió acceso de ninguna app. La siguiente comprobación del flujo debe realizarse en el editor con los cinco casos del segundo pase.
