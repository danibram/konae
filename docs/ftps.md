# Receive files over FTPS

Konae runs one explicit FTPS listener for an installation. An app declares its upload destinations in `app/ftps.json`. The operator configures the listener once in `$KONAE_DATA_DIR/ftps.json`.

FTPS accepts uploads only. It does not list, download, append, rename or delete files. The listener requires TLS for the control and data connections.

## Declare destinations

Add `app/ftps.json` to the app folder. A destination can accept uploads from a signed-in app user or from a device credential issued by the owner.

```json
{
  "version": 1,
  "destinations": {
    "photos": { "kind": "member" },
    "meters": { "kind": "device", "folder": "readings" }
  }
}
```

A device `folder` is an optional normalized relative path. It cannot start with `/`, contain `\\`, empty, `.` or `..` path components, or contain control characters. The server combines it with the uploaded relative path beneath that destination.

## Choose who can upload

| Declaration | Who can upload | What happens |
| --- | --- | --- |
| `{"kind":"member"}` | A signed-in app user with an active session | The app's session issues a credential that expires after 10 minutes. It accepts one confirmed upload. The upload goes into Files and remains private to the uploading owner, even when `files.fileAccess` is `app-users`. |
| `{"kind":"device"}` | A device or team that holds the owner's credential | The owner issues a permanent credential for this one destination. It accepts multiple uploads and has no human actor. Konae records each upload as durable Intake data internally. The app does not need the `file-intake` or `actions` module for this destination. |

Turn on `auth` and `files` for member destinations. A member credential is temporary and belongs to one upload. The app's runtime MCP provides `ftps_upload_create { destination }` and `ftps_upload_status { uploadId }`. The create response contains `credentialId`, `host`, `port`, `username`, `password`, optional `expiresAt` and optional `uploadId`. The status response never contains the password.

An owner with `manage_users` can call `ftps_credentials_list { projectId }`, `ftps_device_create { projectId, destination, label }`, `ftps_device_rotate { projectId, credentialId }` and `ftps_credential_revoke { projectId, credentialId }`. Create and rotate return a password once. List, revoke and upload status never return one. Read `ftps_credentials_list` after an owner-side change to confirm it. Read `ftps_upload_status` after a member upload to confirm its result.

Konae stores salted password hashes in the private `data/secrets.sqlite` database, outside the app package. Audit records and conversation history must not contain passwords. The agent must not ask for a password or store one in chat.

## Configure the installation

Create `$KONAE_DATA_DIR/ftps.json` with mode `0600`. Store the certificate and private key under the installation data directory, outside app folders. Keep the private key readable only by the installation owner.

The installation file configures the listener bind address, passive port range, certificate, private key and upload limits. `advertisedHost` and `advertisedPort` are optional. Set them when clients connect through a proxy, NAT or a public port that differs from the listener's bind address. The listener uses explicit TLS. Clients must issue `AUTH TLS` for control and `PROT P` for file data.

```json
{
  "bind": "0.0.0.0:2121",
  "passivePorts": [31000, 31010],
  "certificate": "certs/ftps.pem",
  "privateKey": "certs/ftps-key.pem",
  "maxUploadBytes": 16777216,
  "advertisedHost": "ftps.example.com",
  "advertisedPort": 2121
}
```

Open the control port and the full passive port range in the host firewall and any router. If the service runs in a container, publish those ports to the Kernel container. The operator owns TLS certificates, port exposure, listener limits and any advertised address. App owners manage destination declarations and device credentials through the app tools.

Changing installation listener settings requires a Kernel restart. Destination and credential changes take effect on the running listener without a restart or app publish. Rotating a device credential invalidates its previous password. Revoking it stops new logins.

## Connect and upload

Use an FTPS client that trusts the configured certificate. Do not put the password in a shell command or agent prompt. The username format is `k1.<appKey>.<credentialId>`.

```sh
curl --fail --ssl-reqd --user 'k1.<appKey>.<credentialId>' \
  --upload-file sample.txt ftp://<host>:<port>/sample.txt
```

Let the client prompt for the password. For a member credential, the app must report a confirmed upload before the credential is considered spent. A device credential can send multiple files to its bound destination.

The listener stages bytes in an owner-only installation directory and removes incomplete uploads. It confirms an upload only after the destination accepts it. SQLite records, blobs and `index.json` do not share one commit. Check the upload status or the app's authorized read path after an interrupted client request.

## Move an app

The destination declaration travels with the app folder. Use `konae pull` to bring the published app declaration into a local folder, then use `konae export` or the app-folder export to carry it to another installation. The receiving operator configures that installation's listener and TLS settings. Device credentials remain in the installation's private secrets database. Issue new credentials there, or rotate them when a team or device changes.

This declaration does not require publish for a credential rotation. App data and rules remain in the app folder; listener addresses, TLS files and credential hashes remain installation data.

## Legacy intake credentials

Installations that already use the legacy `credentials` entries in `$KONAE_DATA_DIR/ftps.json` keep working. Those entries bind a username and password to a published `file-intake` source. Keep them as a migration path while replacing each binding with a destination declaration and an owner-issued device credential. The legacy format does not provide the new app-scoped lifecycle or one-time password handling.

Functions do not process FTPS bytes. Use the destination's supported app data path; do not declare or assume an FTPS Function consumer.
