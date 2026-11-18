# Installation copy format v3

The installation worker can write an experimental block repository from the same staged cut as a directory v1 or S3 v2 copy. The Kernel still compacts its private SQLite images with `VACUUM INTO` before publication, including identity and secrets. An internal backup payload or a file in an app's sources keeps its original bytes.

## Run the candidate

Generate a separate random 128-bit ID for each new repository, for example with `openssl rand -hex 16`. Keep each ID with the repository when moving it. In an isolated installation, set either or both destinations.

| Setting | Value |
| --- | --- |
| `KONAE_COPY_V3_DIR` | An absolute directory ending in `copies-v3/<id>`, outside both the live data tree and the legacy copy tree |
| `KONAE_COPY_V3_S3` | A second ID. The writer uses `copies-v3/<id>/` in the configured copy bucket |
| `KONAE_COPY_SECRET` | The existing installation copy secret, at least 32 bytes. Keep it stable for the revocation record |
| `KONAE_COPY_V3_SECRET` | The active data secret, at least 32 bytes. It defaults to the installation copy secret |
| `KONAE_COPY_V3_PREVIOUS_SECRETS` | A JSON array of earlier data secrets. Supply these from custody to read points made before a rotation |

The ordinary `KONAE_COPY_DIR` and `KONAE_COPY_S3` settings still select the original writers. Setting the v3 destinations alongside them writes all configured targets from one capture, including its pending-operation check and blob verification. Each target reports its own result under `/_konae/health`. The v3 names are `directory-v3` and `s3-v3`. After publication the worker prunes expired points and collects unreachable blocks. [Pruning and recovery permissions](../recovery-drill.md#pruning-v3-copies)

On a clean volume, set `KONAE_DATA_DIR`, supply the keys, and select the repository root and the point. For S3 also supply the normal copy bucket connection settings. Both operator binaries use the same reader.

```sh
konae restore --from "$KONAE_COPY_V3_DIR" --copy "$point" --require-revocation "$witness"
konae restore --from "s3://$KONAE_S3_BUCKET/copies-v3/$KONAE_COPY_V3_S3/" --copy "$point" --require-revocation "$witness"
```

The optional witness is the SHA-256 of a specific signed revocation entry in that repository, held independently after the cut. Restore verifies that the entry is present and that it changes the restored account or API key. Without a witness, restore checks the entries returned by the repository. A valid MAC on one entry cannot detect omission of a different entry.

## Objects and points

| Repository location | Encoding |
| --- | --- |
| `points/<UTC copy ID>` | Four ASCII bytes `KCP3`, a random 12-byte nonce, then AES-256-GCM ciphertext and its 16-byte tag. The whole envelope must fit in 4 MiB |
| `objects/<address>` | AES-256-GCM ciphertext and its 16-byte tag. The address contains 64 lowercase hexadecimal digits |
| `users/<UUID>` and `collectors/<UUID>` | S3 reservation keys. Their presence protects active operations until release |
| `revocations/started.json` and `revocations/<entry name>` | JSON with a hexadecimal HMAC. The MAC covers the repository ID, file name and record |

The manifest plaintext is UTF-8 JSON. It carries `version: 3`, `repository`, `copyId`, `createdAt` in Unix milliseconds, `pagesPerObject` and `files`. Each file supplies its relative `path`, byte `size`, hexadecimal plaintext `sha256`, nullable `pageSize` and an ordered `objects` array. The writer sorts files by path. A reference names the entire object in the same repository. Its array index determines its byte offset. Restoring a point requires its own manifest and those objects.

For SQLite, the writer reads bytes 16 and 17 of the header as a big-endian page size. The value 1 means 65,536. SQLite accepts powers of two from 512 through 65,536. The installation packs 16 consecutive pages per object. The format accepts 1 through 64 pages per object. Other files use 1 MiB objects and `pageSize: null`. The last object carries the remaining bytes. SQLite images must end on a page boundary. Empty ordinary files have zero references. Reordering references changes the reconstructed file hash.

A changed page replaces its containing object. Larger groups reduce HEADs and GETs and upload more data for scattered writes. The writer reads every staged byte on every run. S3 reuse checks the length and stored SHA-256 of the deterministic ciphertext with HEAD. When a provider omits the checksum, the writer compares the bounded GET body. New PUTs request SHA-256 validation and use `If-None-Match: *`. Directory publication syncs a temporary file and hard-links it under its final name. The writer compares stored block bytes and publishes the manifest last. A retry of the same point verifies an existing manifest. A conflicting manifest or block fails the attempt.

## Keys and rotation

The installation derives the format secret from the UTF-8 string `v3/<ledger key fingerprint>/<data secret>`. The fingerprint is the existing lowercase hex v1 key check. The standalone crate takes its format secret directly. HKDF-SHA256 extracts it with salt `konae-copy-v3`. Each expansion yields 32 bytes. Its info is the ASCII repository ID followed immediately by the literal label `address`, `block` or `point`.

- The address is HMAC-SHA256 of the plaintext under the address key. Equal plaintext can share an object within one repository and key epoch.
- The AES block key is HMAC-SHA256 under the block key material over the ASCII address. The nonce is twelve zero bytes. Each distinct plaintext gets its own AES key through its keyed address. Repeated plaintext encrypts to the same bytes. This construction relies on collision resistance of the full 256-bit address. The address is the associated data.
- The manifest AES key is HMAC-SHA256 under the point key material over the ASCII copy ID. Each seal draws a fresh nonce from the OS. The copy ID is the associated data. The reader also checks the encrypted repository ID and timestamp.
- The Kernel signs the v3 revocation marker and entries with the installation's existing revocation MAC key. The message is compact JSON for the array `["v3 record",repository,name,record]` with sorted object keys and the MAC field omitted. An entry copied between repositories fails verification.

Rotate the data secret in place. Keep the previous value in custody until every point that needs it has expired. If the first epoch used the default, its historical data secret is the original value of `KONAE_COPY_SECRET`. Supply old values through `KONAE_COPY_V3_PREVIOUS_SECRETS` while the current value remains active. New objects use the new epoch. Reading an old manifest selects its authenticating key for every referenced object. The stable revocation key verifies suspensions recorded after rotation on both destinations.

The collector authenticates every listed root before deleting expired manifests. Keep a historical key available until the last manifest from its epoch has been removed. An unreadable root stops pruning for that destination. Rotating the installation revocation secret requires a separate ledger transition. The v3 data rotation described here keeps that secret in service.

[Executable format vectors](../../kernel-rs/backup/src/blocks/tests.rs) · [Format measurements](../../kernel-rs/backup/block-measure.py) · [Installation and rotation trial](../../tests/smoke/block-copies.py)
