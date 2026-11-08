# HTTP routes take named callers, and calls leave through the Kernel

Status: accepted.

A payment provider confirms an order with a webhook, a script the owner wrote posts rows, and an app tells another app that a task changed. None of that may run code of the app's own on the server or put a secret in a page anyone can read. Each is a declaration in `actions.json` that the Kernel runs, so it leaves with the app.

## Decision

### A route binds one action to a method and a path

An action declared `anonymous` may add `"http"` with a method and a path on its Deployment's hosts. An `authenticated` action may not, so a route never needs a cookie to reach a person's rows. The action keeps its bindings, its query bound and its rate limit. A route answers 413 above 64 KiB.

### A route answers to the callers it names

A route has three kinds of caller, resolved before any SQL runs:

| Caller | Proof | Actor ID |
| --- | --- | --- |
| An outside service | the route's signature | none |
| A program the owner wrote | an API key as `Authorization: Bearer` | the key's ID |
| A signed-in person | the app session cookie, same origin | the user's ID |

- `"callers"` names any of `"signature"`, `"key"` and `"session"`, and lists `"signature"` exactly when the route declares one. Without it, a signed route takes the signature or a key, an unsigned route on a private Deployment takes a key or a session, and an unsigned route on a public Deployment answers anyone.
- A caller with no proof the route takes gets 401 `authentication-required`.
- An action whose route does not answer anyone answers only there. The runtime endpoint refuses it with `Action <name> answers only on its HTTP route, <method> <path>.`
- Visibility does not gate a route, so a private app can receive a webhook.

### API keys

An API key is a Runtime Actor of one Deployment. The owner issues it with `api_keys_create`, names the actions and file visibilities it may use, and sees its token once. The Kernel stores only the SHA-256. A key grants no owner tool. On the file path it reads what a visitor reads, plus the visibilities it names. An unknown or revoked key counts against the client address, 10 a minute per Deployment, and a valid key never drains that count. The runtime endpoint refuses a request that carries a key and says where a key works.

### Signatures

- Konae's own scheme signs `t`, a nonce, the method, the path, the sorted query string and the raw body with HMAC-SHA256, as `t=<unix seconds>,n=<nonce>,v1=<hex>` in the header the route declares. A `t` more than 300 seconds off answers `stale-timestamp` before the MAC is checked. A header carries at most two `v1=` values, one per secret during a rotation. `src/lib/route-signature.ts` and the Kernel sign the same vectors.
- A route may name a provider's own scheme: `hmac-body` (GitHub, Shopify), `timestamped` (Stripe) or `svix` (Svix, Resend). A preset, `{ "provider": "stripe", "secret": "<name>" }`, stands for that provider's long form, from `kernel-rs/kernel/src/signature-presets.json`.
- `"fields"` maps each input to a JSON path into the body, so a provider's nested payload reaches the action and other keys are left out. Without it, a body key the action does not declare is refused.
- The Kernel records each accepted signature in the app's identity file before the action runs and forgets it when the action fails, so a provider's retry of a failed delivery runs and a replay does not. A body-only MAC and a Svix id are remembered for 72 hours, a timestamped one until its `t` leaves the window.

### Secrets

The owner sets a secret with `secret_set`, or the `kernel` binary's `secret`, and its value is never returned. It lives in the app's `data/secrets.sqlite` and never enters `actions.json`, the artifact or an export. Each secret has one use: `route` verifies a route, `call` signs or carries an outgoing call, and `google` holds the app's Google client (ADR-0003). Konae refuses a route secret whose value another of the same owner's apps holds as a route secret, because two apps that verify with one value accept each other's requests. A `call` secret may repeat another app's route secret, so an owner's app can sign a call to another of their apps, and a route that names a `call` secret is refused at publish.

### A call action sends one request from the Kernel

- An action of `"kind": "call"` declares one request: `"method"` `GET` or `POST`, `"url"`, `"body"` as a map from body keys to its inputs, an optional `"auth"` and an optional `"then"`. Its bindings may only be inputs, so the Kernel adds no identity or clock to a request that leaves the app.
- The runtime endpoint checks the caller and the inputs, queues the call in `calls.sqlite` and answers `{ callId, queued: true }`. One worker sends it, so a slow host never holds a request and a call survives a restart.
- `"auth": { "secret": "<name>" }` signs with Konae's scheme, in `X-Konae-Signature` or the header the call names. `"auth": { "key": "<name>" }` sends the secret's value as a bearer token.
- A URL is https, or http only to an app of the same installation. Both doors refuse credentials, fragments, IP addresses, `localhost`, `metadata`, `instance-data` and `.internal` names. Before each attempt to an outside host the Kernel resolves the name, refuses the call when any address is not public, and connects to the address it checked. It follows no redirect and uses no proxy from the environment.
- A call to an app of the same installation goes through the same route code an outside request meets, with its callers, signature, replay memory, guard and Usage.
- An attempt has 10 seconds and reads at most 64 KiB. A network failure, a timeout, 408, 425, 429 or a 5xx is retried after 30 seconds, then 1, 2 and 4 minutes, up to 5 attempts. An app queues at most 20 calls a minute unless the operator sets `KONAE_CALLS_PER_MINUTE`. A paused app sends nothing.
- When a call ends, its `then` mutation runs once with no actor, its inputs read by JSON pointers from `{ ok, status, error, body, input }`. Only the Kernel runs a `then` mutation, so nobody can forge an answer. The owner reads calls with `calls_read`.

### A page may read another app's open route

The manifest's `"connects"` adds origins to the page's `connect-src`, and a GET route that answers anyone sends `Access-Control-Allow-Origin: *`. A route that takes a key, a session or a signature sends no CORS header.

## Rejected options

- **Let visibility gate routes too.** A private app could not receive a webhook.
- **`http` on an `authenticated` action with a cookie.** A person's rows would sit behind a URL that cross-site requests reach.
- **Let a session reach a route-only action through the runtime endpoint as well.** Two doors to one action are two places to get the check wrong. The app's page reaches its route with `fetch`, which sends the cookie.
- **An adapter outside the app that re-signs a provider's webhook.** A person who does not write code cannot run one, and it takes the app's rules to a server that does not leave with the app.
- **A template language for any signature layout.** Both doors would have to parse a small language identically. A new provider form is a new scheme with its own cases.
- **Trust a provider's delivery ID for replays.** GitHub's sits outside its MAC, so a replay could change it.
- **Bind the host into Konae's signature.** It would change the wire format for every sender, and senders behind a proxy do not always know the host. The reuse refusal covers the replay between two apps instead.
- **Call another app with `fetch` from the page.** A signed request would put the secret in the page, and an open `connect-src` would let any script send the app's data anywhere.
- **Send the request from the request handler.** One slow host would hold a request for 10 seconds per call, and a restart would lose it.
- **Allow private addresses for operators with internal services.** The rule is what keeps an app from reading the Kernel's own network, and a switch would loosen it for every app.
- **Follow redirects after checking each hop.** A redirect is the usual way around an address check.

## Consequences

- A body-only MAC binds no path, so each provider route gets its own secret. A delivery replayed more than 72 hours later passes an `hmac-body` route, and the provider's event ID in a `UNIQUE` column is the lasting guard.
- A form-encoded body and a scheme with its timestamp in its own header, such as Slack's, are still refused.
- An export carries no secret and every key revoked, so the owner sets secrets and issues keys again on a new installation. Pending calls stay on the installation that queued them.
- The editor preview answers a call with `{ callId, queued: true }` and sends nothing. Calls run on `konae dev` and on a Deployment.
- The Action Contract names each route's scheme and fields, and `konae conform` signs its probes in that scheme.
