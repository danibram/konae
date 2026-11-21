# Rechazos de artefactos y límite del manifiesto portable

Esta revisión amplía la [evidencia TS anterior](../2026-11-20-ts-contract/README.md). Los 25 casos de declaraciones no cubrían la forma externa de `serverArtifact`. Un campo desconocido pasaba por el Gate y el deserializador Rust lo rechazaba con otra respuesta. También faltaba comprobar en publicación el límite de 256 KiB que ya tenía el lector de `server/manifest.json`.

Pasan 485 pruebas en 15 archivos, con 1486 aserciones y ningún fallo. El typecheck termina con código 0 y sin diagnósticos. [manifest.json](./manifest.json) registra los comandos, versiones, resultados y hashes de las fuentes. [tested-source.tar.gz](./tested-source.tar.gz) conserva los 40 archivos enumerados. Los logs están en [tests.txt](./tests.txt) y [typecheck.txt](./typecheck.txt).

## Contrato comprobado en TypeScript

Los 58 casos de `functions-artifact-cases.json` cubren los siguientes datos.

- Campos desconocidos en el artefacto, el manifiesto y una función. El rechazo nombra la primera clave en orden UTF-8, también cuando hay caracteres fuera del plano básico Unicode.
- Objetos ausentes, valores escalares, arrays, versiones o ABI incorrectos y tipos incorrectos en las declaraciones y los bundles.
- Valores por defecto aplicados a las fuentes. El artefacto debe incluir los valores normalizados; no puede omitir acceso o acciones ni cambiar el orden normalizado de sus arrays. El orden de campos de un objeto JSON no cambia su significado.
- Errores combinados que fijan la precedencia. Se comprueban ABI, claves, manifiesto y normalización, nombres y tipos de bundles, bytes, y finalmente la correspondencia con la declaración de origen.
- Metadatos de 32 funciones con 1948 referencias a acciones y texto no ASCII, a 262143, 262144 y 262145 bytes. Los dos primeros tamaños se escriben y vuelven a abrir. El tercero se rechaza.

El tamaño se mide sobre el JSON que contiene versión, ABI, manifiesto y hashes, con sangría de dos espacios y sin salto final. El Gate usa hashes provisionales de 64 caracteres hexadecimales. Ocupan exactamente lo mismo que los hashes calculados al guardar. La prueba comprueba la longitud de los bytes escritos, no solo el resultado del cálculo previo. El lector también rechaza un archivo cuyo espacio final lo eleva de 262144 a 262145 bytes.

## Estado del lado Rust

`functions.rs` incorpora `parse_server_artifact`, `parse_deploy_input`, el límite de metadatos y dos pruebas nuevas. Las pruebas consumen los mismos 58 casos y escriben y reabren las carpetas en los tamaños límite.

Este agente no ejecutó esas pruebas Rust ni compiló el Kernel. El agente del Kernel conectó `parse_deploy_input` en la entrada HTTP y en la lectura y aplicación de releases por MCP. La revisión de código confirma que la entrada HTTP conserva el `Refusal` antes de devolver un error genérico. La ejecución de las pruebas Rust y la comprobación de esas entradas reales siguen a cargo de ese agente.

La salida de tests adjunta prueba el comportamiento TS, el Gate, el build y el transporte del cliente. No prueba paridad universal para cualquier JSON ni ejecución real del Kernel, autenticación, aislamiento, transacciones o latencia. Las pruebas con funciones conservan un `throw` de nivel superior y no ejecutan el bundle.
