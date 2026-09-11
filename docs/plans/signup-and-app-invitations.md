# Registro e invitaciones a apps

Detalle de implementación iniciado el 5 de noviembre y revisado el 6 de noviembre de 2026. El [plan de lanzamiento](platform-launch.md) fija el alcance, los casos de Eject y traslado y los criterios para abrir la beta. Este documento desarrolla el acceso y las invitaciones; no describe funciones ya implementadas.

## Resultado esperado

Una persona se registra en Littlebox, conecta su agente por MCP y le pide una app con usuarios. El agente crea y publica la app con su política de registro. El creador permite que cualquiera se registre o invita a personas por correo. Cada usuario entra en esa app y accede solo a los datos que sus acciones le permiten. Exportar conserva las cuentas, sus identificadores, sus contraseñas y las reglas.

## Decisiones propuestas

- Usar Clerk con Google para registrar al creador en Littlebox alojado. Mantener el acceso local del propietario para instalaciones propias.
- Conservar un ID interno para cada creador. Asociar la identidad externa por proveedor y sujeto, nunca unir cuentas por coincidencia de correo.
- Mantener cuentas y sesiones de usuarios de apps en el Kernel, separadas por Deployment. Ser propietario del proyecto no concede una sesión de usuario de la app.
- Usar correo y contraseña para las cuentas de apps en esta versión. Verificar el correo al registrarse; la invitación recibida por correo sirve como verificación al aceptarla.
- Usar Resend como primer transporte de correo alojado. Littlebox configura una cuenta de envío para la instalación; cada creador no tiene que contratar ni configurar un proveedor.
- Configurar el transporte fuera de la app y sus archivos. El Kernel debe permitir otro transporte, incluido SMTP para instalaciones propias.
- Mantener la autorización de acciones y filas en la app. Esta entrega no añade un sistema general de roles personalizados.

## Flujo del creador

1. La persona pulsa "Continuar con Google" en Littlebox. Creamos su cuenta interna una sola vez y abrimos su lista de proyectos.
2. Conecta su cliente MCP. La autorización existente de Littlebox muestra los permisos que solicita el agente y conserva el destino al volver del registro.
3. Pide, por ejemplo, "Crea una app de gastos con cuentas personales. Solo pueden entrar las personas que invite. Invita a ana@example.com".
4. El agente crea el proyecto, genera las tablas y las acciones, y usa el SDK de Littlebox para el acceso. No genera un servicio de autenticación propio ni guarda contraseñas en las tablas de negocio.
5. El agente prepara la publicación con visibilidad y política de registro explícitas. El Kernel aplica ambas antes de servir la primera petición.
6. Tras publicar y obtener una URL estable, el agente solicita el envío de las invitaciones indicadas por el creador.
7. Littlebox devuelve la URL y el estado de cada invitación. Un correo pendiente o fallido no aparece como entregado.

El agente puede crear la app completa desde MCP. El editor muestra y modifica los mismos datos mediante las mismas operaciones. No se exige visitar una pantalla para completar un paso que el MCP puede ejecutar con la autorización concedida.

## Flujo de quien usa la app

### Registro abierto

1. Abre la app y pulsa "Crear cuenta".
2. Introduce correo y contraseña. El Kernel crea un registro pendiente y envía la verificación.
3. Abre el enlace y confirma el registro. El Kernel consume el token una sola vez y activa la cuenta.
4. Entra con una sesión limitada a esa app. Antes de la verificación solo puede completar el registro, reenviar la verificación o salir.

### Acceso por invitación

1. Recibe un correo con el nombre de la app, quién le invita y un enlace que caduca.
2. Abre el enlace y ve el correo invitado, sin poder sustituirlo por otro.
3. Establece su contraseña y acepta. El Kernel activa la cuenta y consume la invitación en una misma transacción.
4. Entra directamente en la app. No necesita una cuenta de creador en Littlebox.

Una invitación pendiente no es una cuenta activa. Si el destinatario ya tiene una cuenta activa en esa app, el flujo pide iniciar sesión; la invitación no sustituye su contraseña ni restablece sus credenciales. Una cuenta suspendida no se reactiva mediante una invitación.

### Recuperación

El usuario puede pedir un correo para recuperar su contraseña. La respuesta no revela si existe la cuenta. Al completar el cambio, el Kernel consume el token e invalida las sesiones anteriores. El propietario puede suspender una cuenta y revocar su acceso, pero no leer ni escoger la contraseña de otra persona.

## Políticas y visibilidad

| Política | Registro sin invitación | Invitación válida |
| --- | --- | --- |
| Abierto | Permitido tras verificar el correo | Permitida |
| Solo invitación | Denegado | Permite crear la cuenta |
| Cerrado | Denegado | No permite crear cuentas mientras siga cerrado |
| Lista permitida, compatible con lo existente | Solo correos o dominios permitidos, tras verificarlos | Debe respetar también la lista |

La interfaz principal ofrece Abierto y Solo invitación. Cerrado queda como control para pausar altas. La lista permitida existente se conserva como opción avanzada; introducir una dirección en esa lista no envía una invitación.

La visibilidad sigue siendo independiente. Una app pública puede mostrar su portada a visitantes y exigir una cuenta para datos privados. Una app privada exige una cuenta activa antes de mostrar su contenido. Las rutas mínimas de registro, invitación, recuperación y acceso deben funcionar antes de iniciar sesión, sin exponer archivos privados.

Para una app con cuentas, el agente declara ambas opciones en la primera publicación. Si la petición no determina el acceso, propone privada y Solo invitación. Al republicar se conservan los valores vigentes salvo un cambio explícito. Cambiar a Cerrado detiene las altas pendientes; no expulsa a usuarios existentes. Cada aceptación comprueba de nuevo la política bajo el bloqueo de escritura correspondiente.

## Qué existe y qué falta

| Área | Estado observado | Trabajo pendiente |
| --- | --- | --- |
| Acceso del creador | Cuenta local con clave de instalación | Registro alojado con Clerk y asociación a ID interno |
| MCP | Proyectos, publicación, OAuth y credenciales delegadas | Permiso y herramientas de gestión de usuarios |
| Cuentas de apps | Contraseña y sesión por Deployment | Verificación, recuperación, estados e invitaciones |
| Política de registro | Open, Allowlist y Closed | Solo invitación y validación durante activación |
| Editor | Panel de acceso y política | Usuarios, invitaciones y estados de correo |
| Exportación | Identidad y política junto con los datos | Nuevos estados y reglas para tokens y correo pendientes |

La lista permitida actual comprueba el texto del correo, sin demostrar que el solicitante lo controla. La migración no debe marcar esas cuentas como verificadas ni conceder permisos nuevos a quien solo conoce una dirección.

## Datos y límites de confianza

La plataforma mantiene la asociación entre identidad externa y propietario interno. Para la primera entrega se conserva el registro actual de proyectos por propietario. Un espacio personal no necesita introducir colaboración de equipos; si se añaden Workspaces persistentes, la migración debe preservar propietarios y permisos existentes.

Cada `identity.sqlite` añade estado de cuenta, fecha de verificación, invitaciones y desafíos de verificación o recuperación. Los registros de negocio siguen referenciando el ID estable del usuario. Suspender o cambiar un correo no cambia ese ID ni reasigna sus filas.

Una invitación guarda destinatario normalizado, Deployment, creador, vencimiento, estado y hash del token. Los desafíos tienen propósito explícito; un token de recuperación no acepta invitaciones. Proponemos siete días para invitaciones, 24 horas para verificar registros y 30 minutos para recuperación, con límites configurables por instalación.

La cola de correo guarda el estado de envío por separado de la aceptación. Los secretos necesarios para enviar enlaces se cifran mientras esperan en la cola y se eliminan al terminar. No aparecen en respuestas MCP, logs ni exportaciones. Los enlaces se construyen con el origen configurado y validado del Deployment.

Abrir un enlace mediante GET no consume el token. La persona confirma mediante una operación POST, para que los analizadores de correo no acepten invitaciones ni restablezcan contraseñas.

## Contrato MCP propuesto

Los nombres siguientes son nuevos salvo las herramientas de política y publicación.

| Operación | Resultado |
| --- | --- |
| `release_plan`, `release_apply` | Incluyen la política inicial y la instalan antes de abrir la app |
| `signup_policy_read`, `signup_policy_set` | Leen y actualizan también Solo invitación |
| `app_users_list` | Lista paginada de cuentas, verificación y estado |
| `app_user_suspend`, `app_user_resume` | Cambian el acceso sin borrar datos ni alterar contraseñas |
| `app_invitations_create` | Crea invitaciones por correo y las pone en cola, con resultado por destinatario |
| `app_invitations_list` | Lista paginada con aceptación, caducidad y estado de envío |
| `app_invitation_resend` | Sustituye el token anterior y solicita un nuevo envío |
| `app_invitation_revoke` | Invalida una invitación pendiente |

Las herramientas comprueban la pertenencia del Deployment al proyecto autorizado. Un permiso nuevo `manage_users` autoriza consultar cuentas e invitaciones, cambiar políticas, invitar y suspender. `read` conserva la consulta de metadatos del proyecto y su política, sin habilitar listas de datos personales. El propietario tiene ese permiso, pero los tokens existentes no lo reciben automáticamente. El cambio desde el actual permiso `publish` para modificar políticas necesita versionar el contrato y actualizar los clientes.

La autorización de publicación permite definir la política inicial dentro del plan. Cambiar posteriormente quién puede entrar requiere `manage_users`, también cuando la petición llega dentro de una republicación. Un token no puede evitar esa comprobación escogiendo otra herramienta.

Crear o reenviar invitaciones requiere una clave de idempotencia. Los reintentos de un agente devuelven el mismo resultado y no generan una nueva invitación ni un nuevo envío. Los lotes tienen un máximo explícito y resultados individuales. El catálogo explica que invitar envía un correo y el agente lo hace cuando el creador solicita ese envío.

## Correo y operación

- Guardar invitación y trabajo de envío en una misma transacción. Un fallo de red no pierde la solicitud.
- Procesar una cola persistente con reintentos limitados y claves de idempotencia del proveedor. Distinguir pendiente, aceptado por el proveedor, entregado al servidor de correo y fallido.
- Verificar la firma de los eventos del proveedor y tolerar duplicados y eventos fuera de orden. La entrega no demuestra que el usuario leyera el mensaje.
- Limitar envíos por instalación, creador, app y destinatario. Aplicar también límites al registro, recuperación y aceptación de tokens.
- Usar plantillas de la plataforma con nombre de app escapado, enlaces controlados y remitente de Littlebox. No permitir contenido arbitrario del agente en esta primera entrega.
- Si no hay transporte configurado, explicar el error antes de activar registro abierto o enviar invitaciones. Una importación conserva la política, pero muestra que las altas y la recuperación no están disponibles hasta configurar correo. El acceso de cuentas existentes sigue funcionando si el operador activa expresamente ese modo limitado. En desarrollo, un buzón local permite probar todo el flujo sin correo externo.

## Plan de implementación

### 1. Cuentas verificadas en el Kernel

Añadir las migraciones de identidad, estados, tokens de un solo uso, verificación, recuperación y suspensión. Resolver los conflictos con cuentas pendientes y el cambio de contraseña sin permitir que un registro previo no verificado capture una invitación posterior. No emitir sesiones de runtime activas para cuentas pendientes o suspendidas.

Migrar las cuentas heredadas sin verificación a verificación requerida, revocar sus sesiones y exigir prueba del correo y establecimiento de contraseña antes de entrar en producción. Conservar IDs y filas. El estado previo no demuestra que quien escogió la contraseña controlase el correo. Las instalaciones sin correo siguen en modo local; no se promocionan a producción con esas cuentas activas.

Archivos principales: `kernel-rs/src/auth.rs`, `kernel-rs/src/main.rs` y sus pruebas de seguridad.

### 2. Invitaciones y transporte de correo

Implementar invitaciones, cola persistente, adaptador Resend, buzón de desarrollo y configuración SMTP para instalaciones propias. Añadir aceptación, recuperación y los estados visibles de envío. Enlazar el alta verificada de la fase anterior al transporte.

Salida comprobable: una persona invitada crea su contraseña y entra; una persona sin invitación no puede crear cuenta en esa app.

### 3. Gestión mediante MCP y primera publicación

Añadir las herramientas, permisos, idempotencia y auditoría. Ampliar el plan de publicación para guardar la política inicial de forma atómica. Actualizar `platform_guide`, los contratos de cliente y las instrucciones del agente. Las cuentas de demostración solo existen en la vista previa; el agente invita a personas reales sin generar contraseñas para ellas.

Salida comprobable: un cliente MCP crea una app, la publica con Solo invitación y solicita correos sin recurrir a pasos manuales en el editor.

### 4. Registro del creador y conexión MCP

Integrar Clerk en el acceso alojado. Validar en servidor firma, emisor, vigencia y origen autorizado de la prueba de identidad. Asociar su sujeto a un propietario interno mediante una restricción única, crear la cuenta una vez y emitir la sesión propia de Littlebox. No aceptar tokens de Clerk como credenciales de usuarios de apps.

Definir vencimiento y revocación de la sesión de Littlebox y sus credenciales delegadas. Procesar la baja o suspensión del creador e invalidar su acceso; cerrar sesión en el navegador no revoca por sí solo todas las delegaciones MCP. Mantener la autorización OAuth existente con consentimiento y permisos del Kernel.

Preservar el retorno al cliente MCP después del registro. El acceso local con clave de instalación queda disponible en instalaciones propias. Vincular una cuenta local existente exige autenticar ambas identidades, sin unirlas por correo.

Salida comprobable: un creador nuevo se registra y autoriza su agente sin conocer una clave de instalación.

### 5. Editor y publicación

Añadir una vista de Usuarios con cuentas activas, suspendidas e invitaciones pendientes. Permitir invitar por correo, reenviar, revocar y cambiar la política mediante MCP. El diálogo de publicación muestra visibilidad y registro antes de publicar.

La interfaz de acceso incluye registro, verificación pendiente, invitación caducada, recuperación y correo fallido. Evitar mensajes que confundan crear una app con entrar como usuario de esa app.

Archivos principales: `src/components/PlatformAccess.tsx`, `src/components/CapabilityWorkspace.tsx`, `src/components/DeployDialog.tsx`, `src/mcp/` y páginas de acceso del Kernel.

### 6. Exportación y documentación

Aplicar las operaciones diferenciadas del plan de lanzamiento. Eject mantiene el alojamiento y sus sesiones; una copia completa exporta IDs, hashes de contraseña, verificación, suspensión y política. Invalidar sesiones, enlaces de recuperación y tokens de invitación al importar esa copia. Las invitaciones pendientes se conservan como historial que necesita reenvío, nunca como enlaces todavía válidos al host anterior. Excluir claves del proveedor y trabajos de correo pendientes. Un traslado exige además pausar el origen y coordinar el estado final y la activación del destino.

El destino configura su transporte y puede reenviar invitaciones pendientes. Los usuarios activos conservan su contraseña y sus filas sin depender de Clerk ni del host anterior. Probar la restauración sobre una instalación nueva.

Actualizar `docs/mcp.md`, arquitectura y la decisión de identidad para documentar Clerk como prueba externa del creador y el Kernel como autoridad de cuentas, sesiones y permisos. Actualizar el vocabulario de políticas cuando se implemente el cambio.

## Pruebas de aceptación

- Un creador nuevo conecta MCP, publica una app y envía una invitación; el destinatario entra con sus propias credenciales.
- El registro abierto exige verificar el correo antes de acceder como usuario autenticado.
- Escribir el correo de una persona invitada o de un dominio permitido no concede acceso.
- Un token de otra app, caducado, revocado o ya usado no crea una cuenta. Dos aceptaciones simultáneas producen una sola cuenta y un solo consumo.
- Visitar enlaces desde un analizador de correo no consume tokens ni cambia credenciales.
- Una cuenta pendiente anterior no conserva una contraseña elegida por un tercero cuando su destinatario verifica o acepta una invitación.
- Un agente limitado a un proyecto no puede consultar ni invitar usuarios de otro. Sin `manage_users` no puede cambiar el acceso de una app existente mediante una republicación.
- Reintentar una solicitud MCP o reiniciar el procesador de correo no duplica invitaciones. Los fallos de entrega quedan visibles y permiten reintentar.
- Cambiar a Cerrado impide activar altas pendientes. Suspender una cuenta invalida su acceso en las sesiones existentes sin borrar sus filas.
- Dos usuarios de una app de notas privadas no pueden leer ni modificar las filas del otro. Las consultas siguen exigiendo LIMIT en el editor y el Kernel.
- La primera petición de una app recién publicada encuentra su política definitiva. Republicar conserva cuentas, datos y política salvo cambio explícito autorizado.
- Tras exportar e importar en otra máquina, el usuario conserva ID, contraseña y datos. Los tokens antiguos fallan y las nuevas invitaciones usan el dominio del destino.

## Referencias verificadas

- [Verificación de tokens de sesión de Clerk](https://clerk.com/docs/guides/sessions/manual-jwt-verification). La integración debe validar la prueba en el servidor y restringir los orígenes autorizados.
- [Claves de idempotencia de Resend](https://resend.com/changelog/idempotency-keys). Complementan la deduplicación persistente que mantiene Littlebox.
- [Estado actual del MCP](../mcp.md).
- [Decisión actual sobre identidad](../adr/0002-kernel-owns-identity-and-visibility.md).
