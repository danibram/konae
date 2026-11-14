# Routes and calls

Declare an HTTP route to receive a request or a call action to send one, and read here who may use each and what the Kernel refuses.

## HTTP routes

An action declared `anonymous` may answer one method and path on its Deployment's host, so a project receives a webhook or serves JSON without a server of its own. `kernel-rs/kernel/src/http.rs` implements it, and [ADR-0013](../adr/0013-http-routes-take-named-callers-and-calls-leave-through-the-kernel.md) records the decision.

```json
"orders.receive": {
  "kind": "mutation",
  "access": "anonymous",
  "http": { "method": "POST", "path": "/hooks/orders", "signature": { "header": "X-Signature", "secret": "orders_webhook" } },
  "statement": "INSERT INTO orders (external_id, total) VALUES (?1, ?2)",
  "bindings": [
    { "source": "input", "key": "id", "type": "string", "maxLength": 64 },
    { "source": "input", "key": "total", "type": "number" }
  ]
}
```

A query binds `GET` and a mutation binds `POST`. A path starts with `/` and has segments of letters, digits, `.`, `_`, `~` and `-`, up to 200 characters, with no `.` or `..` segment and no trailing slash. Paths under `/_konae`, `/oauth`, `/.well-known` and `/mcp` belong to the Kernel. Two actions share a path only with different methods. A route wins over an artifact file at its path for its method, and every other `GET` serves the Build Artifact. An `authenticated` action may not declare `http`. The Gate and the Kernel run the cases in `kernel-rs/kernel/src/http-binding-cases.json`.

A `GET` maps the query string onto the input bindings by type, so `?min=10` binds the number 10. A `POST` reads a JSON object of at most 64 KiB. An input the action does not declare is refused unless the route declares `fields`, described under [Webhooks from outside services](#webhooks-from-outside-services). A route keeps the action's bindings, its query bound and the runtime endpoint's rate limits. A call that succeeds counts in Usage under the route's method and path and records the action's Signal.

| Answer | Status | When |
| --- | --- | --- |
| `{ "result": [rows] }` or `{ "result": { "changes", "lastInsertId" } }` | 200 | The action ran. Every answer carries `Cache-Control: no-store`. A `GET` route that answers anyone also carries `Access-Control-Allow-Origin: *` |
| `{ "error": "<binding message>" }` | 400 | An input is missing, unexpected or of the wrong type, the body is not a JSON object, or a query parameter repeats |
| `{ "error": "invalid-signature" }` | 401 | The signature is missing, malformed, lacks its declared prefix, or does not match |
| `{ "error": "stale-timestamp" }` | 401 | The signature's time is more than 300 seconds off, or outside a `timestamped` signature's `window`. Checked before the MAC |
| `{ "error": "authentication-required" }` | 401 | A route that takes an API key or a session, and no signature, got neither |
| `{ "error": "invalid-api-key" }` | 401 | The bearer token starts with `lbk_` and names no live key of this Deployment |
| `{ "error": "This route does not accept API keys." }` | 403 | The route's `callers` leave out `key` and the request carries a live key |
| `{ "error": "This API key may not call <action>." }` | 403 | The key does not name the action |
| `{ "error": "<guard>" }` | 403 | The mutation's [guard](./usage-and-signals.md#guards) refused |
| `{ "error": "No route accepts POST on this path." }` | 405 | No action binds the path for `POST` |
| `{ "error": "A route does not answer HEAD. Send GET." }` | 405 | A `HEAD` reaches a path a query binds, refused before any SQL with `Allow: GET` |
| `{ "error": "replayed-signature" }` | 409 | The same signature arrived on this path in the last 600 seconds, or a provider's delivery arrived before |
| `{ "error": "The request body exceeds 64 KiB." }` | 413 | |
| `{ "error": "rate-limited" }` | 429 | With `Retry-After`. Also after 10 unknown or revoked `lbk_` keys from one client address to one Deployment in a minute; from then on every key from that address is refused until the bucket refills |

### Who may call a route

The Kernel resolves the caller before any SQL runs.

| Caller | `callers` entry | Proof | Actor ID |
| --- | --- | --- | --- |
| A service | `"signature"` | The signature the route declares | None, so an action that binds the actor's ID refuses |
| A program the owner wrote | `"key"` | `Authorization: Bearer lbk_...`, an API key that names the action | The key's ID, `key_...` |
| A person | `"session"` | The app session cookie, sent from the app's own origin | The app user's ID |

Without `"callers"`, a signed route takes `["signature", "key"]`, an unsigned route on a private Deployment takes `["key", "session"]`, and an unsigned route on a public Deployment answers anyone. `"signature"` belongs in the list exactly when the route declares a signature. A key the route does not take answers 403. A session the route does not take counts for nothing. The Kernel uses the cookie only when the request comes from the app's own origin, so a request whose `Sec-Fetch-Site` is `same-site` or `cross-site`, or whose `Origin` is another host, runs as a visitor.

```json
"events.feed": {
  "kind": "query", "access": "anonymous",
  "http": { "method": "GET", "path": "/api/events", "callers": ["key"] },
  "statement": "SELECT id, title FROM events WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT 100",
  "bindings": [{ "source": "input", "key": "after", "type": "number", "nullable": true }]
}
```

An action whose route does not answer anyone answers only there. `actions_invoke` refuses it with `Action <name> answers only on its HTTP route, <method> <path>.`, always for a signed route or one that declares `callers`, and on a private Deployment for an unsigned one. The app's own page reads a private route with `fetch`, which sends the session cookie.

| Declaration | Message |
| --- | --- |
| `callers` that is not a non-empty list of distinct `"signature"`, `"key"` and `"session"` | `Action <name> has an invalid callers list. List one or more of "signature", "key" and "session", each once.` |
| `"signature"` without a declared signature | `Action <name> lists "signature" among its callers but declares no signature.` |
| A signature that `callers` leaves out | `Action <name> declares a signature, so its callers must include "signature".` |

### Signatures

The owner sets the secret before publishing. A release whose signature names an unset secret is refused at `release_plan` and at `release_apply` with `Action <name> signs with the secret <secret>, which is not set for this project. Set it with secret_set before publishing.`, and one refusal names every action and secret when several are unset. The Gate makes the same refusal before Deploy from `secrets_list`, which answers names and never values. The cases are in `kernel-rs/kernel/src/release-cases.json`.

```text
secret_set    { "projectId", "name": "orders_webhook", "value": "<16 to 512 printable characters, no spaces>", "use"?: "route" | "call" | "google" }
secrets_list  { "projectId" }           names, uses, update times and svixKey, never values
secret_delete { "projectId", "name" }   refused while the published Deployment signs with it or a call uses it
```

A signature names no app, so a request signed for one project would pass on another holding the same value. `secret_set` therefore refuses a route secret that another project of the same owner holds, with `Another of your projects already uses this secret value. A signature does not name the app, so a request signed for one would be accepted by the other. Set a different value.`

Secrets live in the app folder's `data/secrets.sqlite`, which exists from `project_create`, so a first publish finds them. A scheduled copy carries them. They never enter `actions.json`, the Build Artifact, a listing, the audit log or an export. On another installation the owner runs `project_create`, then `secret_set`, then the release.

The sender puts this in the declared header:

```text
t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256, keyed with the secret, of:
<t>\n<nonce>\n<METHOD>\n<path>\n<canonical query string>\n<raw request body>>
```

The nonce is 1 to 64 letters, digits, `-` and `_`, fresh for each request, so a retry is signed again. The canonical query string is the request's pairs sorted by key, then value, written as `application/x-www-form-urlencoded`: spaces as `+`, and every byte but letters, digits, `*`, `-`, `.` and `_` as `%XX`. It is empty when the request has no query.

The Kernel refuses a `t` more than 300 seconds from its clock with `stale-timestamp` before it looks at the MAC. It then recomputes the MAC over the exact bytes it received, compares in constant time, and remembers each accepted signature for 600 seconds in the `route_signatures` table of `identity.sqlite`, so a replay answers 409 across a restart and on an imported app. Two `v1=` values may be sent while a secret rotates; more answers `invalid-signature` before any MAC is computed. `src/lib/route-signature.ts` signs the same way, and `kernel-rs/kernel/src/signature-cases.json` holds vectors both sides check. A sender with OpenSSL:

```sh
body='{"id":"A-1","total":12}'
t=$(date +%s)
n=$(openssl rand -hex 12)
sig=$(printf '%s\n%s\n%s\n%s\n%s\n%s' "$t" "$n" POST /hooks/orders "" "$body" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')
curl -X POST "https://orders.example.net/hooks/orders" \
  -H 'Content-Type: application/json' -H "X-Signature: t=$t,n=$n,v1=$sig" --data-binary "$body"
```

### Webhooks from outside services

A service the owner does not control signs its webhook its own way and posts its own nested JSON. A route takes it as it arrives when its signature names the provider's `"scheme"` and its `"fields"` say where each input sits in the body. `kernel-rs/kernel/src/provider.rs` implements it.

```json
"issues.receive": {
  "kind": "mutation", "access": "anonymous",
  "http": {
    "method": "POST", "path": "/hooks/github",
    "signature": { "scheme": "hmac-body", "header": "X-Hub-Signature-256", "secret": "github_webhook", "algorithm": "sha256", "encoding": "hex", "prefix": "sha256=" },
    "fields": { "id": "$.issue.node_id", "title": "$.issue.title", "repo": "$.repository.full_name" }
  },
  "statement": "INSERT INTO issues (node_id, title, repo) VALUES (?1, ?2, ?3) ON CONFLICT (node_id) DO NOTHING",
  "bindings": [
    { "source": "input", "key": "id", "type": "string", "maxLength": 100 },
    { "source": "input", "key": "title", "type": "string", "maxLength": 300 },
    { "source": "input", "key": "repo", "type": "string", "maxLength": 200 }
  ]
}
```

| Scheme | Keys | What the sender puts in the request | Replay |
| --- | --- | --- | --- |
| `konae`, the default | `header`, `secret` | The header [above](#signatures) | The MAC, for 600 seconds |
| `hmac-body` | `header`, `secret`, `algorithm` (`sha256` or `sha1`), `encoding` (`hex` or `base64`), optional `prefix` | The prefix, then the HMAC of the raw body. GitHub: `X-Hub-Signature-256`, `sha256`, `hex`, prefix `sha256=`. Shopify: `X-Shopify-Hmac-Sha256`, `sha256`, `base64` | The MAC, for 72 hours |
| `timestamped` | `header`, `secret`, `prefix` (the time field, such as `t=`), `tag` (the MAC field, such as `v1=`), `window` (1 to 3600 seconds) | Comma-separated fields: the Unix time after `prefix` and the hex HMAC-SHA256 of `<t>.<body>` after `tag`. Stripe: `Stripe-Signature`, `t=`, `v1=`, `300` | A `t` outside the window answers `stale-timestamp`; inside it the MAC is accepted once |
| `svix` | `secret` | `svix-id`, `svix-timestamp` and `svix-signature`, the base64 HMAC-SHA256 of `<id>.<t>.<body>` keyed with the base64 after `whsec_`. Resend and Svix send it | A timestamp more than 300 seconds off answers `stale-timestamp`; the id is accepted once in 72 hours |

A `timestamped` or `svix` header may carry up to four MAC values while the provider rotates a secret. A svix secret is the `whsec_` value the provider shows; a release whose svix route names any other value is refused with `Action <name> verifies svix signatures with the secret <secret>, which is not a whsec_ signing secret. Set it to the value the provider shows, starting with whsec_.` `secrets_list` answers `svixKey`, so the Gate makes the same refusal without seeing the value.

The Kernel records an accepted delivery before the action runs, in `route_deliveries` of `identity.sqlite`, so two copies sent at once do not both run. When the action then fails, for a missing input, a SQL error or a guard, the Kernel forgets the delivery and the provider's retry runs. `hmac-body` signs the body alone, so give each provider route its own secret. The provider's event ID in a `UNIQUE` column, with `ON CONFLICT DO NOTHING`, is the lasting guard against a delivery replayed after 72 hours.

`fields` maps an input key to a JSON path into the POST body: `$`, then 1 to 16 steps of `.name` or `[index]`, up to 200 characters, such as `$.data.object.id`. The route reads each mapped input from its path, every other input from the body's key of the same name, and ignores every other key. A path the body lacks leaves its input out, so a required input answers `Missing action input: <key>` and a nullable one binds `NULL`. `fields` works on any POST route, signed or not.

| Declaration | Message |
| --- | --- |
| A scheme other than the four | `Action <name> names an unknown signature scheme. Use "konae", "hmac-body", "timestamped" or "svix".` |
| A key the scheme does not take | `Action <name> has an invalid <scheme> signature. It takes "scheme", "header" and "secret".`, with the scheme's own list |
| A provider's scheme on a `GET` route | `Action <name> binds GET, so its signature must use the konae scheme. A provider's scheme signs a POST body.` |
| No header, or one the Kernel reads itself | `Action <name> has an invalid signature header.` |
| `hmac-body` with no or another `algorithm` | `Action <name> has an invalid signature algorithm. Use "sha256" or "sha1".` |
| `hmac-body` with no or another `encoding` | `Action <name> has an invalid signature encoding. Use "hex" or "base64".` |
| `hmac-body` with a `prefix` of other characters or over 32 | `Action <name> has an invalid signature prefix. Use 1 to 32 letters, digits, "=", "-", "_", "." or ":", such as "sha256=".` |
| `timestamped` with no `prefix`, or one that is not a name and `=` | `Action <name> has an invalid timestamp prefix. Use the field's name and "=", such as "t=".` |
| `timestamped` with no `tag`, one that is not a name and `=`, or the prefix again | `Action <name> has an invalid signature tag. Use the field's name and "=", such as "v1=", other than the timestamp's.` |
| `timestamped` with no `window`, or one outside 1 to 3600 | `Action <name> has an invalid signature window. Use a whole number of seconds from 1 to 3600.` |
| `fields` on a `GET` route | `Action <name> binds GET, so it cannot declare fields. Fields read a POST body.` |
| `fields` that is not an object of 1 to 32 keys | `Action <name> has an invalid fields map. Map 1 to 32 input keys to JSON paths, such as "$.data.object.id".` |
| A key no input binding declares | `Action <name> maps <key>, which is not one of its input bindings.` |
| A path that is not text, lacks `$`, is `$` alone or has another step | `Action <name> maps <key> to an invalid JSON path. Use "$" and then ".name" or "[index]" steps, such as "$.data.object.id".` |

`src/lib/provider-signature.ts` signs the way each provider does, and `kernel-rs/kernel/src/provider-signature-cases.json` holds vectors both sides check. A form-encoded body, as Slack and Twilio send, answers 400.

### API keys

```text
api_keys_create { "deploymentId", "name", "actions": ["orders.receive"], "files": ["private"] }
api_keys_list   { "deploymentId" }               metadata, revocation times and reasons, never the key
api_keys_revoke { "deploymentId", "keyId" }
```

`api_keys_create` returns the key once. A key belongs to one Deployment, grants no owner tool, and works only on HTTP routes and the file read path. The runtime endpoint refuses it with 403 `An API key works only on the app's HTTP routes and its file read path. The runtime endpoint takes an app session.` A key names actions that have an HTTP route and the file visibilities `public` and `private`; a key that names `private` reads every private file of its Deployment. A Deployment holds at most 50 live keys. The Kernel keeps the key's SHA-256 hash in `identity.sqlite`.

An export carries each key's name, actions, file visibilities and hash, and an import marks every live key revoked. `api_keys_list` on the destination shows those with `revokedReason: "import"`, and Auth > Integrations lists them under "Revoked at import", so the owner knows which programs need a new key. A key the owner revoked has `revokedReason: null`. A restore keeps keys live. Auth > Integrations in the editor also sets and deletes secrets and shows a new key once with a curl for each route it names.

### The file read path

`GET /_konae/files/<id>` on the Deployment host returns a stored file with its content type, except that a JavaScript, HTML, XML or SVG type is sent as `application/octet-stream`. It sends `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; sandbox`, `Cross-Origin-Resource-Policy: same-origin`, an `ETag` of the file ID and size, and `Cache-Control: private, no-cache`. A request whose `If-None-Match` names that tag gets 304 without the bytes, after the same check as a read, and counts no file read in Usage. A refusal answers `Cache-Control: private, no-store`. PNG, JPEG, GIF, WebP and AVIF answer `inline`, every other type `attachment`. `miniapp.files.url(fileId)` returns this path, so an `img` or a `fetch` on the app's own origin reads it.

A person reads by the [file rules](./actions.md#runtime-endpoint). With `fileAccess: "app-users"`, only a session for that app grants access. Otherwise a key reads the visibilities it names, and on a public Deployment also public files. A visitor with no session or key on a private Deployment gets 401 `authentication-required` before any file is looked up. Every other caller who may not read the file gets 404 `File not found.`, the same as for a missing file. There is no listing at `/_konae/files/`, and nothing is served from the bucket.

## Calls between apps

An action of `"kind": "call"` sends one HTTP request to another app or an outside service. The Kernel sends it from its own process, so a signing secret or an API key the request needs stays in the Kernel. `kernel-rs/kernel/src/calls.rs` implements it.

```json
"tasks.escalate": {
  "kind": "call",
  "access": "authenticated",
  "method": "POST",
  "url": "https://desk.apps.example.org/hooks/tickets",
  "bindings": [
    { "source": "input", "key": "taskId", "type": "number" },
    { "source": "input", "key": "title", "type": "string", "maxLength": 200 }
  ],
  "body": { "ref": "taskId", "title": "title" },
  "auth": { "secret": "desk_tickets", "header": "X-Desk-Signature" },
  "then": { "action": "tasks.escalated", "inputs": { "id": "/input/taskId", "status": "/status", "note": "/body/error" } }
}
```

`miniapp.actions.call("tasks.escalate", { taskId, title })` checks the caller and the inputs, queues the call and answers `{ callId, queued: true }`. A call binds only inputs. `body` maps each key to an input or to a constant written `{ "value": "tasks" }`. A `POST` sends a JSON object and a `GET` sends query parameters, and an input that is null or missing leaves its key out. Usage counts each queued call under `call`.

| `auth` | What the request carries |
| --- | --- |
| `{ "secret": "<name>" }` | A signature in Konae's [route scheme](#signatures), in `X-Konae-Signature` or the header `"header"` names, signed again for each attempt |
| `{ "key": "<name>" }` | `Authorization: Bearer <value>`, where the secret holds an API key the other app's owner issued |

A secret that signs a call to another app of the same owner holds that app's route secret, which `secret_set` refuses as a reused value. Set it with `"use": "call"`. Such a secret verifies no route of its own app, and a route that names it is refused at publish with `Action <name> signs its route with the secret <secret>, which is set for calls only. Set a route secret with a value of its own.`

### Where a call may go

A URL is `https`, or `http` only to an app of this installation.

| Rule | Checked | Refusal |
| --- | --- | --- |
| An absolute `http` or `https` URL, up to 2048 characters, with no user, password, fragment, space or control character | Both doors | `Action <name> has an invalid url. Use an absolute https:// URL without credentials or a fragment, up to 2048 characters.` |
| No IP address as the host, in any notation | Both doors | `Action <name> calls the IP address <host>. Name a host instead.` |
| No `localhost`, `metadata`, `instance-data` or name under `.internal` | Both doors | `Action <name> calls <host>, which names this machine or a cloud metadata service.` |
| No first path segment `_konae`, `oauth`, `.well-known` or `mcp` | Both doors | `Action <name> calls a path under /_konae, /oauth, /.well-known or /mcp, which belong to the Kernel.` |
| Not the installation's public or MCP host | The Kernel, at publish and before each attempt | `Action <name> calls <host>, the editor's host. A call reaches an app or an outside service, never the editor.` |
| `http` only to a host under `KONAE_HOST_SUFFIX` | The Kernel, at publish and before each attempt | `Action <name> calls http://<host>, which is not an app of this installation. Use https.` |
| Every address an outside host resolves to is public | The Kernel, before each attempt | The call fails with `address-refused` |

Before each attempt to a host outside the installation, the Kernel resolves the name and refuses the call when any address is loopback, private, link-local, shared, documentation, benchmarking, multicast, reserved, unspecified, unique-local, or one of those carried in IPv6. It connects to the address it checked, follows no redirect and uses no proxy from the environment. A URL whose host is a Deployment of this installation does not leave the process: the Kernel answers it with the route code an outside request meets, with its callers, signature check, rate limit, guard, Usage and Signal. A host under the suffix that no app holds fails with `app-not-found`, and an app in maintenance answers 503.

### Delivery and the outbox

The worker in `calls.sqlite` sends each call within seconds. An attempt has 10 seconds, and an answer over 64 KiB ends the call with `answer-too-large`. A network failure, a timeout, a DNS failure, 408, 425, 429 or a 5xx is retried after 30 seconds, then 1, 2 and 4 minutes, up to 5 attempts. Any other answer is final. An app queues at most 20 calls in any 60 seconds, or `KONAE_CALLS_PER_MINUTE`; the next is refused with `This app reached its limit of calls in the last minute. Try again in a minute.` An app in maintenance sends nothing until it leaves maintenance.

When a call ends, its `then` mutation runs once, with no actor. Each of its inputs is read by a JSON pointer from this document:

| Pointer | Value |
| --- | --- |
| `/ok` | `true` for a 2xx answer |
| `/status` | The HTTP status, or null when no answer arrived |
| `/error` | The Kernel's error kind, such as `timeout`, `address-refused` or `app-not-found`, or null |
| `/body` | The answer parsed as JSON, or null; `/body/error` is another Konae app's refusal |
| `/input` | The call's own inputs |

A pointer that finds nothing leaves its input out, so it binds NULL when the input is nullable. A string input takes a number, a boolean or a structure as its JSON text. The `then` mutation must be a mutation with no HTTP route and no actor binding, and every required input must be filled. `miniapp.actions.call` refuses it with `Action <name> runs only after the call <call> answers.` It counts in Usage as an action, or under `refused` when its guard refuses. Its `notify` sends as any mutation's does; a notice to `actor` records `no-account`.

`calls_read { "deploymentId", "cursor"? }` needs `read` and lists the app's calls newest first, 50 at a time: the action, `pending`, `delivered` or `failed`, attempts, method, URL, status, error kind, the first 1 KiB of the answer and the `then` mutation's outcome. The outbox keeps the newest 1000 finished calls per app. An export does not carry pending calls. A scheduled copy carries `calls.sqlite`.

### A page that reads another app

`connects` in the project manifest, `manifest.connects` in `konae.json`, such as `["https://desk.apps.example.org"]`, adds those origins to the page's `connect-src`, so its `fetch` may read another app's public route. A `GET` route that answers anyone answers with `Access-Control-Allow-Origin: *`. A route that takes a key, a session or a signature sends no CORS header, and a request that needs a key is a call action. `connects` lists at most 10 origins, each exactly `https://host` or `https://host:port`, and `http` only under `localhost`.

| Declaration | Message |
| --- | --- |
| Not a list of at most 10 strings | `connects must be a list of at most 10 origins, such as ["https://api.example.com"].` |
| A path, a trailing slash, a wildcard, capitals, the default port, or `http` outside `localhost` | `connects lists <entry>, which is not an origin. Write it as https://host or https://host:port, with no path; http only for a host under localhost.` |
| An origin twice | `connects lists <entry> twice.` |

### Declarations both doors refuse

The Gate and the Kernel parse calls and `connects` from `kernel-rs/kernel/src/call-cases.json`, with these messages besides the URL rules above:

| Declaration | Message |
| --- | --- |
| A query, mutation or signal action with `url`, `method`, `body`, `headers`, `auth` or `then` | `Action <name> is a <kind>, so it cannot declare url, method, body, headers, auth or then. Those keys belong to an action of "kind": "call".` |
| A call with a statement, signal, guard or http | `Action <name> is a call, so it runs no SQL and cannot declare a statement, a signal, a guard or an http route.` |
| A call that binds the actor, the time or a literal | `Action <name> is a call, so its bindings may only be inputs.` |
| A method other than `"GET"` or `"POST"` | `Action <name> must call with "method": "GET" or "POST".` |
| A `body` that is not an object of at most 32 keys, each naming an input or a constant | `Action <name> has an invalid body. Map each body key to one of the call's input keys or to a constant written as an object with "value", at most 32.` |
| A body value that names no input | `Action <name> sends body key <key> from <input>, which is not one of its input bindings.` |
| An `auth` with both or neither of `secret` and `key`, or another key | `Action <name> has an invalid auth. Name a secret with "secret" to sign the request, or with "key" to send the API key it holds.` |
| A secret that is not a secret name | `Action <name> names an invalid auth secret. Use lowercase letters, digits and "_", starting with a letter.` |
| A header the Kernel sets itself, or not 1 to 64 letters, digits and `-` | `Action <name> has an invalid signature header.` |
| A `then` that is not `{ "action", "inputs" }` of strings | `Action <name> has an invalid then. Name a mutation and map its input keys to JSON pointers into the answer.` |
| A pointer outside the document | `Action <name> reads <pointer> for then, which is not a JSON pointer into /ok, /status, /error, /body or /input.` |
| `/input/<key>` for an input the call lacks | `Action <name> reads <pointer> for then, but <key> is not one of its input bindings.` |
| A `then` action that is missing, not a mutation, routed, or binds the actor | `Action <name> names <target> as its then, which is not declared.`, `... which is not a mutation.`, `... which answers on an HTTP route. A then mutation runs only after its call.`, `... which binds the actor. A then mutation runs with no caller.` |
| A `then` input the mutation lacks, or a required one left out | `Action <name> fills <key>, which is not an input of <target>.`, `Action <name> leaves <key> of <target> unfilled. Fill it from the answer or make the input nullable.` |

The editor preview answers a call with `{ callId, queued: true }` and sends nothing. Try calls on `konae dev` or a Deployment.

## Read next

- [Receive a webhook](../guides/receive-a-webhook.md)
- [Call another service](../guides/call-another-service.md)
- [Upload files](../guides/upload-files.md)
- [Limits](../limits.md)
