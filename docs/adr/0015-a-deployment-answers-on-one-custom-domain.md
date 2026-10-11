# A Deployment answers on one custom domain

Status: accepted, 2026-11-07. Builds on [ADR-0008](0008-authenticated-mcp-is-the-platform-operation-protocol.md)'s isolation headers and [ADR-0013](0013-an-app-leaves-as-a-chunked-package.md)'s package, and on [ADR-0014](0014-an-app-migrates-with-one-writer-at-a-time.md) for the maintenance page.

A landing page, a blog and a shop each need their own domain (capability matrix, F22). The Kernel answered only the hostnames it generates, `<slug>-<id>.<suffix>`. Another `Host` answered 404, and a proxy that rewrote `Host` still carried the page's `Origin`, which the runtime endpoint refused. The nearest alternative was a redirect, which leaves the generated hostname in the address bar and in every link.

## Decision

A Deployment may hold one custom domain, a hostname alias kept in `deployments/<id>/domain.json` next to `deployment.json`. Three admin tools manage it: `domain_read` with `read`, and `domain_set { deploymentId, hostname }` and `domain_clear` with `publish`, the permission that already decides what a Deployment serves.

A hostname is lowercased and loses a trailing dot. It must be a DNS name of letters, digits and hyphens with at least two labels, not an IP address, not `localhost`, not under the installation's suffix and not the host of `LITTLEBOX_PUBLIC_ORIGIN` or `LITTLEBOX_MCP_ORIGIN`. `kernel-rs/src/domain-cases.json` holds every rule with its message, and the Kernel and the CLI's `src/lib/domain-name.ts` both run it.

The first `domain_set` records a pending claim with a token and answers with the TXT record `_littlebox.<hostname>` that must hold it. Every later `domain_set` looks the record up with the Kernel's own DNS query over UDP, sent to `LITTLEBOX_DNS_RESOLVER` or to the first `nameserver` in `/etc/resolv.conf`. The alias serves when one of the record's values is the token. A record that holds other values is refused with what it holds and the token it needs, except on the call that issued the token, which the owner could not have known; that call answers pending and names what the record holds. No record, or a lookup that fails, leaves the claim pending and the answer says which.

A name that another existing Deployment serves is refused, at `domain_set` and at import. A pending claim blocks nobody, so nobody can hold a name they cannot prove. The token is random per Deployment and stays when the hostname changes, so an owner can add the record for a new name before switching to it. `domain_clear` deletes the claim and its token.

`deployment_for` resolves a verified alias to its Deployment after the generated hostnames. Everything that reads the host then treats the alias as the app's own origin with no other change: the runtime endpoint's `Origin` check, host-only cookies with `__Host-` names under HTTPS, the policy's `'self'`, HTTP routes, `GET /_littlebox/files/<id>` and the maintenance page. The generated hostname keeps answering, and a session made on one host is not sent to the other. The SDK calls `/_littlebox/mcp` on the page's own host. Administrative MCP, owner sign-in and OAuth count an `Origin` on any claimed name as an app origin and refuse it.

Over HTTPS an alias gets `Strict-Transport-Security: max-age=31536000` without `includeSubDomains`, because the owner's other names under their domain are not the installation's to put on HTTPS. Verification, recovery and invitation email links use the alias once it serves.

The Kernel issues no certificate. The operator's proxy terminates TLS for the alias and forwards `Host` unchanged; `ops/nginx.local.conf` shows how a second host reaches the Kernel.

A package's manifest carries the hostname without its token. The import records it as a pending claim with a token the destination issues. Whoever uploads a package writes its manifest, so a token copied from public DNS into a manifest would otherwise let one owner claim another's pending name on a shared installation. An imported app is paused until `migration_activate`, which refuses `domain_set`, so the order that leaves no gap on the alias is: activate the destination, read its token with `domain_read`, add it to the TXT record next to the origin's value, call `domain_set` on the destination, then point the name's DNS at the destination. A migration confirmed without `newUrl` links the alias on the origin's maintenance page, and the page served on the alias itself shows no link to itself.

## Considered options

- **A field on `Deployment`.** Rejected because the record is built in a dozen places, test fixtures included, and the alias has its own lifecycle. A file in the Deployment's folder travels with copies and restores like `migration.json`.
- **Check DNS on each request, or on a schedule.** Rejected. A lookup on a request lets a visitor make the Kernel query DNS at will, and no request needs it: once DNS points elsewhere the Kernel no longer receives the alias's traffic. The owner frees a name with `domain_clear`.
- **Prove control with an HTTP file on the domain.** Rejected because the domain answers from the operator's proxy, which would first have to route the unverified name to the Kernel.
- **Carry the token in the package.** Rejected because a manifest is written by its uploader, as the decision says.
- **Issue certificates in the Kernel.** Rejected. TLS stays with the proxy, as it does for the generated hostnames.

## Consequences

- A Deployment answers on two origins with separate cookies, so a person signed in on the generated hostname signs in again on the alias.
- A name stays verified until the owner clears it, even after its TXT record is removed.
- The editor exposes the domain under App settings, reachable from More and Deploy. It shows the hostname, TXT claim, verification result and actions to change or remove it. A change names the alias it replaces before confirmation. The integrated agent uses the same MCP operations, bound to the current project's deployment, with owner confirmation before mutation. The CLI remains available through `bun run littlebox -- domain`.
- The editor labels `active` as ownership verified. Routing and HTTPS remain the hosting provider's work, and the editor does not report them as checked. An already verified record keeps its verification when refreshed; it does not trigger another DNS lookup.
- Both the editor's self-host kit and the chunked package carry the hostname without a token. The destination creates a pending claim with a fresh token. The generated deploy script prints the TXT record and the verification operation.
