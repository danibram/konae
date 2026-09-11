# Portabilidad del despliegue entre Cloudflare, Scaleway y Hetzner

_Investigación cerrada el 4 de septiembre de 2026. Solo usa documentación oficial. Las conclusiones que dependen de la ausencia de un producto o garantía se indican como inferencias._

## Decisión recomendada

Littlebox puede ser agnóstico del proveedor si define sus propios contratos y trata S3 como almacenamiento de objetos, no como plataforma de despliegue ni motor de base de datos.

La primera versión debería separar cuatro piezas:

1. El navegador compila una app y produce un artefacto estático inmutable.
2. `ObjectStore` guarda artefactos, blobs y generaciones de SQLite mediante un subconjunto pequeño de S3.
3. `StaticPublisher` decide cómo publicar esos objetos con HTTPS, dominio, caché y fallback de SPA.
4. Un Kernel compartido ejecuta acciones declaradas y serializa las escrituras de cada base de datos.

El usuario puede entregar `endpoint`, `region`, `bucket`, `accessKeyId` y `secretAccessKey` para subir a un bucket existente. Esas cinco propiedades no bastan para configurar de forma uniforme un sitio web, CDN, dominio o servicio de cómputo. Cada proveedor usa una API de control distinta.

La consecuencia práctica es importante. Littlebox puede mantener todos los bytes duraderos de la app en object storage, incluido cada snapshot SQLite, pero necesita cómputo para abrir SQLite y un coordinador para las escrituras. En la primera versión, el Kernel cumple ambos papeles.

## Comparación verificada

| Capacidad | Cloudflare | Scaleway | Hetzner |
| --- | --- | --- | --- |
| Object storage | R2 expone una API compatible con S3 y una API nativa para Workers. R2 ofrece consistencia fuerte para lecturas, escrituras, borrados y listados ([R2: funcionamiento](https://developers.cloudflare.com/r2/how-r2-works/), [modelo de consistencia](https://developers.cloudflare.com/r2/reference/consistency/)). | Object Storage se basa en el protocolo S3 y garantiza lectura tras escritura para `PUT`, sobrescrituras y `DELETE` ([FAQ de Object Storage](https://www.scaleway.com/en/docs/object-storage/faq/)). | Object Storage es compatible con S3 y usa endpoints regionales en Falkenstein, Núremberg y Helsinki ([descripción oficial](https://docs.hetzner.com/storage/object-storage/overview/)). La afirmación inicial de que Hetzner no tiene S3 ya no es correcta. |
| Hosting estático | Workers Static Assets publica archivos con caché global y soporta fallback de SPA. R2 también permite buckets públicos, pero `r2.dev` es solo para desarrollo y producción requiere un dominio personalizado ([Static Assets](https://developers.cloudflare.com/workers/static-assets/), [límites de R2](https://developers.cloudflare.com/r2/platform/limits/)). | Bucket Website publica un sitio estático. Al activarlo, Scaleway aplica por defecto una política que hace públicos todos los objetos del bucket. Edge Services añade caché, TLS y dominio ([Bucket Website](https://www.scaleway.com/en/docs/object-storage/how-to/use-bucket-website/), [Edge Services](https://www.scaleway.com/en/docs/edge-services/concepts/)). | Permite URLs públicas para objetos, pero no implementa la acción S3 `Website` ni dominios personalizados de bucket. Hetzner recomienda un CDN externo para distribución a gran escala ([acciones soportadas](https://docs.hetzner.com/storage/object-storage/supported-actions/), [FAQ](https://docs.hetzner.com/storage/object-storage/faq/general/)). |
| Escritura condicional | `PutObject` acepta `If-Match` y `If-None-Match`; algunos SDK requieren middleware para enviar esos headers ([compatibilidad S3 de R2](https://developers.cloudflare.com/r2/api/s3/api/), [ejemplo oficial](https://developers.cloudflare.com/r2/examples/aws/custom-header/)). | `PUT`, copia y cierre de multipart aceptan `If-Match` y `If-None-Match`. Scaleway también permite exigirlos mediante bucket policy, con restricciones para `CopyObject` ([escrituras condicionales](https://www.scaleway.com/en/docs/object-storage/api-cli/using-conditional-writes/)). | La lista oficial declara compatibilidad S3 básica y enumera excepciones, pero no promete de forma expresa condiciones sobre `PutObject` ([acciones soportadas](https://docs.hetzner.com/storage/object-storage/supported-actions/)). Littlebox debe considerarlas no disponibles hasta que una prueba de capacidades confirme su semántica. |
| Cómputo compartido | Workers ejecuta el Kernel y puede servir Static Assets. D1 ofrece semántica SQL de SQLite, recuperación y acceso por Worker o HTTP, pero es una opción específica de Cloudflare ([D1](https://developers.cloudflare.com/d1/)). | Serverless Containers ejecuta cualquier imagen HTTP, escala a cero y permite entre 0 y 50 réplicas según configuración ([API de Serverless Containers](https://www.scaleway.com/en/developers/api/serverless-containers)). | La ruta portable documentada es un Cloud Server, opcionalmente con Load Balancer y Volume. Un Volume solo puede conectarse a un servidor a la vez ([Cloud Servers](https://docs.hetzner.com/cloud/servers/overview/), [Volumes](https://docs.hetzner.com/cloud/volumes/overview/)). |
| Base administrada cercana | D1 usa el motor de SQLite y la mayor parte de sus convenciones SQL ([sentencias de D1](https://developers.cloudflare.com/d1/sql-api/sql-statements/)). | Serverless SQL Database es PostgreSQL, no SQLite, y puede entrar en reposo tras cinco minutos sin consultas ([descripción de Serverless SQL](https://www.scaleway.com/en/docs/serverless-sql-databases/reference-content/serverless-sql-databases-overview/)). | No se encontró en el catálogo oficial consultado un servicio administrado compatible con SQLite. La opción verificable es ejecutar SQLite en un servidor y su disco o Volume. Esta conclusión es una inferencia del catálogo, no una garantía de producto futura. |
| Credenciales | R2 emite claves S3. Los tokens de lectura y escritura de objetos pueden limitarse a buckets concretos, pero no administran buckets ni su configuración ([autenticación de R2](https://developers.cloudflare.com/r2/api/tokens/)). | Las API keys pertenecen a una persona o aplicación IAM, heredan sus políticas y fijan un proyecto preferido para S3 ([API keys y Object Storage](https://www.scaleway.com/en/docs/iam/api-cli/using-api-key-object-storage/)). | Las claves S3 valen por defecto para todos los buckets del proyecto. Una bucket policy puede bloquear el resto y permitir una clave concreta ([credenciales S3](https://docs.hetzner.com/storage/object-storage/faq/s3-credentials/)). |

La documentación pública consultada no presenta un servicio serverless de Hetzner equivalente a Workers o Serverless Containers. Por eso la tabla limita la afirmación a la ruta que sí documenta Hetzner, una VM compartida. El adaptador no debe codificar esa diferencia en la app generada.

## Contratos que Littlebox debe poseer

El contrato de object storage debería admitir solo operaciones que existen en los tres proveedores:

```ts
interface ObjectStore {
  get(key: string): Promise<StoredObject | null>;
  head(key: string): Promise<ObjectMetadata | null>;
  put(key: string, body: Body, metadata: PutMetadata): Promise<PutResult>;
  delete(key: string): Promise<void>;
  list(prefix: string, cursor?: string): Promise<ObjectPage>;
}

interface ObjectStoreCapabilities {
  conditionalPut: boolean;
  publicObjects: boolean;
  presignedUrls: boolean;
}
```

`ETag` debe tratarse como un token opaco. No debe usarse como hash de contenido, sobre todo con uploads multipart. Una función como `putIfMatch` pertenece a una extensión comprobada en tiempo de conexión, no al contrato mínimo.

Publicar es otro contrato:

```ts
interface StaticPublisher {
  publish(input: {
    artifactPrefix: string;
    entrypoint: string;
    spaFallback: boolean;
  }): Promise<{ originUrl: string }>;
}
```

Las implementaciones iniciales pueden ser:

- `CloudflareStaticAssetsPublisher` o `R2CustomDomainPublisher`.
- `ScalewayBucketWebsitePublisher`, con Edge Services como opción.
- `ReverseProxyPublisher`, válido para Hetzner y cualquier S3 genérico.

El último puede vivir junto al Kernel. Resuelve `/apps/{deployment}/...`, lee objetos privados y aplica `Cache-Control`, fallback de SPA y HTTPS en un único sitio. Es el único comportamiento uniforme sin pedir credenciales de la API de control de cada proveedor.

## SQLite almacenado como objetos

No conviene tratar un fichero SQLite dentro de S3 como si el motor pudiera abrirlo allí. Hetzner avisa de que su Object Storage no sustituye una base de datos y no está pensado para lecturas o escrituras frecuentes de ficheros pequeños ([FAQ de Hetzner](https://docs.hetzner.com/storage/object-storage/faq/general/)). El mismo límite mecánico existe en cualquier object store: SQLite necesita acceso aleatorio y locking de fichero, mientras S3 reemplaza objetos completos.

Para bases pequeñas, el Kernel puede mantener una copia de trabajo local y persistir generaciones inmutables:

```text
littlebox/
  apps/{appId}/deployments/{deployId}/artifact/...
  databases/{databaseId}/generations/{generationId}.sqlite
  databases/{databaseId}/head.json
  blobs/{databaseId}/...
```

Una mutación seguiría esta secuencia:

1. El router entrega todas las escrituras de `databaseId` al mismo coordinador.
2. El coordinador ejecuta una acción declarada dentro de una transacción SQLite local.
3. Sube una generación nueva con una clave que nunca se reutiliza.
4. Actualiza `head.json` y conserva la generación anterior para rollback.

`If-Match` puede proteger el paso 4 en R2 y Scaleway. No puede ser el único mecanismo de exclusión si Hetzner forma parte del contrato. La primera versión debe usar un solo escritor por base de datos. Una versión con varias réplicas necesitará un `WriteCoordinator` con fencing, implementado mediante una base de control, un actor por clave o un servicio de consenso. Guardar las generaciones en S3 sigue siendo portable aunque el coordinador cambie.

Subir el SQLite completo después de cada mutación limita el tamaño y la frecuencia de escritura. La versión inicial debe imponer cuotas medidas, por ejemplo tamaño máximo de base, bytes persistidos por minuto y una sola mutación activa. Más adelante, un journal append-only puede agrupar snapshots, pero exige definir recuperación, compactación y checksum antes de sustituir este modelo.

## Qué permiten unas credenciales S3

Un endpoint de despliegue neutral puede aceptar este objeto:

```json
{
  "endpoint": "https://...",
  "region": "...",
  "bucket": "...",
  "accessKeyId": "...",
  "secretAccessKey": "...",
  "prefix": "littlebox/"
}
```

Con esas credenciales puede verificar acceso, subir artefactos y mantener datos privados. No puede asumir que puede:

- crear el bucket;
- habilitar website hosting;
- crear un dominio o certificado;
- configurar un CDN;
- desplegar Workers, Functions, Containers o servidores;
- crear una credencial nueva con menos permisos.

R2 separa los tokens de objetos de la administración de buckets. Scaleway usa IAM y una API de control propia. Hetzner crea las credenciales S3 desde Console y las abre a todo el proyecto por defecto. Un onboarding que solo solicite S3 debe partir de un bucket ya creado y dejar la publicación avanzada como configuración manual o como un adaptador opcional del proveedor.

El backend recibe el secreto una vez, prueba las capacidades y lo cifra si el Kernel va a leer la base después del despliegue. Borrarlo tras subir el estático solo funciona para una app sin datos. El bundle público nunca recibe estas claves. El navegador compila, pero envía el artefacto a la API administrativa para desplegarlo.

## Trampas de portabilidad

- No mezclar el artifact público y la base privada en un bucket que se vuelve público. Bucket Website de Scaleway hace públicos todos los objetos por defecto. Usar dos buckets es la regla sencilla: uno para apps y otro privado para bases y blobs.
- No modelar `Website`, ACLs, dominios, CDN, WAF o lifecycle como parte de S3. R2 no implementa ACLs ni website metadata en su API S3, y Hetzner no implementa Website ni dominios de bucket ([compatibilidad S3 de R2](https://developers.cloudflare.com/r2/api/s3/api/), [acciones de Hetzner](https://docs.hetzner.com/storage/object-storage/supported-actions/)).
- No derivar seguridad del nombre de bucket o prefijo. El Kernel resuelve el deployment y solo entonces entrega una clave interna al adaptador.
- No incluir `databaseId`, SQL ni credenciales en el bundle. El cliente llama a una acción por identificador y el Kernel valida actor, input, statement parametrizado, output y cuotas.
- No asumir el mismo valor para `region` ni la misma forma de endpoint. R2 usa `auto` en los ejemplos de su API S3; Scaleway fija un proyecto preferido para cada API key; Hetzner usa regiones como `fsn1` y virtual-host addressing.
- No firmar requests S3 de larga duración en el navegador. Las URLs prefirmadas sirven para una operación concreta. El deploy administrativo y las lecturas de base pasan por el Kernel.
- No usar el hostname público del bucket como identidad estable de la app. El manifest debe registrar un `deploymentId` propio y el publicador devuelve una URL reemplazable.
- No dar por hecho que el cache refleja el object store. R2 documenta que un dominio con cache puede seguir sirviendo una versión borrada o sobrescrita hasta purgarla o agotar el TTL ([consistencia y cache de R2](https://developers.cloudflare.com/r2/reference/consistency/)). Los artifacts deben usar rutas con hash y Littlebox solo cambia el puntero del deployment.

## Primera implementación

La combinación con menos dependencias específicas sería:

1. Build WASM en el navegador.
2. Un Kernel escrito contra `Request` y `Response` de Fetch. El paquete ofrece una entrada Worker para Cloudflare y una imagen OCI para Scaleway Serverless Containers o una VM Hetzner.
3. Un `S3ObjectStore` configurable por endpoint, región, bucket y prefijo.
4. Un `ReverseProxyPublisher` dentro del Kernel como publicación universal.
5. Dos buckets o dos credenciales: artifacts públicos por proxy y datos privados.
6. SQLite local en el Kernel, una cola de escritura por base y snapshots inmutables en S3.
7. Acciones declaradas por string. La app nunca envía SQL y el manifest público solo contiene identificadores y tipos necesarios para el SDK.
8. Pruebas contractuales contra R2, Scaleway y Hetzner. La prueba decide capacidades como conditional `PUT`, CORS, multipart, metadata y consistencia observada.

Cloudflare sigue siendo un buen primer host del Kernel, pero D1 y Workers Static Assets deben quedar detrás de adaptadores opcionales. Scaleway permite una versión más portable del Kernel con una imagen OCI y un S3 bastante completo. Hetzner también encaja desde object storage y VM; requiere proxy/CDN para una publicación web cuidada y operación propia del proceso compartido.
