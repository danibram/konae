# An installation names its hosts from one domain, and an app may answer on a path

Status: Accepted, 2026-11-10. Builds on [ADR-0015](0015-a-deployment-answers-on-one-custom-domain.md) for the alias rules and the package's handling of a name, and on [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md) for the isolation headers.

An operator sets three variables to place an installation, `KONAE_PUBLIC_ORIGIN`, `KONAE_MCP_ORIGIN` and `KONAE_HOST_SUFFIX`, and nothing serves the bare domain. A person who wants to share a page asks for `konae.es/p/<something>`, which reads better than `<slug>-<id>.konae.es` and needs no DNS. The Kernel answers only hostnames today, so every app address is a subdomain or a proved custom domain.

## Decision

### One domain names every host

`KONAE_DOMAIN=<domain>` derives the hosts:

| Host | Serves |
| --- | --- |
| `<domain>/` | The home, which says what Konae is |
| `<domain>/p/<id>` and `<domain>/p/<name>` | An app on its path address |
| `app.<domain>` | The dashboard and the editor |
| `app.<domain>/mcp` | MCP, OAuth and owner sign-in, as the editor host already routes them |
| `<slug>-<id>.<domain>` | An app on its generated hostname, unchanged |
| A proved custom domain | An app on its alias, unchanged ([ADR-0015](0015-a-deployment-answers-on-one-custom-domain.md)) |

From it the Kernel derives `KONAE_PUBLIC_ORIGIN` as `https://app.<domain>`, `KONAE_MCP_ORIGIN` as the same origin, and `KONAE_HOST_SUFFIX` as `<domain>`. The scheme is `http` when the domain is `localhost` and `https` otherwise, and `KONAE_SCHEME` overrides it. Each of the three variables still wins when it is set, so an installation that sets them keeps its hosts. The Kernel refuses to start when `KONAE_PUBLIC_ORIGIN` or `KONAE_MCP_ORIGIN` has the bare domain as its host, because the home and path addresses answer there. The local stack sets `KONAE_DOMAIN=localhost`, so the editor answers on `app.localhost`.

MCP stays on the editor host at `/mcp` because the gateway already sends `/mcp`, `/_konae`, `/oauth` and `/.well-known` from that host to the Kernel. A separate `mcp.<domain>` would add a host, a certificate and a server block and move nothing out of reach.

The home is a public Konae app. The Deployment whose path address holds the name `konae-home` answers `<domain>/` and every other path on `<domain>` outside `/p/`, under the same rules as a path address, and keeps answering on `/p/konae-home` too. The operator claims the name with `path_set` like any other, so no variable and no restart choose the home. With no app on that name, `<domain>/` answers not found.

### An app may answer on a path

The Project Owner decides whether an app answers on `<domain>/p/`. Turning it on gives it `/p/<deployment id>`, the UUID the Kernel already assigns, which never changes on republish and needs no claim. The owner may add a name, `/p/<name>`, that no other Deployment holds. The UUID keeps answering next to the name, as the generated hostname keeps answering next to an alias, so a shared link never breaks and a rename frees the old name without stranding anyone.

Three admin tools manage it: `path_read` with `read`, and `path_set { deploymentId, enabled, name?, acceptSharedOrigin? }` and `path_clear` with `publish`. A `null` name removes the name and leaves the id address answering. The state lives in `deployments/<id>/path.json` next to `domain.json`, and the Kernel keeps a name index in memory as it does for aliases.

A name is lowercased. It is 3 to 40 letters, digits and hyphens, does not start or end with a hyphen, does not have the shape of a UUID, so nobody can take another app's default address, and is not reserved: `_konae`, `p`, `app`, `mcp`, `www`, `api`, `admin`, `runtime.js`. `kernel-rs/src/path-cases.json` holds every rule with its message, and the Kernel and `src/lib/path-name.ts` both run it. `domain-cases.json` refuses `app.<domain>` and `mcp.<domain>` through its existing suffix rule.

`deployment_for` resolves, in order, the generated hostname, a proved alias, then on `<domain>` the prefix `/p/<id-or-name>/` and finally `/` for the home. The router strips the prefix before the request reaches the app routes, so `/_konae/mcp`, `/_konae/files/<id>`, `/runtime.js` and HTTP routes answer under `/p/<x>/` with no other change. `security.rs` chooses the app policy for a request on `<domain>` by its prefix.

### The SDK reads its prefix from the page

The runtime derives its base from `location.pathname`, so one Build Artifact answers on its hostname, its alias and its path. The Validation Gate and the release warn, with the same words, about a page that names a root-relative URL such as `href="/about"`, because it leaves the app's prefix on a path address. The Kernel does not rewrite HTML, and `<base href>` is not an option because the policy sends `base-uri 'none'`.

### Sharing an origin is the owner's choice, said before it happens

Every path address and the home share the origin `<domain>`. On that origin, any app's script can open another app's page with `window.open` and read its DOM, read its `localStorage`, and call its runtime, because the runtime's `Origin` check compares hosts. A cookie scoped to `Path=/p/<id>` is not a boundary, and a `__Host-` cookie must have `Path=/`. A session made on a path address can therefore be used by any other app on `<domain>`, including apps of other owners.

The Kernel does not refuse a path address to an app with accounts or a private one. When the owner turns it on for such an app, `path_set` requires `acceptSharedOrigin: true` and refuses without it with a stable code whose message names the risk. The editor shows the same message before confirming. An app needs no acknowledgement when it is public, its signup policy is Closed and every action is `anonymous`, because then no session exists to share. A public app starts Open, so the refusal names that way out too. The app keeps its generated hostname and alias, where its sessions stay on its own origin, and the editor shows the hostname as the address to give signed-in users.

The app session cookie on a path address is named for the Deployment, `konae_app_session_<id>`, with `__Secure-` under HTTPS and `Path=/`. The name keeps two apps from overwriting each other's session. `Path=/` lets one session reach the app on both its id and its name, which a `Path=/p/<segment>/` cookie would not. That keeps apps apart by mistake only, not against a hostile app. The runtime also refuses a request whose `Referer` names another app's path, which a script can avoid by sending no referrer.

### What leaves with the app

A package's manifest carries the name and whether the path is on, never the UUID. On import the app gets its own `/p/<id>` on the destination. A name that is free there is kept, and a name that is taken is dropped and the import notice says so. An exported Kernel serves path addresses once its operator sets `KONAE_DOMAIN`. Without it the app answers on its generated hostname.

## Considered options

- **Path addresses only for public apps without accounts.** Rejected by the product owner: the owner decides. The shared-origin risk is stated at both doors instead.
- **A redirect from `/p/<name>` to the app's hostname.** Rejected because the address bar would show the hostname, which is what the path address exists to avoid.
- **Serve `/p/` on the editor's origin.** Rejected. The owner cookie is `Path=/` and an app's same-origin request would carry it to administrative MCP.
- **`mcp.<domain>`.** Rejected as described above. `KONAE_MCP_ORIGIN` still lets an operator choose it.
- **Rewrite root-relative URLs in served HTML.** Rejected because a script builds URLs at run time, which no rewrite reaches.

## Consequences

- An app may have three kinds of address at once: generated hostname, custom domain, path. App settings lists them together, built from the domain the Kernel reports, since the browser never receives the variable.
- The home is edited like any app, and the docs and architecture map in `examples/docs-site` and `examples/architecture-map` can be published the same way.
- `ops/nginx.local.conf` serves the editor on `app.localhost` and sends the bare host to the Kernel with `Host` unchanged.
- `guide.rs` adds a row to `pieces` for the path address, and `AGENTS.md` adds one to its table.
- `docs/architecture.md` records that `<domain>` is one origin shared by the home and every path address. The isolation it describes holds for generated hostnames and aliases only.
