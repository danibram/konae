# Give the app an address

A Deployment always answers on the hostname the Kernel generated for it. It can also answer on one hostname you control, and on a short path of the installation's own domain.

## The three addresses

| Address | Looks like | Needs |
| --- | --- | --- |
| Generated hostname | `notes-1a2b3c.apps.example.com` | Nothing. Every Deployment has one, and republishing keeps it. |
| Custom domain | `notes.example.org` | DNS you control, and a proxy that terminates TLS for the name. |
| Path address | `example.com/p/notes` | An installation with `KONAE_DOMAIN` set. No DNS of your own. |

The generated hostname keeps answering when you add either of the others.

## Add a custom domain

1. Point the hostname at the installation's proxy with a CNAME or an A record. Konae issues no certificate, so the proxy terminates TLS for the name.
2. Ask for the record that proves you control the name.

    ```sh
    bun run konae -- domain --dir ./my-app --deployment <id> --set notes.example.org
    ```

    `--dir` is the folder whose `.konae/credentials.json` holds your login. Set `KONAE_MCP_TOKEN` to skip it. The command prints a TXT record named `_konae.notes.example.org`. Over MCP the same step is `domain_set`.
3. Add the TXT record at your DNS provider and run the same command again. The Kernel looks the record up itself, and the answer changes from `pending` to `active`.

On the custom domain the app has its own origin. A session made there stays there, and one made on the generated hostname stays on the generated hostname. Email links use the custom domain once it serves. `--clear`, or `domain_clear`, removes it.

## Turn on a path address

A path address is a short link on the installation's domain, for when DNS is not yours. It needs `KONAE_DOMAIN`. To try it locally, use the Compose stack, which sets `KONAE_DOMAIN=localhost`, so paths answer at `http://localhost:8080/p/<name>`.

1. Call `path_set` with `enabled: true` and, if you want one, a `name` of 3 to 40 letters, digits and hyphens. The app then answers at `<domain>/p/<id>` and at `<domain>/p/<name>`.
2. Read it back with `path_read`, which lists the addresses.

The app whose path holds the name `konae-home` also answers `<domain>/`. Every path and the installation's home share one origin. Any other app on that domain, including other owners' apps, can use the sessions of this app's users. An app with accounts therefore needs `acceptSharedOrigin: true` before its path turns on, and without it the call refuses with `path.shared-origin`. An app has accounts when it is private, when its signup is not Closed, or when one of its actions has an access other than `anonymous`. For an app with private data, prefer its own hostname.

Write the app's links and file references as relative paths, such as `about.html` and not `/about.html`. A root-relative URL leaves the `/p/<name>` prefix, and the build warns about each one.

## Check it worked

- `domain_read` shows the custom domain's `status`, and `path_read` shows `enabled` and the `addresses`.
- Open each address and call one Server Action from the page.

## Read next

- [The MCP guide on custom domains](../mcp.md#custom-domain) and [on path addresses](../mcp.md#path-address), with every refusal
- [Accounts and access](../accounts-and-access.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
