# Receive files over FTPS

The Kernel can run one explicit FTPS listener for an installation. It accepts uploads only. Each username and password is bound to one served app and one intake source declared by that app. The source decides whether a retransmission is a duplicate or a new version.

The listener is off unless `<data directory>/ftps.json` exists. Keep that file mode `0600`. Store the certificate and private key inside the installation data directory and outside app folders. The key must be readable only by the installation owner.

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

Restart the Kernel after changing the file. `kernel ftps status` reads the saved settings and reports whether the listener is configured, its bind address, passive ports, credential count and upload limit. It does not probe the socket or certificate. The Kernel logs bind failures, TLS handshake failures, idle timeouts and transfer outcomes to its operator log. Client connection errors can also come from a closed firewall port, an unreachable passive port or a certificate the client does not trust.

An upload is first written to an owner-only installation staging directory. The Kernel removes the temporary file if the client aborts, the data connection fails, the input exceeds the byte limit, or Intake rejects the upload. The server acknowledges a transfer only after Intake commits the immutable version and its change event. A stalled transfer times out after 60 seconds without data, and a transfer has a one-hour total limit. At most 32 sessions run at once. Stale staging files are removed when the listener starts.

The listener does not list, download, append, delete or rename files. It does not create intake sources or change their delivery identity rules. Those remain part of the app declaration and Intake contract.
