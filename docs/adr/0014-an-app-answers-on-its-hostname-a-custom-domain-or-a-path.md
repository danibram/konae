# An app answers on its hostname, a custom domain or a path

Status: accepted.

A shop needs its own domain. A person who shares a page wants `konae.es/p/<name>`, which needs no DNS. An app moved to another installation has to keep its name there. Every address must keep the app's sessions on the right origin, and the owner must hear what an address costs before choosing it.

## Decision

### One domain names an installation's hosts

`KONAE_DOMAIN=<domain>` derives the hosts:

| Host | Serves |
| --- | --- |
| `<domain>/` | The home |
| `<domain>/p/<id>` and `<domain>/p/<name>` | An app on its path address |
| `app.<domain>` | The editor and the project list |
| `app.<domain>/mcp` | Konae's MCP, OAuth and owner sign-in |
| `<slug>.<domain>`, or `<slug>-<id>.<domain>` when the slug is taken | An app on its generated hostname |
| A proved custom domain | An app on its alias |

The Kernel derives `KONAE_PUBLIC_ORIGIN` as `https://app.<domain>` and `KONAE_HOST_SUFFIX` as `<domain>`, and Konae takes the same origin for `KONAE_MCP_ORIGIN`. The scheme is `http` for `localhost` and `https` otherwise, and `KONAE_SCHEME` overrides it. Each variable still wins when it is set. The Kernel refuses to start when the editor or MCP origin is the bare domain, because the home and path addresses answer there.

The home is a public app. The Deployment whose path address holds the name `konae-home` answers `<domain>/` and every path on `<domain>` outside `/p/`. The operator claims that name with `path_set` like any other, so no setting and no restart choose the home.

### A Deployment may hold one custom domain

- `domain_read` needs `read`. `domain_set { deploymentId, hostname }` and `domain_clear` need `publish`, the permission that decides what a Deployment serves.
- A hostname is a DNS name with at least two labels, not an IP address, not `localhost`, not under the installation's suffix and not an editor or MCP host. `kernel-rs/kernel/src/domain-cases.json` holds every rule, and the Kernel and `src/lib/domain-name.ts` both run it.
- The first `domain_set` records a pending claim with a random token and answers with the TXT record `_konae.<hostname>` that must hold it. Each later `domain_set` looks the record up with the Kernel's own DNS query. The alias serves when one of the record's values is the token. A name another Deployment serves is refused, and a pending claim blocks nobody.
- A verified alias is the app's own origin, with no other change: the runtime endpoint's `Origin` check, host-only cookies, the policy's `'self'`, routes, the file path and the maintenance page. The generated hostname keeps answering, and a session made on one host is not sent to the other.
- Over HTTPS an alias gets HSTS without `includeSubDomains`. Email links use the alias once it serves. The Kernel issues no certificate. The operator's proxy terminates TLS and forwards `Host` unchanged.
- An export carries the hostname without its token. The destination records a pending claim with a token of its own, because whoever uploads an export writes its manifest.

### An app may answer on a path

- The owner turns the path on with `path_set { deploymentId, enabled, name?, acceptSharedOrigin? }`. The app answers on `/p/<deployment id>`, which never changes, and on `/p/<name>` when the owner names it. The id address keeps answering next to the name, so a shared link never breaks.
- A name is 3 to 40 lowercase letters, digits and hyphens, is not shaped like a UUID and is not reserved (`_konae`, `p`, `app`, `mcp`, `www`, `api`, `admin`, `runtime.js`). `kernel-rs/kernel/src/path-cases.json` holds the rules for both doors.
- The router strips `/p/<x>` before the request reaches the app, so the runtime endpoint, files, `/runtime.js` and routes answer under the prefix unchanged. The SDK reads its base from `location.pathname`, so one artifact answers on a hostname, an alias and a path. The Gate and the release warn about a page that names a root-relative URL, because it leaves the prefix.
- An export carries whether the path is on and its name, never the id. On import the app gets its own `/p/<id>`, keeps the name when it is free there, and the import says so when it is not.

### Sharing an origin is the owner's choice, said before it happens

Every path address and the home share the origin `<domain>`. Any app's script there can open another app's page with `window.open` and read its DOM, read its `localStorage`, and call its runtime. A session cookie on a path address is named for the Deployment, which keeps two apps from overwriting each other's session by mistake, not against a hostile app.

So `path_set` refuses an app that has accounts, or a private app, without `acceptSharedOrigin: true`, with a code whose message names the risk, and the editor shows the same words first. A public app with Closed signup and only `anonymous` actions needs no acknowledgement, because no session exists to share. The app keeps its generated hostname and alias, where its sessions stay on their own origin.

## Rejected options

- **A field on the Deployment record for the alias.** The alias has its own lifecycle, and a file in the app's `.konae/` travels with copies and restores.
- **Check DNS on each request or on a timer.** A visitor could make the Kernel query DNS at will. Once DNS points elsewhere, the Kernel no longer receives the alias's traffic.
- **Prove control with an HTTP file on the domain.** The operator's proxy would first have to route an unproved name to the Kernel.
- **Carry the token in an export.** A token copied from public DNS into a manifest would let one owner claim another's pending name.
- **Issue certificates in the Kernel.** TLS stays with the proxy, as for the generated hostnames.
- **Path addresses only for public apps without accounts.** The owner decides, and the risk is stated at both doors instead.
- **A redirect from `/p/<name>` to the hostname.** The address bar would show the hostname, which the path address exists to avoid.
- **Serve `/p/` on the editor's origin.** The owner's cookie would reach Konae's MCP from an app's request.
- **Rewrite root-relative URLs in served HTML.** A script builds URLs at run time, which no rewrite reaches. `<base href>` is out because the policy sends `base-uri 'none'`.
- **A separate `mcp.<domain>` by default.** It adds a host and a certificate and moves nothing out of reach. `KONAE_MCP_ORIGIN` still allows it.

## Consequences

- An app may have three kinds of address at once. App settings lists them, built from the domain the Kernel reports.
- A person signed in on the generated hostname signs in again on the alias.
- A name stays verified until the owner clears it, even after its TXT record is removed.
- An app with a Google client registers one callback per address (ADR-0003).
- [The architecture](../architecture.md#isolation-of-hosted-code) records that the isolation it describes holds for generated hostnames and aliases only.
