# Give the app an address

A Deployment always answers on the hostname the Kernel generated for it. It can also answer on one hostname you control, and on a short path of the installation's own domain.

## The three addresses

| Address | Looks like | Needs |
| --- | --- | --- |
| Generated hostname | `notes.apps.example.com` (or `notes-1a2b3c.…` if that slug is taken) | Nothing. Every Deployment has one, and republishing keeps it. `hostname_set` moves it to another free name under the suffix. |
| Custom domain | `notes.example.org` | DNS you control, and a proxy that terminates TLS for the name. |
| Path address | `example.com/p/notes` | An installation with `KONAE_DOMAIN` set. No DNS of your own. |

The generated hostname keeps answering when you add either of the others.

## The generated hostname

Konae names every app under the installation's app suffix, `KONAE_HOST_SUFFIX`. The first publish takes `<slug>.<suffix>` when no other app holds it, and `<slug>-<code>.<suffix>` otherwise, with a code from the app's ID. Every later release keeps that name. `release_plan` answers it as `hostname` and `url` before the first release, with `firstRelease: true`, and `konae publish` prints it then. Read it before you register Google sign-in or share links.

A name under the suffix is never a custom domain. `domain_set`, and `konae domain --set`, refuse it with:

```
notes.apps.example.com is under this installation's app suffix apps.example.com, where Konae names apps itself. Move the app's generated hostname there with hostname_set, or name a hostname outside the suffix.
```

The name belongs to the installation, and the app folder does not carry it. An export leaves it behind, and an import on another installation gets a name under that installation's suffix by the same rule.

## Move the generated hostname

```sh
bun run konae -- hostname --dir ./my-app --deployment <id> --set notes
```

Over MCP the same step is `hostname_set` with the `deploymentId` and the new `name`, one label such as `notes`, and it needs `publish`. Without `--set` the command prints the current address. The app answers on `notes.<suffix>` at once, and `project_inspect` shows the new `url`.

The old name stays this app's. Every request on it gets a 308 redirect to the same path and query on the new name, so links in email already sent and bookmarks keep working, and no other app can take the name until this app is deleted. Moving back to an old name takes it off the redirect list.

After a move, update what holds the old name:

- Google sign-in sends the callback of the name the request arrived on, and Google refuses one it does not list. Add `https://<name>.<suffix>/_konae/auth/google/callback` to the app's Google client before the move. The answer names it as `googleCallback`.
- The new name is a new origin. App users sign in again there.
- A service that calls the app's HTTP routes, an API key client or an MCP client may not follow the redirect. Give it the new address.
- A custom domain and a path address do not change.

The Kernel refuses a name another app holds now, held before, or is taking in a first publish, the installation's own hosts, and a name outside the suffix. [The MCP guide](../mcp.md#generated-hostname) lists each refusal. For a hostname outside the suffix, add a custom domain instead.

## Add a custom domain

1. Point the hostname at the installation's proxy with a CNAME or an A record. The Kernel issues no certificate, so the proxy terminates TLS for the name.
2. Ask for the record that proves you control the name.

    ```sh
    bun run konae -- domain --dir ./my-app --deployment <id> --set notes.example.org
    ```

    `--dir` is the folder whose `.konae/credentials.json` holds your login, or set `KONAE_MCP_TOKEN` instead. The Deployment's ID is the `id` in that folder's `konae.json`, and `project_inspect` shows it too. The command prints a TXT record named `_konae.notes.example.org`. Over MCP the same step is `domain_set`.
3. Add the TXT record at your DNS provider and run the same command again. The Kernel looks the record up itself, and the answer changes from `pending` to `active`.

On the custom domain the app has its own origin. A session made there stays there, and one made on the generated hostname stays on the generated hostname. Email links use the custom domain once it serves. `--clear`, or `domain_clear`, removes it.

## Turn on a path address

A path address is a short link on the installation's domain, for when DNS is not yours. It needs `KONAE_DOMAIN`. To try it locally, use the Compose stack, which sets `KONAE_DOMAIN=localhost`, so paths answer at `http://localhost:8080/p/<name>`.

1. Call `path_set` with `enabled: true` and, if you want one, a `name` of 3 to 40 letters, digits and hyphens. The app then answers at `<domain>/p/<id>` and at `<domain>/p/<name>`.
2. Read it back with `path_read`, which lists the addresses.

The app whose path holds the name `konae-home` also answers `<domain>/`. Every path and the installation's home share one origin. Any other app on that domain, including other owners' apps, can use the sessions of this app's users. An app with accounts therefore needs `acceptSharedOrigin: true` before its path turns on, and without it the call refuses with `path.shared-origin`. An app has accounts when it is private, when its signup is not Closed, when it declares `fileAccess: "app-users"`, or when one of its actions has an access other than `anonymous`. For an app with private data, prefer its own hostname.

Write the app's links and file references as relative paths, such as `about.html` and not `/about.html`. A root-relative URL leaves the `/p/<name>` prefix, and the build warns about each one.

## Check it worked

- `domain_read` shows the custom domain's `status`, `pending` or `active`. `path_read` shows `enabled` and the `addresses`.
- Open each address and call one Server Action from the page.

## Read next

- [The MCP guide on custom domains](../mcp.md#custom-domain) and [on path addresses](../mcp.md#path-address), with every refusal
- [Accounts and access](../accounts-and-access.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
