# Limpieza de almacenamiento temporal propio

Fecha: 2026-11-20. Se revisaron procesos antes de retirar intermediarios. No había `cargo`, `rustc` ni `docker buildx` activos. `lsof` no mostró ningún proceso usando los dos ejecutables inspeccionados.

## Rutas verificadas y acción

- `/tmp/konae-deno-core-target` era el target macOS de este trabajo, identificado en el briefing. Ocupaba 2,2 GiB. Se conservaron `release/konae-functions-spike` (92 MiB, SHA-256 `98619085910b1fc10eb15a198e1938663f821e184d296dc279ba22eff812bd23`) y el directorio `release/`; se eliminaron solo `release/deps`, `release/build`, `release/gn_out`, `release/incremental` y `release/examples`. El target quedó en 92 MiB.
- `/tmp/konae-runner-artifact-unpacked/konae-functions-spike` era el artefacto ARM64 de esta ronda, SHA-256 `fa8dd0db1da173db59ae31127fab73c7555408a8e603cfada87b9f3b51724ca3`, ya usado en los builds y con hash guardado. Se eliminó el binario temporal; el directorio vacío quedó en 0 bytes.
- No se retiraron `/tmp/konae-integration-target`, `/tmp/konae-target-*`, `/tmp/konae-wave2-target` ni otros targets bajo `/tmp`; la inspección mostró que son de otros trabajos o su propiedad no se pudo establecer. Tampoco se modificó `kernel-rs/target`, la caché de Cargo, Docker.raw, imágenes, volúmenes ni caché global de Docker.

El espacio disponible subió de 418 MiB a 2,7 GiB. `docker desktop status` reporta `stopped`; `docker info` responde `Docker Desktop is unable to start` con `ServerVersion` vacío. El proceso de interfaz/backend de Docker Desktop sigue activo, pero no hay daemon utilizable. No se reinició el servicio global. No hay `podman`, `nerdctl`, `lima`, `colima` ni `qemu-system-*` disponibles para crear otra VM Linux local. Por eso no fue posible inspeccionar recursos propios dentro del daemon ni continuar pruebas de imágenes.
