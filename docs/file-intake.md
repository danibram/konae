# File intake v1

File intake accepts bytes from a declared source and stores each accepted version in the app's shared blob store. It works with the Files, Functions and Calls modules turned off. It adds no `miniapp.files` SDK method.

## Declare a source and consumer

Add `file-intake.json` to the app folder and turn on the `file-intake` module in `konae.json` or with `module_set` before publishing.

```json
{
  "version": 1,
  "sources": [
    {
      "id": "camera",
      "version": 1,
      "replay": "delivery-key-or-current-path-sha256"
    }
  ],
  "consumers": [
    {
      "id": "record-photo",
      "source": "camera",
      "action": "photos.accept",
      "input": {}
    }
  ]
}
```

Each consumer names an existing Actions mutation. Its fixed `input` object may set only fields that the action declares. At run time, the Kernel adds `versionId`, `sourceId`, `path`, `sha256` and `size` to those inputs. The action cannot read the blob or the host filesystem. It can update app rows through its declared SQL statement.

Source and consumer IDs use ASCII letters, digits, `_` and `-`. Source version 1 is the only supported declaration version. Each source supports the `delivery-key-or-current-path-sha256` replay rule.

## Accept bytes

An authenticated local operator can accept a completed file in test mode:

```sh
bun run konae -- test --dir ./my-app intake camera --path incoming/photo.jpg --file /tmp/photo.jpg --delivery-key camera-00042
```

The local command uses the same `accept_staged` operation that a trusted FTPS adapter calls. It runs under the local operator's OS account and requires a `konae dev` test Kernel. The adapter resolves the current Deployment, authenticates its credential, binds that credential to one app and declared source, and then passes the completed staged file, normalized relative path and optional delivery key. The operation rechecks the current Deployment and pinned source declaration after it acquires the app write lock.

With a delivery key, retries return the original version ID when source, key, path and SHA-256 match. Reusing a source key with another path or digest fails. Without a key, the Kernel compares the content hash with the current version at that source path. The same current bytes return the same version. Changed bytes create a new version. If an old payload arrives after a replacement, it becomes a new replacement version because the old hash is no longer current.

The Kernel stages each blob under the shared app quota. Under the app write lock it checks replay and quota, then commits the immutable version, source-scoped delivery identity, current path and pending consumer rows together. The command reports success only after that commit. A crash before the commit can leave an unreferenced staged blob, which package export and quota ignore.

Each version is a durable intake event. Consumer jobs store the version metadata, input and exact declared Actions mutation accepted with the event. The dispatcher runs one job per app turn. It commits the action, a unique `(version, consumer)` effect record and the completed job in one SQLite transaction. A retry after a crash either sees the complete effect or runs the rolled-back action again.

## Move and recover

The `file-intake` module registers its reserved app tables and its blob inventory. Package export includes the database records and private blob bytes through the existing database and files components. The inventory counts those bytes with Files under the same app quota, even when the Files module is off. Import restores the references and wakes pending jobs. Kernel startup also recovers leased jobs and wakes each served app.

The FTPS listener and credential binding belong to the separate adapter. File intake defines no transport or network listener.
