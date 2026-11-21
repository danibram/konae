# Verificación HTTP del ejecutor con procesos

Esta ronda pasa las 18 comprobaciones de [hardening-http.json](hardening-http.json). [artifacts.json](artifacts.json) identifica el binario probado y las fuentes presentes al iniciar la prueba; [tested-script.py](tested-script.py) conserva el verificador ejecutado. La captura de fuentes no demuestra que todas correspondan a ese build, porque otros agentes trabajaban en el checkout. Los servidores y sus bases eran desechables y terminaron al acabar la prueba.

La prueba bloquea SQLite durante 350 ms frente a un presupuesto nativo de 100 ms. HTTP devuelve 504 en 101,22 ms y no cambia el pedido, la auditoría ni los recibos. Con Hono en el proceso Deno, un bloqueo de 550 ms frente a un presupuesto de 300 ms devuelve 504 en 302,84 ms y conserva esas mismas tablas. Repetir después con la misma clave produce una sola escritura.

También comprueba repetición concurrente, conflicto de payload, clave obligatoria, ausencia de arranque antes de llamar, evaluación inicial bloqueada, bucle síncrono y promesa sin resolver. Los tres bloqueos JavaScript vencen, el padre sigue respondiendo y la siguiente llamada crea una instancia que responde. Los contadores distinguen fallo durante inicialización de fallo después de crear el runtime.

Una capacidad guardada por Alice no permite leer datos durante la petición siguiente de Bob. El host devuelve `capability_expired` aunque el handler capture la excepción y trate de devolver una respuesta correcta.

Esta ronda no mide rendimiento bajo carga, no prueba límites cgroup y no ejecuta el dispatcher del Kernel. La prueba de commit confirmado con retorno tardío pertenece a la [evidencia nativa](../../rust-host/evidence/native-budget-v2/README.md).

## Repetición

Desde la raíz del repositorio, usa el binario construido de las fuentes actuales y un directorio de salida nuevo.

```sh
python3 spikes/functions-runtime/checks/hardening_http.py \
  --binary /tmp/konae-deno-core-target/release/konae-functions-spike \
  --output spikes/functions-runtime/checks/hardening-results-next
```
