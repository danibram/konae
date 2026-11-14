# Addresses

A Deployment always answers on its generated hostname, and it can also answer on a hostname the owner controls and on a path of the installation's domain.

## Custom domain

A Deployment can answer on one hostname its owner controls, such as `notes.example.org`, next to its generated hostname, which keeps answering ([ADR-0014](../adr/0014-an-app-answers-on-its-hostname-a-custom-domain-or-a-path.md)).

1. Point the hostname at the installation's proxy, with a CNAME or an A record. The proxy terminates TLS for it and forwards `Host` unchanged. Konae issues no certificate.
2. Call `domain_set { deploymentId, hostname }`, which needs `publish`. It answers with the record to add:

   ```json
   { "deploymentId": "…", "hostname": "notes.example.org", "status": "pending",
     "url": "https://notes.example.org", "generatedUrl": "https://notes.example.com",
     "record": { "type": "TXT", "name": "_konae.notes.example.org", "value": "konae-verify-3f0c…" },
     "dns": "No TXT record at _konae.notes.example.org yet.",
     "next": "Add a TXT record _konae.notes.example.org holding konae-verify-3f0c…, then call domain_set again with notes.example.org." }
   ```

3. Add the TXT record and call `domain_set` again with the same hostname. The Kernel looks the record up over UDP, from `KONAE_DNS_RESOLVER` or the first `nameserver` in `/etc/resolv.conf`. When one of its values is the token, the answer is `status: "active"` and the alias serves.

`bun run konae -- domain --deployment <id> --set <hostname>` runs steps 2 and 3. On the alias the app has its own origin. Cookies are host-only, so a session made there stays there, and `'self'` in the Content Security Policy is the alias. Account email links use the alias once it serves. `domain_read { deploymentId }` returns the state and the installation's `rules`, and `domain_clear { deploymentId }` removes the alias and its token. An app in maintenance refuses both changes. The Kernel checks DNS only when `domain_set` runs.

| Refusal | Message |
| --- | --- |
| A name that is not a DNS name, such as a URL, a port, `_` or `*` | `<name> is not a valid DNS name. Use labels of letters, digits and hyphens, 1 to 63 characters each, that do not start or end with a hyphen.` |
| One label, an IP address, a local name | `<name> needs at least two labels, such as notes.example.org.` / `<name> is an IP address. Name a hostname under a domain you control.` / `<name> is a local name. …` |
| A name under the installation's suffix | `<name> is under this installation's app suffix <suffix>, where Konae names apps itself. Name a hostname outside it.` |
| The editor's or the MCP host | `<name> is this installation's own host. Name a hostname no Konae host uses.` |
| A TXT record that holds another value | `The TXT record at _konae.<name> holds "<value>", not this Deployment's token. Set it to <token> and call domain_set again.` |
| A name another Deployment serves | `<name> already serves another Deployment on this installation. Clear it there with domain_clear first.` |

An export carries the hostname without its token. The import creates a pending claim with a token the destination issues, and `domain_read` there prints the record to add. `kernel-rs/kernel/src/domain-cases.json` lists every rule.

## Generated hostname

The first publish names a Deployment `<slug>.<suffix>`, or `<slug>-<code>.<suffix>` when another app holds the slug or the name is one of the installation's own hosts, such as `app.<domain>`, and later releases keep that name. `hostname_set { deploymentId, name }` moves it to another name under the same suffix and needs `publish`. `name` is one label, such as `notes`, or the whole hostname.

```json
{ "deploymentId": "…", "hostname": "notes.example.com", "url": "https://notes.example.com", "changed": true,
  "previousHostnames": ["notes-1a2b3c.example.com"],
  "googleCallback": "https://notes.example.com/_konae/auth/google/callback" }
```

The names the Deployment held before answer every request with a 308 redirect to the same path and query on the new name, and no other app can take them. Email links already sent and bookmarks keep working, because the browser keeps the fragment through the redirect. The new name is a new origin. App users sign in again there, and an app with Google sign-in needs `googleCallback` registered in its Google client before the button works on the new name. A service that calls the app's HTTP routes, an API key client or an MCP client may not follow the redirect, so give it the new address. A custom domain and a path address do not change. `project_inspect` shows the new `url`. Moving back to a previous name takes it off the redirect list, and deleting the app frees its previous names.

| Refusal | Message |
| --- | --- |
| No name | `Name the new hostname, such as notes or notes.<suffix>.` |
| More than one label, or a name outside the suffix | `<name> is not one name under this installation's app suffix <suffix>. Name one label, such as notes, or add a hostname outside the suffix with domain_set.` |
| A label with other characters, or that starts or ends with a hyphen | `<label> is not a valid name. Use letters, digits and hyphens, 1 to 63 characters, that do not start or end with a hyphen.` |
| A name another app holds now or held before | `<name> is another app's address on this installation. Choose another name.` |
| The editor's or the MCP host | `<name> is this installation's own host. Choose another name.` |

The names live in `apps/<id>/.konae/hostname.json`, which copies carry and an export does not, like the generated hostname itself.

## Path address

A Deployment can answer at `<domain>/p/<id>`, and at `<domain>/p/<name>` once named, with no DNS of its own. `KONAE_DOMAIN` names the domain.

1. `path_set { deploymentId, enabled: true, name?, acceptSharedOrigin? }` turns the path on and needs `publish`. A name is 3 to 40 letters, digits and hyphens, does not start or end with a hyphen, is not a word Konae uses such as `app` or `mcp`, and does not look like an ID. No other Deployment may hold it while its path is on.
2. An app with accounts needs `acceptSharedOrigin: true`, or the call refuses with `path.shared-origin`. An app has accounts when it is private, when its signup is not Closed, when it declares `fileAccess: "app-users"`, or when an action is not `anonymous`. Every path address shares the origin `<domain>`, so any other app there can use the sessions of this app's users.
3. `path_read { deploymentId }` returns `enabled`, `name`, `acceptSharedOrigin` and the `addresses`. Read it back after each change.
4. `path_set` with `enabled: false` turns the path off and frees the name. `path_clear` also removes the name and the acceptance.

The app whose path holds the name `konae-home` also answers `<domain>/`. An omitted `name` or `acceptSharedOrigin` keeps the stored value, and `name: null` removes the name. Without `KONAE_DOMAIN`, `path_read` answers `domainConfigured: false` and `path_set` with `enabled: true` refuses. An export carries `enabled`, `name` and `acceptSharedOrigin`. On import a name another app holds is dropped, and the receipt's `notices` says so. `kernel-rs/kernel/src/path-cases.json` and `kernel-rs/kernel/src/path-state-cases.json` list the rules.

## Read next

- [Give the app an address](../guides/give-the-app-an-address.md)
- [Take the app elsewhere](../guides/take-the-app-elsewhere.md)
- [Accounts and access](../accounts-and-access.md)
