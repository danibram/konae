# Contrato TypeScript, Gate y transporte de funciones

Ejecución del 20 de noviembre de 2026 sobre el checkout compartido. Pasan 369 pruebas en 14 archivos, con 1366 aserciones y ningún fallo. `tsc --noEmit -p tsconfig.app.json` termina con código 0 y sin diagnósticos.

[manifest.json](./manifest.json) contiene los comandos completos, las versiones de Bun, Node, TypeScript y esbuild, el commit base y los SHA-256 de 36 archivos relacionados. [tested-source.tar.gz](./tested-source.tar.gz) conserva esos archivos. Es una captura del alcance TS y de las reglas compartidas, no de todo el checkout. El typecheck también leyó el trabajo concurrente de otros agentes.

[tests.txt](./tests.txt) conserva la salida de las pruebas. [typecheck.txt](./typecheck.txt) está vacío porque el compilador no emitió diagnósticos.

## Lo observado

- El parser y el Gate devuelven los códigos y mensajes de los 25 casos compartidos de `functions-cases.json`. El acceso omitido queda autenticado y el módulo `functions` no se activa por encontrar fuentes.
- El builder del editor, el CLI y el script generado de la carpeta producen ESM de servidor separado de `dist/`. Conservan una excepción de nivel superior en los bytes sin ejecutarla. La preview tampoco importa esa entrada.
- Los imports de servidor desde una entrada de navegador y desde una URL de CSS fallan antes de publicar sus bytes. El bundler rechaza imports de host, imports dinámicos sin destino conocido y bundles que no exportan `handle`.
- La prueba con Hono instalado produce un único bundle. Esa prueba usa `spikes/functions-runtime/app/node_modules/hono` y se omite si falta; no se omitió en esta ejecución.
- La carpeta transporta `app/package.json`, su lockfile, el manifiesto publicado y los bundles. Una alteración de bytes falla al verificar SHA-256. Las fuentes editables adelantadas no sustituyen los bundles exportados.
- El upload de publicación lleva `serverArtifact` junto al artefacto estático. El import lleva `initialReleased` sin reescribir el borrador. `pull` guarda `server/`, y `rehearse` lo copia al ensayo.
- Los generados de SDK coinciden con el contrato. El contrato describe `handle(Request, context)`, `context.actions.call`, `shutdown` opcional, idempotencia, límites UTF-8 y la ausencia de backend en preview.

## Límites de esta evidencia

Estas pruebas no arrancan un Kernel, un supervisor ni un ejecutor real de funciones. No prueban aislamiento, autenticación HTTP, transacciones, idempotencia nativa, latencia, rollback de publicación ni traslado entre dos instalaciones. Las pruebas de transporte sustituyen el extremo MCP y comprueban los bytes que el cliente envía o conserva.

El runtime de funciones sigue necesitando un runner experimental configurado por el operador. La indicación de usar `konae dev` o una Deployment no implica que cualquier instalación pueda ejecutarlo. Los cuerpos de respuesta del ABI actual deben ser UTF-8; no se documenta soporte binario ni fetch de red.

No se ejecutó Cargo, un build completo del editor, un despliegue, un commit ni un push. No se cambiaron los benchmarks históricos.
