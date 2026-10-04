# A route verifies a provider's own signature

Status: accepted, 2026-11-07. Amends [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md)'s HTTP routes, which took a signature in Littlebox's scheme only.

A route's signature was Littlebox's own: an HMAC over a timestamp, a nonce, the method, the path, the query and the body, in one header. No outside service signs that way, so a code host's, a shop's or a payment provider's webhook answered `invalid-signature`, and a body with fields the action did not declare answered `Unexpected action input`. The capability matrix found that this left the integration hub with no way to receive a webhook and left payments with no verified confirmation. The only answer was an adapter the owner runs elsewhere, which is a second server, the thing Littlebox exists to avoid.

## Decision

A route's `signature` may name a `scheme`. `littlebox` stays the default. Three more cover the forms the common providers use:

- `hmac-body`: the HMAC of the raw body with `algorithm` `sha256` or `sha1`, written in `hex` or `base64`, after an optional `prefix`. GitHub and Shopify sign this way.
- `timestamped`: comma-separated fields in one header, the Unix time after `prefix` and the hex HMAC-SHA256 of `<t>.<body>` after `tag`, refused outside a declared `window` of 1 to 3600 seconds. Stripe signs this way.
- `svix`: the three `svix-*` headers, the HMAC-SHA256 of `<id>.<t>.<body>` keyed with the base64 after `whsec_`, refused 300 seconds off. Resend and Svix sign this way, and the Kernel already checked it for Resend's email events.

A route may also declare `fields`, a JSON path into the body for each input. With it the route reads each named input from its path, every other input from the body's key of its name, and leaves out every other key, so a provider's nested payload reaches the action as it is.

Both are declarations in `actions.json`. The Kernel reads them once per release and verifies inside its one process; no app runs code of its own. The Validation Gate and the Kernel refuse an unknown scheme, a key the scheme does not take, a provider's scheme or `fields` on `GET`, a missing or reserved header, a bad setting, a bad path and a path for an input the action does not bind, with the same messages from one case file.

Each scheme accepts a delivery once. The Kernel records it in a `route_deliveries` table of the Deployment's `identity.sqlite`, with its own expiry, before the action runs, and forgets it when the action fails, so a provider's retry of a delivery that failed runs. A body-only MAC and a Svix id are remembered for 72 hours, a timestamped MAC until its `t` leaves the window.

## Considered options

- **An adapter outside Littlebox that verifies the sender and re-signs in Littlebox's scheme.** Rejected as the answer, because a person who does not write code cannot run one, and it takes the app's rules to a server that does not leave with the app.
- **One generic template for the signed string and its header layout.** Rejected because each template is a small language both doors must parse identically, and the three forms above cover the providers the showcase and the probes needed. A fourth form is a new scheme with its own cases.
- **Trust a provider's delivery ID header for replays.** Rejected for `hmac-body`, because GitHub's `X-GitHub-Delivery` is outside the MAC, so a replay could change it. The MAC is remembered instead. Svix's id is inside its MAC, so it is the key there.
- **Remember a delivery even when its action fails.** Rejected because providers retry a delivery that did not answer 2xx with the same body, and a failure the owner then fixes, such as a missing column, would refuse every retry as a replay.
- **Allow undeclared body keys everywhere.** Rejected because the strict rule catches a sender that misspells an input. It stays for routes without `fields`.

## Consequences

- The integration hub receives GitHub-, Stripe- and Resend-style webhooks without an adapter, and a payment provider's webhook can mark an order paid through a guarded mutation.
- A body-only MAC binds no path, so two routes with one secret accept each other's deliveries. Each provider route gets its own secret, and docs/mcp.md says so.
- A delivery replayed more than 72 hours later passes an `hmac-body` route. The provider's event ID in a `UNIQUE` column is the lasting guard.
- A delivery over 64 KiB answers 413, as every route body does. A form-encoded body and a scheme with the timestamp in its own header, such as Slack's, are still refused.
- The Action Contract names each route's scheme, settings and fields, and the conformance runner signs its probes in that scheme.
