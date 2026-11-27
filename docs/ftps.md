# Receive files over FTPS

The Kernel can run one explicit FTPS listener for an installation. It accepts uploads only. Each username and password is bound to one served app and one intake source declared by that app. The source decides whether a retransmission is a duplicate or a new version.

## Bind a published source

Publish an app with [File intake](./file-intake.md) enabled and the source declared. Record the app ID from `project_inspect` or its `konae.json`. Use that ID and the exact source ID in the credential below.

The installation operator creates `<KONAE_DATA_DIR>/ftps.json` with mode `0600`. Store the certificate and private key inside the installation data directory, outside app folders. Keep the key readable only by the installation owner. Enter the password on the installation. The app's `file-intake` switch enables its source. The `ftps.json` file configures the listener.

```json
{
  "bind": "0.0.0.0:2121",
  "passivePorts": [31000, 31010],
  "certificate": "certs/ftps.pem",
  "privateKey": "certs/ftps-key.pem",
  "maxUploadBytes": 16777216,
  "credentials": [
    {
      "username": "meter-west",
      "password": "replace-with-a-long-random-secret",
      "appId": "app_meter",
      "sourceId": "meter-west"
    }
  ]
}
```

`passiveHost` is optional. Set it to the IPv4 address or DNS name clients must use when the Kernel sits behind NAT. Open the control port and every passive port in the configured range on the host firewall and any router. The listener uses explicit TLS. Clients must issue `AUTH TLS` for control and `PROT P` for file data. Plain control or data transfers are refused.

## Verify the listener

1. Restart the Kernel after changing the file. Under Compose, publish the control port and the full passive range on the Kernel container.
2. Run `kernel ftps status` with the same `KONAE_DATA_DIR` as the service. The command reads the saved bind address, passive ports, credential count and upload limit.
3. Upload a completed test file with a client that trusts the certificate. For example, use the command below and enter the password when prompted.
4. Expect FTP reply `226` after the upload. If the source has a consumer, verify the corresponding app rows with its authorized query.

```sh
curl --fail --ssl-reqd --user meter-west --upload-file sample.txt ftp://ftps.example.com:2121/sample.txt
```

Check the Kernel log for bind failures, TLS handshake failures, idle timeouts and transfer outcomes. If the status command reports the settings but the client cannot connect, check the exposed control and passive ports and the client's certificate trust.

An upload is first written to an owner-only installation staging directory. The Kernel removes the temporary file if the client aborts, the data connection fails, the input exceeds the byte limit, or Intake rejects the upload. The server acknowledges a transfer only after Intake commits the immutable version and its change event. A stalled transfer times out after 60 seconds without data, and a transfer has a one-hour total limit. At most 32 sessions run at once. Stale staging files are removed when the listener starts.

The listener does not list, download, append, delete or rename files. It does not create intake sources or change their delivery identity rules. Those remain part of the app declaration and Intake contract.
