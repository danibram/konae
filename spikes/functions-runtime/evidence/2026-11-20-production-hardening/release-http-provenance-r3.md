# Recorridos HTTP y procedencia de imágenes, ronda r3

Fecha: 2026-11-20. No hubo procesos `cargo`, `rustc`, `docker buildx`, `docker run` ni `docker create` activos al reanudar.

## Pruebas HTTP ARM64

Se usaron las imágenes finales existentes, sin rebuild. Kernel: `konae-hardening-kernel:20261121-final`, ID `sha256:e2f2f17582a6f4308c76b0c8b4eb9c802b4f8863e8f834c380fbc3dd9f5dde56`. Konae: `konae-hardening-konae:20261121-final`, ID `sha256:14bc714d91c8e4cdaf9e3f961640adb7a9b15e026891d072a52af3e06c31a625`. Ambas son `linux/arm64` y empaquetan el runner `df1d3de9b6cbab0d41a28d23e1df810be735b9fe7a24716892619703a638330b`.

- `image-kernel-sandbox-denial-r4.log`: el POST autenticado bajo el perfil seccomp que deniega `landlock_create_ruleset` devolvió HTTP 503 `sandbox_unavailable`. Después de parar el Kernel, SQLite conservó `orders=[(1,6),(2,4)]`, `audit=[(1,7),(2,6)]` y los dos recibos existentes. El bundle habría actualizado la cantidad y añadido auditoría. No hubo efectos ni recibo nuevo.
- `image-konae-auth-sql-r3.log`: el entrypoint real de Konae aceptó la cookie de la app y la función autenticada devolvió HTTP 200 con `version=A` y la fila SQLite `quantity=6`. Se fijó `KONAE_FUNCTIONS_TIMEOUT_MS=5000` para admitir el arranque frío del fixture. La configuración por defecto de 1000 ms había vencido durante la inicialización de esta VM.
- Fixture regenerado por la prueba `real_functions_http_auth_transaction_publish_deadline_and_usage`, sin recompilar el runner. Su publicación final apuntaba al bundle de timeout; se restauró el bundle `HANDLER` que el mismo test declara y se sincronizaron `server/manifest.json`, `konae.json` y `.konae/deployment.json`. El bundle coincide con `app/functions/orders.ts`, SHA-256 `9ccc94813c13b4f8c0e4c6c20142f0d87b865287f0730b2725363bb3e51d1990`. El fixture temporal se retiró tras las pruebas.

## Procedencia AMD64 Konae

El runner nativo AMD64 generado en `release-runner-build-amd64-r1.log` tiene SHA-256 `1014c7a522dc260cab132cf52336626e5a03407baab1103fb59ea913c9b6e48c`. Se verificó como ELF x86-64 y se extrajo del Kernel AMD64 de esta misma ronda, ID `sha256:a4416e499051b21ba3a87bffd396b79cc11c49896e507d9d8bdd41a7a82b770c`; el hash extraído coincidió. Ese fue el archivo del contexto de `release-konae-amd64-r2.log` (BuildKit transfirió 95,02 MB).

La imagen que salió de r2, ID `sha256:3acbaee63b8cbbc8dd73c05e44cff1f261669fdf26111f1bac85b40bc9abc04d`, es `linux/amd64`, pero su runner tiene SHA-256 `c34a2012a05a70be037b6f32fafe4f9951e4ea162b69738f32774cc0945401b3`, distinto al contexto suministrado. El paso `COPY --from=functions-runner` figura `CACHED` en r2. La causa inmediata observada es una capa COPY cacheada con bytes que no corresponden al artefacto AMD64 usado por ese comando. No se pudo recuperar el contenido original que creó esa capa para atribuir su origen más atrás. La imagen r2 no es válida para publicación y no se publicó.

Para impedir que vuelva a pasar inadvertido, los dos Dockerfiles ahora copian a una ruta que incluye el SHA-256 esperado y lo verifican antes de mover el archivo a la ruta final. Los callers locales, Compose y release pasan ese hash. `release-konae-amd64-r3.log` muestra que BuildKit ejecutó el COPY a la ruta hash-nombrada en vez de reutilizar el COPY previo, pero falló al escribir/confirmar `metadata_v2.db` en solo lectura. No hay una imagen r3 completa ni se afirma que el checksum final haya pasado.

## Estado de almacenamiento y límite

Al reanudar había 130 MiB libres. Se borraron solo el runner AMD64 temporal ya verificado, fixtures temporales generados por esta ronda y directorios temporales propios vacíos. Se conservaron los ejecutables/logs/evidencia y las dos imágenes ARM64 usadas para HTTP. Quedaron 231 MiB libres.

Se intentó borrar únicamente estos tags de prueba propios: `konae-release-contract-konae:amd64`, `konae-release-contract-konae:latest`, `konae-release-contract-kernel:latest` y `konae-release-contract-kernel:amd64`. El daemon rechazó cada eliminación porque `/var/lib/desktop-containerd/daemon/io.containerd.metadata.v1.bolt/meta.db` está en solo lectura. `docker system df` también falla porque BuildKit no puede escribir `metadata_v2.db`. No se usó prune ni se tocó Docker.raw. No quedan contenedores activos. No se hicieron más builds con 231 MiB disponibles.

No hay VM Linux local alterna instalada (`podman`, `nerdctl`, `lima`, `colima` o `qemu-system-*`). La comprobación systemd/cgroup del procedimiento tampoco se ejecutó. El requisito externo preciso es recuperar escritura del almacén Docker Desktop y margen de almacenamiento antes de cualquier nueva compilación o contenedor de prueba.

## SHA-256 de cambios y registros

- `Dockerfile.kernel`: `7ebe40167f70f1f452b91ea4191ed50808a5beb775810c6dce48f5f18d0630eb`
- `Dockerfile.konae`: `801567d692983f8747eb5a73a7ec42e88c9131f241a4caf5341160882d45c55d`
- `.github/workflows/release.yml`: `95038ff91f0796a5241245ca128b76465a8c2e7a759df239bb4f353e0f79579a`
- `scripts/build-functions-images.sh`: `b1664408ad4db80a481ee0d60933b60f23a2688860d551a8afb213efdf163512`
- `scripts/dev-compose.sh`: `33626e2987bb21b49626a5a65e8437e0a8049d1d4fac2389e8d9fb292e2a94d8`. `git diff --check`, `sh -n`, `actionlint` y `docker compose config --quiet` pasan.
- `image-kernel-sandbox-denial-r4.log`: `6ce271b5b5fe33c5e477edffaf6039b0ba743c14b2eafdc8638ac99ba52ec241`
- `image-konae-auth-sql-r3.log`: `332777e9b0f61f96f654ba3e05dd7d54a7bb19b96410571ef09c0479d47b9958`
- `release-konae-amd64-r2.log`: `8d9a12e713a5a0cc48bb77b0b04c43abfae8b6e6557082c36ba237fa5eaa28f2`
- `release-konae-amd64-r3.log`: `6b6c1470a03e8c825764056a96559aff4ad89741b7aa7cca04c8fb82e2be4f5f`
