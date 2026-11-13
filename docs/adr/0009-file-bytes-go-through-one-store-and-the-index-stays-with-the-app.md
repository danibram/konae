# File bytes go through one store, and the index stays with the app

Status: accepted.

Apps take uploads: photos, attachments, avatars. Who may read a file is the app's decision (job 5), and the files must leave with the app (job 4). A practice app's photos can outgrow the Kernel's volume, and an S3-compatible bucket holds them without growing it, as long as apps, the SDK and an export see no difference.

## Decision

### A file belongs to its uploader and has a visibility

- `miniapp.files.upload` stores a file for a signed-in app user. By default a file is private to its uploader unless the upload makes it public. A visitor on a public app may list and read public files.
- An app can declare `manifest.fileAccess: "app-users"` to share all files with its signed-in users. The policy takes effect on existing IDs as well as new uploads. The Kernel applies it to lists, byte reads and visibility changes. File API keys cannot read these files.
- The uploader alone may change a file's visibility or delete it, with `files_update` and `files_delete`. A deleted file frees its share of the quota, and its ID answers `File not found.` through every door.
- A caller who may not read a file gets 404, as for a missing one. A visitor without a session on a private app gets 401 before any lookup.
- `miniapp.files.url` returns `/_konae/files/<id>`, which a page reads over its own origin with the session cookie. An API key may use the same path. It reads what a visitor reads, plus the visibilities it names (ADR-0013). There is no unauthenticated object URL.
- The file path answers with an ETag made of the file ID and its size, because the bytes of an ID never change, and with `Cache-Control: private, no-cache`. A browser may keep the bytes but asks before each use. The Kernel checks the caller against the index and answers 304 without reading the store, or 404 once the file is gone or the caller lost access. A refusal answers `private, no-store`, and a 304 counts no file read in Usage.
- The file path sends a JavaScript, HTML, XML or SVG type as `application/octet-stream`, so a file never runs as a script on the app's origin.
- Uploads and deletes are audited. A Deployment holds at most 10,000 files. Visitors share a transfer pool of 16 of their own, so visitors cannot hold every slot a signed-in user needs.

### Quotas are installation settings

`KONAE_FILES_QUOTA_BYTES` (all files of one Deployment, 20 MiB by default) and `KONAE_FILE_MAX_BYTES` (one file, 2 MiB by default) belong to the operator. Every refusal names the configured number, and the SDK contract states none.

### The bytes go through one file store

- `FileStore` in `kernel-rs/kernel/src/file_store.rs` has `put`, ranged `get`, `delete` and `list` for one Deployment. The local store keeps the bytes in the app's folder. The S3 store keeps them under `files/` in the installation's one private bucket, `KONAE_S3_BUCKET`, beside copies under `copies/`, with one key pair.
- The S3 store uses only `PutObject`, `GetObject` with a `Range`, `DeleteObject` and `ListObjectsV2`, the calls [the portability note](../research/provider-portability.md) finds on R2, Scaleway and Hetzner. No contract test has run against those three, so S3 file storage is not yet called supported there. Bytes still pass through the Kernel with their SHA-256. The browser SDK opens an upload with `transfer_begin`, sends the whole file in one `PUT /_konae/transfers/<id>` on the app's origin, which checks the size and SHA-256 as `transfer_complete` does, and stores it with `files_upload`. `files.upload` answers with the file's path and does not read the bytes back. No bucket URL reaches the browser.
- The index, `files/index.json` in the app's data, is the authority. It says which files exist, who uploaded each, its visibility and its size, so listing, reading rights and the quota never ask the store.
- An upload puts the object before the index names it, and a delete removes the index entry before the object. A crash leaves an object no index names, never an entry without bytes. `files-reconcile` removes objects no index names that are older than one hour.
- `files-store.json` records which store holds the bytes, and a Kernel whose settings name another store refuses to start. `files-migrate --to s3|local` copies every indexed file, reads each back and compares its SHA-256, records the new store, and only then removes the old bytes. It and `files-reconcile` run while the Kernel is stopped.
- An export reads each file from the store, and an import writes each through the destination's store, so an app moves between an S3 installation and a local one with the same bytes. A scheduled copy carries the bytes too.

## Rejected options

- **A public object URL for a file.** Any person with the link could read a private file.
- **Per-app bucket credentials.** They would not travel, and an export would not carry the files.
- **Presigned URLs for uploads and reads.** The browser would reach the bucket and skip the Kernel's checksum, quota and audit, and the page's policy refuses the other origin.
- **The index in the bucket.** Every list and quota check would call S3, and the app's folder would no longer say which files it has.
- **A cache lifetime on the file path (`max-age`).** A browser would go on showing a deleted file, or one its reader may no longer read, until the lifetime ran out.
- **A copy that points at the objects under `files/` instead of holding its own bytes.** A restore would depend on what the bucket holds at restore time. Files deleted after the copy would come back as entries without bytes, or files their uploaders deleted would come back.

## Consequences

- A Deployment is its folder plus, when the installation uses a bucket, its file bytes there.
- An export from an installation with raised limits can hold more files than a default one accepts. The import is refused with the destination's numbers, and its operator raises them.
- A browser upload takes four requests whatever the file's size. `files.list` returns metadata and a same-origin file URL without reading bytes. An export and a copy still move 64 KiB per MCP call, about 400 calls for 25 MiB.
- A device that opened a file keeps its bytes in the browser's cache until the browser evicts them, after its user signs out too. The Kernel answers 404, never 304, to a caller who may not read the file, so the page cannot show them, but the bytes stay on that disk.
- Copies read every file from the bucket each time. An incremental copy is later work.
- The editor preview refuses a file over 2 MiB, whatever the Kernel's setting.
