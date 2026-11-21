# Evidencia de release e integración, ronda posterior

Fecha: 2026-11-20. Esta evidencia conserva el fallo del daemon de esta ronda y no sustituye los registros históricos.

## Contrato local del workflow

- runner_job_native_arch_matrix: PASS
- images_wait_for_runner_job: PASS
- image_matrix_has_both_products_and_arches: PASS
- artifact_is_unpacked_and_named_context_passed: PASS
- both_dockerfiles_consume_named_context: PASS

`actionlint .github/workflows/release.yml` pasó. El contrato estático confirma un build nativo por arquitectura, un artefacto por arquitectura, descarga en cada celda de imagen y contexto BuildKit nombrado. No se ejecutó GitHub Actions.

## Builds locales sin publicar

- ARM64: `scripts/build-functions-runner.sh` generó runner SHA-256 `fa8dd0db1da173db59ae31127fab73c7555408a8e603cfada87b9f3b51724ca3`. Los dos Dockerfiles consumieron el contexto y construyeron imágenes; registros `release-images-arm64-r1.log`.
- AMD64: runner SHA-256 `1014c7a522dc260cab132cf52336626e5a03407baab1103fb59ea913c9b6e48c`. `Dockerfile.kernel` construyó y exportó su imagen. La compilación de `Dockerfile.konae` empezó, pero falló al escribir `/var/lib/docker/buildkit/containerd-overlayfs/metadata_v2.db`: `read-only file system`. Registro `release-konae-amd64-r1.log`.
- No se hizo push. Los binarios runner temporales propios se borraron después de guardar sus hashes. Los IDs de imágenes finales no se pudieron consultar porque el daemon dejó de responder.

## Recorridos de imagen pendientes

- No se completó el HTTP E2E de Landlock denegado. La evidencia anterior solo demuestra rechazo del runner empaquetado y mapeo unitario a 503.
- La imagen Konae no recibió una solicitud autenticada de la función SQL. La evidencia previa solo verificó su entrypoint.
- No se ejecutó la comprobación de despliegue con delegación systemd. Docker Desktop no expone el host systemd al contenedor de prueba.

## Fallo del daemon

La última compilación informó que `metadata_v2.db` estaba en solo lectura. `docker info` quedó sin responder; se interrumpió esa consulta. `docker ps` tampoco pudo confirmar el estado. El host macOS tenía 127 MiB libres justo después del fallo. Se retiraron solo tres runners temporales propios y quedaron 399 MiB libres. No se reinició Docker ni se limpió caché/contenedor global. No se pudo recuperar una conexión al daemon dentro de esta ronda.

## Validaciones de fuentes

`actionlint` y `git diff --check` pasaron después de modificar ADR y documentación. Los builds de imágenes, prueba de HTTP, entrypoints y suite de Kernel siguen pendientes tras el error del daemon.

## SHA-256 de integración y evidencia

- `8fd895e9ff94a99375e20763b61d10171d4d6931bf8eb0e742b47b3bb8b8f0c8`  `.github/workflows/release.yml`
- `aef10d0ea50b11eaa63d129ce4bc0d9dfc62687129f4ce89046190789cb904e1`  `Dockerfile.functions-runner`
- `9996ac291098f41b2dd84b4ace0cdad7b8de211323e0d2a30320ca7895c4610a`  `Dockerfile.kernel`
- `0cd36af2fabb58e0a56e805399596c1132f0f6eead70cd7d70634b6977fcf9ed`  `Dockerfile.konae`
- `d9d9619cfa1d607078dc98537422619a7ffaab3b04211bcb21f2edafdedec2b7`  `scripts/build-functions-runner.sh`
- `8bc0c875d0573cde58db85b05fba854a3f0d8873ffb36d74bae74cdbe475ecec`  `scripts/build-functions-images.sh`
- `7234c0eb017ec43faee4c57e4e1e68b910bb9fc9cbf9b052b4be6b937194f25e`  `scripts/dev-compose.sh`
- `ae6e2b0ca4313031fb2d39e48ce7fdecee56bdbe71ea88ff773a4180117c3460`  `scripts/release-local.sh`
- `e54bbb40ed7194d030ce66ba6e03fbfb4086eb8684a94d84c3ec144e61b1c5f8`  `docs/operate.md`
- `5331afc18d5fdb89ef708611a73e9a3e7a206be44fd8442dd7b2b7e4f1d8cf88`  `docs/adr/0018-typescript-functions-load-on-demand-and-use-kernel-services.md`
