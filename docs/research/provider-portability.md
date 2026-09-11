# Deployment portability across Cloudflare, Scaleway and Hetzner

_Research closed on September 4, 2026. It uses official documentation only. Conclusions that depend on the absence of a product or guarantee are marked as inferences._

## Recommended decision

Littlebox can be provider-agnostic if it defines its own contracts and treats S3 as object storage, not as a deployment platform or a database engine.

The first version should separate four pieces:

1. The browser compiles an app and produces an immutable static artifact.
2. `ObjectStore` stores artifacts, blobs and SQLite generations through a small subset of S3.
3. `StaticPublisher` decides how to publish those objects with HTTPS, domain, caching and SPA fallback.
4. A shared Kernel runs declared actions and serializes the writes to each database.

The user can supply `endpoint`, `region`, `bucket`, `accessKeyId` and `secretAccessKey` to upload to an existing bucket. Those five properties are not enough to configure a website, CDN, domain or compute service in a uniform way. Each provider uses a different control API.

This has a practical consequence. Littlebox can keep all of an app's durable bytes in object storage, including each SQLite snapshot, but it needs compute to open SQLite and a coordinator for writes. In the first version, the Kernel plays both roles.

## Verified comparison

| Capability | Cloudflare | Scaleway | Hetzner |
| --- | --- | --- | --- |
| Object storage | R2 exposes an S3-compatible API and a native API for Workers. R2 offers strong consistency for reads, writes, deletes and lists ([R2: how it works](https://developers.cloudflare.com/r2/how-r2-works/), [consistency model](https://developers.cloudflare.com/r2/reference/consistency/)). | Object Storage is based on the S3 protocol and guarantees read-after-write for `PUT`, overwrites and `DELETE` ([Object Storage FAQ](https://www.scaleway.com/en/docs/object-storage/faq/)). | Object Storage is S3-compatible and uses regional endpoints in Falkenstein, Nuremberg and Helsinki ([official overview](https://docs.hetzner.com/storage/object-storage/overview/)). The initial claim that Hetzner has no S3 is no longer correct. |
| Static hosting | Workers Static Assets publishes files with global caching and supports SPA fallback. R2 also allows public buckets, but `r2.dev` is for development only and production requires a custom domain ([Static Assets](https://developers.cloudflare.com/workers/static-assets/), [R2 limits](https://developers.cloudflare.com/r2/platform/limits/)). | Bucket Website publishes a static site. When it is enabled, Scaleway applies by default a policy that makes every object in the bucket public. Edge Services adds caching, TLS and a domain ([Bucket Website](https://www.scaleway.com/en/docs/object-storage/how-to/use-bucket-website/), [Edge Services](https://www.scaleway.com/en/docs/edge-services/concepts/)). | It allows public URLs for objects, but it does not implement the S3 `Website` action or custom bucket domains. Hetzner recommends an external CDN for large-scale distribution ([supported actions](https://docs.hetzner.com/storage/object-storage/supported-actions/), [FAQ](https://docs.hetzner.com/storage/object-storage/faq/general/)). |
| Conditional writes | `PutObject` accepts `If-Match` and `If-None-Match`; some SDKs need middleware to send those headers ([R2 S3 compatibility](https://developers.cloudflare.com/r2/api/s3/api/), [official example](https://developers.cloudflare.com/r2/examples/aws/custom-header/)). | `PUT`, copy and multipart completion accept `If-Match` and `If-None-Match`. Scaleway can also enforce them through a bucket policy, with restrictions for `CopyObject` ([conditional writes](https://www.scaleway.com/en/docs/object-storage/api-cli/using-conditional-writes/)). | The official list declares basic S3 compatibility and lists exceptions, but it does not explicitly promise conditions on `PutObject` ([supported actions](https://docs.hetzner.com/storage/object-storage/supported-actions/)). Littlebox must treat them as unavailable until a capability test confirms their semantics. |
| Shared compute | Workers runs the Kernel and can serve Static Assets. D1 offers SQLite SQL semantics, recovery and access through a Worker or HTTP, but it is a Cloudflare-specific option ([D1](https://developers.cloudflare.com/d1/)). | Serverless Containers runs any HTTP image, scales to zero and allows between 0 and 50 replicas depending on configuration ([Serverless Containers API](https://www.scaleway.com/en/developers/api/serverless-containers)). | The documented portable path is a Cloud Server, optionally with a Load Balancer and a Volume. A Volume can only be attached to one server at a time ([Cloud Servers](https://docs.hetzner.com/cloud/servers/overview/), [Volumes](https://docs.hetzner.com/cloud/volumes/overview/)). |
| Closest managed database | D1 uses the SQLite engine and most of its SQL conventions ([D1 statements](https://developers.cloudflare.com/d1/sql-api/sql-statements/)). | Serverless SQL Database is PostgreSQL, not SQLite, and can go idle after five minutes without queries ([Serverless SQL overview](https://www.scaleway.com/en/docs/serverless-sql-databases/reference-content/serverless-sql-databases-overview/)). | No SQLite-compatible managed service was found in the official catalog consulted. The verifiable option is to run SQLite on a server and its disk or Volume. This conclusion is an inference from the catalog, not a guarantee about future products. |
| Credentials | R2 issues S3 keys. Object read and write tokens can be scoped to specific buckets, but they do not manage buckets or their configuration ([R2 authentication](https://developers.cloudflare.com/r2/api/tokens/)). | API keys belong to an IAM user or application, inherit its policies and set a preferred project for S3 ([API keys and Object Storage](https://www.scaleway.com/en/docs/iam/api-cli/using-api-key-object-storage/)). | S3 keys are valid by default for every bucket in the project. A bucket policy can block the rest and allow one specific key ([S3 credentials](https://docs.hetzner.com/storage/object-storage/faq/s3-credentials/)). |

The public documentation consulted does not present a Hetzner serverless service equivalent to Workers or Serverless Containers. That is why the table limits the claim to the path Hetzner does document, a shared VM. The adapter must not encode that difference in the generated app.

## Contracts Littlebox must own

The object storage contract should only allow operations that exist on all three providers:

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

`ETag` must be treated as an opaque token. It must not be used as a content hash, especially with multipart uploads. A function such as `putIfMatch` belongs to an extension checked at connection time, not to the minimal contract.

Publishing is a separate contract:

```ts
interface StaticPublisher {
  publish(input: {
    artifactPrefix: string;
    entrypoint: string;
    spaFallback: boolean;
  }): Promise<{ originUrl: string }>;
}
```

The initial implementations can be:

- `CloudflareStaticAssetsPublisher` or `R2CustomDomainPublisher`.
- `ScalewayBucketWebsitePublisher`, with Edge Services as an option.
- `ReverseProxyPublisher`, valid for Hetzner and any generic S3.

The last one can live next to the Kernel. It resolves `/apps/{deployment}/...`, reads private objects and applies `Cache-Control`, SPA fallback and HTTPS in a single place. It is the only uniform behavior that does not require credentials for each provider's control API.

## SQLite stored as objects

Do not treat a SQLite file inside S3 as if the engine could open it there. Hetzner warns that its Object Storage does not replace a database and is not meant for frequent reads or writes of small files ([Hetzner FAQ](https://docs.hetzner.com/storage/object-storage/faq/general/)). The same mechanical limit exists in any object store, because SQLite needs random access and file locking, while S3 replaces whole objects.

For small databases, the Kernel can keep a local working copy and persist immutable generations:

```text
littlebox/
  apps/{appId}/deployments/{deployId}/artifact/...
  databases/{databaseId}/generations/{generationId}.sqlite
  databases/{databaseId}/head.json
  blobs/{databaseId}/...
```

A mutation would follow this sequence:

1. The router sends all writes for a `databaseId` to the same coordinator.
2. The coordinator runs a declared action inside a local SQLite transaction.
3. It uploads a new generation with a key that is never reused.
4. It updates `head.json` and keeps the previous generation for rollback.

`If-Match` can protect step 4 on R2 and Scaleway. It cannot be the only exclusion mechanism if Hetzner is part of the contract. The first version must use a single writer per database. A version with several replicas will need a `WriteCoordinator` with fencing, implemented through a control database, one actor per key or a consensus service. Storing the generations in S3 stays portable even if the coordinator changes.

Uploading the full SQLite file after every mutation limits database size and write frequency. The initial version must enforce measured quotas, for example a maximum database size, bytes persisted per minute and a single active mutation. Later, an append-only journal can group snapshots, but recovery, compaction and checksums must be defined before it replaces this model.

## What S3 credentials allow

A neutral deployment endpoint can accept this object:

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

With those credentials it can verify access, upload artifacts and keep data private. It cannot assume that it can:

- create the bucket;
- enable website hosting;
- create a domain or certificate;
- configure a CDN;
- deploy Workers, Functions, Containers or servers;
- create a new credential with fewer permissions.

R2 separates object tokens from bucket administration. Scaleway uses IAM and its own control API. Hetzner creates S3 credentials from the Console and opens them to the whole project by default. An onboarding flow that only asks for S3 must start from an existing bucket and leave advanced publishing as manual configuration or as an optional provider adapter.

The backend receives the secret once, tests the capabilities and encrypts it if the Kernel will read the database after deployment. Deleting it after uploading the static files only works for an app without data. The public bundle never receives these keys. The browser compiles, but it sends the artifact to the administrative API for deployment.

## Portability traps

- Do not mix the public artifact and the private database in a bucket that becomes public. Scaleway's Bucket Website makes every object public by default. Using two buckets is the simple rule, one for apps and a private one for databases and blobs.
- Do not model `Website`, ACLs, domains, CDN, WAF or lifecycle as part of S3. R2 does not implement ACLs or website metadata in its S3 API, and Hetzner does not implement Website or bucket domains ([R2 S3 compatibility](https://developers.cloudflare.com/r2/api/s3/api/), [Hetzner actions](https://docs.hetzner.com/storage/object-storage/supported-actions/)).
- Do not derive security from the bucket name or prefix. The Kernel resolves the deployment and only then hands an internal key to the adapter.
- Do not include `databaseId`, SQL or credentials in the bundle. The client calls an action by identifier, and the Kernel validates the actor, input, parameterized statement, output and quotas.
- Do not assume the same value for `region` or the same endpoint shape. R2 uses `auto` in its S3 API examples, Scaleway sets a preferred project for each API key, and Hetzner uses regions such as `fsn1` and virtual-host addressing.
- Do not sign long-lived S3 requests in the browser. Presigned URLs are for a single specific operation. Administrative deploys and database reads go through the Kernel.
- Do not use the bucket's public hostname as the app's stable identity. The manifest must record its own `deploymentId`, and the publisher returns a replaceable URL.
- Do not take for granted that the cache reflects the object store. R2 documents that a cached domain can keep serving a deleted or overwritten version until it is purged or the TTL expires ([R2 consistency and cache](https://developers.cloudflare.com/r2/reference/consistency/)). Artifacts must use hashed paths, and Littlebox only changes the deployment pointer.

## First implementation

The combination with the fewest provider-specific dependencies would be:

1. WASM build in the browser.
2. A Kernel written against the Fetch `Request` and `Response`. The package offers a Worker entry for Cloudflare and an OCI image for Scaleway Serverless Containers or a Hetzner VM.
3. An `S3ObjectStore` configurable by endpoint, region, bucket and prefix.
4. A `ReverseProxyPublisher` inside the Kernel as the universal publishing path.
5. Two buckets or two credentials, with public artifacts served through the proxy and private data kept apart.
6. Local SQLite in the Kernel, one write queue per database and immutable snapshots in S3.
7. Actions declared by string. The app never sends SQL, and the public manifest only contains the identifiers and types the SDK needs.
8. Contract tests against R2, Scaleway and Hetzner. The tests determine capabilities such as conditional `PUT`, CORS, multipart, metadata and observed consistency.

Cloudflare is still a good first host for the Kernel, but D1 and Workers Static Assets must sit behind optional adapters. Scaleway allows a more portable version of the Kernel with an OCI image and a fairly complete S3. Hetzner also fits through object storage and a VM, but it needs a proxy/CDN for polished web publishing and in-house operation of the shared process.
