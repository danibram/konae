# An app calls another app from the Kernel

Status: accepted, 2026-11-07. Builds on [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md)'s HTTP routes, callers and signing secrets, and on [ADR-0010](0010-kernel-owns-usage-and-signals.md)'s Usage. Amends ADR-0011's rule that a secret's value is unique among one owner's projects: a secret set for calls only may repeat another project's route secret.

The capability matrix (docs/research/capability-matrix.md) found that no action reaches outside its app. A team tool cannot tell another app that a task changed, an integration hub cannot call a service, and the only way to send a signed request was a program the owner runs elsewhere. The page cannot do it either: its policy allows `connect-src 'self'`, and a secret or a key in the page would be readable by anyone who opens it. The product rule (docs/architecture.md) allows no process per app and no user code on the server, so the answer has to be a declaration the Kernel runs.

## Decision

An action of `"kind": "call"` declares one HTTP request: `"method"` `GET` or `POST`, `"url"`, `"body"` as a map of body keys to the call's input keys, an optional `"auth"` and an optional `"then"`. Its bindings may only be inputs, so the Kernel adds no identity or clock to a request that leaves the app. `actions_invoke` checks the caller and the inputs as for any action, then queues the call in `calls.sqlite` and answers `{ callId, queued: true }`. One worker task in the Kernel sends it, so a slow host never holds a request handler, and a call survives a restart.

- `"auth": { "secret": "<name>" }` signs the request with Konae's route scheme, keyed with the secret's value, in `X-Konae-Signature` or the header `"header"` names. `"auth": { "key": "<name>" }` sends the secret's value, an API key, as `Authorization: Bearer`. The value stays in `mcp.sqlite`; it never reaches the browser, the artifact or an export.
- A secret set with `"use": "call"` may hold another project's route secret, so one owner's app can sign a call to another of their apps. It verifies no route of its own app: a route that names it is refused at publish. ADR-0011's reuse refusal stays for route secrets, where two apps verifying with one value is the danger.
- A URL is https, or http only to an app of this installation. Both doors refuse a URL with credentials or a fragment, an IP address as host, `localhost`, `metadata`, `instance-data` or a `.internal` name, and a first path segment the Kernel owns. The Kernel alone, at publish and before each attempt, refuses the editor's hosts and http outside the installation. Before each attempt to an outside host it resolves the name, refuses the call when any address is not public, and connects to the address it checked. It follows no redirect and uses no proxy from the environment.
- A URL whose host is a Deployment of this installation goes through `http::call_route`, the route code a request from outside meets, with the same callers check, signature check and replay memory, guard, Usage and Signal. Each calling app gets its own synthetic client address, so the target's per-address limit counts each app on its own.
- An attempt has 10 seconds and reads at most 64 KiB. A network failure, a timeout, a DNS failure, 408, 425, 429 or a 5xx is retried after 30 seconds, then 1, 2 and 4 minutes, up to 5 attempts. An app queues at most 20 calls in any 60 seconds unless the operator sets `KONAE_CALLS_PER_MINUTE`. A paused app sends nothing.
- When a call ends, delivered or failed, its `then` mutation runs once with no actor, its inputs read by JSON pointers from `{ ok, status, error, body, input }`. So the caller's rows record what the other app answered, a guard refusal included. Only the Kernel runs a then mutation: `actions_invoke` refuses it, so nobody can forge an answer.
- Usage counts a queued call under `call`; the target counts the request as its own route. The owner reads every call, its attempts, status, error and the then mutation's outcome with `calls_read`, which needs `read`.
- The manifest's `"connects"` adds origins to the page's `connect-src`, and a GET route that answers anyone sends `Access-Control-Allow-Origin: *`, so a page may read another app's public route directly. A route that takes a key, a session or a signature sends no CORS header.

The Validation Gate and the Kernel parse calls and `connects` from `kernel-rs/src/call-cases.json`.

## Considered options

- **Let the page call the other app with `fetch`.** Rejected because a signed or keyed request would put the secret in the page, and an open `connect-src` would let any script on the page send the app's data anywhere. `connects` keeps the browser to named origins and to routes that need no secret.
- **Send the request from the request handler and answer with the response.** Rejected because one slow host would hold a request for 10 seconds per call, a restart would lose the call, and a retry would need the person to click again. The queue costs the page an asynchronous answer, which the `then` mutation turns into a row the page reads.
- **Allow private addresses for operators who run apps next to internal services.** Rejected for now. The rule is what keeps a Capsule from reading the Kernel's own network, and an operator switch that loosens it would be on for every app of the installation. An internal service can expose a public endpoint with a key.
- **Sign a call to an app of this installation with the target's own secret, without the caller holding it.** Rejected because the Kernel would then decide on its own that one app may call another, which is the owner's decision. The caller holds the value, as any outside sender does.
- **Follow redirects after checking each hop.** Rejected because a redirect is the usual way around an address check, and a webhook rarely needs one. A 3xx is the answer and the then mutation sees it.

## Consequences

A call's body may use dotted paths such as `line_items.0.price`, which expand into nested JSON on POST, and may set `Idempotency-Key` in `headers`, so a Checkout Session create reaches Stripe without a free-form object or a secret in the page. The shop probe creates a session through a call, stores the id in its then mutation, and marks the order paid from a Stripe-shaped webhook.

Pending calls do not travel in a package or a migration; the outbox stays on the installation that queued them, and a retired origin ends its pending calls with `app-moved`. Scheduled copies carry `calls.sqlite` with the other root stores, and a restore brings it back as the copy held it, so a call queued after the copy is lost and one sent after it may be sent again; a receiver that matters keys its writes on the caller's reference, as the probe's desk does.

The editor preview answers `{ callId, queued: true }` and sends nothing, because the browser cannot reach the other app's route without the secret. Calls run on a Development Kernel or a Deployment.


## Integration check, 2026-11-07

A changed `then` mutation processes its declared notice through ADR-0016's queue and budgets, with the inputs mapped from the answer. A notice to the owner works; a notice to actor records no-account because the answer has no caller. A refused or unchanged mutation sends nothing. The calls probe checks owner notices for three changed answers and the no-account outcome of a notice to actor.
