# A Deployment's file bytes may live outside its folder

Status: accepted, 2026-11-09. Amends the "Deployment" entry of `CONTEXT.md`, which said everything a Deployment is lives in its folder, and [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md) only in where `package_export` reads file bytes from. The package format does not change.

A small practice app runs one Deployment per practice, and its patient photos and attachments outgrow the 20 MiB a Deployment could hold. Raising the quota on the Kernel volume works until the volume is full. An S3-compatible bucket holds the bytes without growing the volume. The [plan](../plans/app-google-sign-in-and-s3-files.md) admits this as operator work on the installation: apps, the SDK and packages see no difference.

## Decision

- The file quota and the per-file cap are installation settings, `LITTLEBOX_FILES_QUOTA_BYTES` and `LITTLEBOX_FILE_MAX_BYTES`, with 20 MiB and 2 MiB as defaults. Every refusal names the configured number, and the SDK contract no longer states one.
- The bytes of app files go through one installation file store, `FileStore` in `kernel-rs/src/file_store.rs`, with `put`, ranged `get`, `delete` and `list` for one Deployment. `LocalFolder` keeps them as `deployments/<id>/files/<file id>.blob`, as before. `S3Store` keeps them as `<prefix>/<deployment id>/<file id>` in a bucket named by `LITTLEBOX_FILES_S3_*`. These settings are separate from `LITTLEBOX_S3_*`, so file bytes, which are private, can sit in another bucket with another key than the artifacts.
- The index, `deployments/<id>/files/index.json`, stays in the folder and is the authority. It says which files exist, who uploaded each, its visibility and its size, so listing, reading rights and the quota never ask the store. A store object that no index names is unreachable.
- `S3Store` uses only `PutObject`, `GetObject` with a `Range`, `DeleteObject` and `ListObjectsV2`, which [the portability note](../research/provider-portability.md) finds on R2, Scaleway and Hetzner. It uses no conditional write, multipart upload, ACL, lifecycle rule or presigned URL. Bytes still pass through the Kernel in 64 KiB MCP blocks with their SHA-256, and `GET /_littlebox/files/<id>` streams from the store; no bucket URL reaches the browser, which `connect-src 'self'` would refuse anyway.
- An upload puts the object under a new file ID before the index names it, and a delete removes the index entry before the object. A crash between the two leaves an object no index names, never an index entry without bytes. `littlebox-kernel files-reconcile` lists the store and removes objects no index names that are older than one hour, so an upload in flight keeps its object.
- `files-store.json` in the data directory records which store holds the bytes. A Kernel whose settings name another store refuses to start and says which command moves the bytes. `littlebox-kernel files-migrate --to s3|local` copies every indexed file of every Deployment into the other store, reads each one back and compares its SHA-256 with the source, records the new store, and only then removes the bytes from the old one. It runs with the Kernel stopped: a serving Kernel holds an exclusive lock on `kernel.lock` in the data directory, `files-migrate` and `files-reconcile` refuse while it is held, and they hold it while they run.
- A package export reads each file from the store into the tar, and an import writes each file through the destination's store. A package from an S3 installation imports into a local one and back with the same bytes.
- Deleting a project removes its Deployment's objects after its folder.

### A copy carries the bytes

A scheduled copy (`copies.rs`) could share the bucket, recording only the index, or carry the bytes. It carries them. The copy takes the index under the Deployment's locks, as before, then reads each indexed file from the store into the copy's `files/` folder after the locks are released, because the objects under a file ID never change. A file deleted between the two steps is left out of the copied index.

Sharing the bucket would make a restore depend on what the bucket holds at restore time. Files deleted after the copy would come back as index entries without bytes, or, if deletes were held back to prevent that, the restore would bring back files their uploaders deleted. Deleting a file is the uploader's decision, which job 5 keeps with the app. A shared bucket would also tie a copy to one installation's bucket, while a copy is how an installation moves.

The cost is reading every file from the bucket on every copy. At an hourly copy, a Deployment at 1 GiB reads 24 GiB a day from the bucket. Copies are already whole snapshots, so this follows the existing model; an incremental copy is a later change. A restore lands the bytes in the folder and leaves `files-store.json` out, so the restored installation uses its folder until `files-migrate --to s3` moves them.

## Consequences

- `CONTEXT.md` says a Deployment is its folder plus, when the installation uses a bucket, its file bytes there.
- A package from an installation with raised limits can hold more files than a default one accepts. The import is refused with the destination's numbers, and the destination's operator raises them. Job 4 still holds, because the refusal says what to change.
- A larger per-file cap means more 64 KiB MCP calls per file: about 400 calls for 25 MiB. The browser SDK holds the whole file in memory while it uploads or reads.
- `S3Store` is tested against SeaweedFS and an in-process fake that speaks the same HTTP. The contract tests against R2, Scaleway and Hetzner that the portability note asks for have not run, so S3 file storage is not yet called supported on those providers.

## Rejected alternatives

- **Per-app bucket credentials.** The credentials would not travel, a package would not carry the files, and the page policy keeps the page from reaching a bucket. The plan rejects it.
- **Presigned URLs for uploads and reads.** They would let a browser reach the bucket directly and bypass the Kernel's checksum, quota and audit, and the page policy refuses the other origin.
- **The index in the bucket.** Every list and quota check would call S3, and a Deployment's folder would no longer say which files it has.
