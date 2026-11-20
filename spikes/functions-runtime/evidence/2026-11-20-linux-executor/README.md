# Supervisor compilado y probado en Linux

El crate `functions-executor` compila en Linux ARM64 con Rust 1.91.1 y pasa cinco pruebas. [tests.txt](tests.txt) conserva el resultado. La política estricta rechaza una delegación ausente y archivos ordinarios que imitan controles cgroup. Las otras pruebas verifican límites de frames y transporte de errores y capacidades.

El contenedor de prueba no tenía red, capacidades Linux ni acceso al socket Docker. Su raíz era de solo lectura. Docker le impuso 1 GiB de memoria, sin swap adicional, dos CPU y 128 tareas. Solo montó las fuentes y el registro Cargo como lectura, y su directorio propio de evidencia como escritura. El contenedor terminó y se eliminó.

[manifest.json](manifest.json) identifica la imagen y las fuentes de [tested-source.tar.gz](tested-source.tar.gz). El script [check.sh](check.sh) copia el crate a un workspace temporal. Parte del lock del Kernel conservado en [kernel-input.lock](kernel-input.lock); [Cargo.lock](Cargo.lock) guarda la resolución del crate independiente. Cargo trabajó sin red. El primer intento falló porque el tmpfs no permitía ejecutar build scripts; el segundo usó versiones del registro local y pasó. La ronda final añadió el lock del Kernel y volvió a pasar.

Una prueba separada creó un contenedor Python con 64 MiB de memoria y solicitó un bloque finito de 128 MiB. Docker registró OOM y salida 137. [docker-oom.json](docker-oom.json) conserva límites y resultado. El contenedor también se eliminó.

## Límite de esta evidencia

La prueba OOM usa un cgroup gestionado por Docker. No demuestra que el supervisor haya creado y usado su propio subgrupo delegado. Esta máquina no proporcionó una delegación Linux escribible para esa prueba. Tampoco se compiló ni ejecutó V8 en Linux en esta ronda. No se confunde rechazo correcto sin delegación con éxito de la política estricta en una instalación preparada.
