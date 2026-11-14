# Receive a webhook

Declare an `http` route on an `anonymous` action, name who may call it, and set the signing secret or issue an API key.

## How it works

An action declared `anonymous` may declare one HTTP route, a method and a path on its Deployment host. A `GET` route binds a query and a `POST` route binds a mutation with a JSON object body. An `authenticated` action may not declare a route. This is the inbound door. To make the app call another service, see [Call another service](./call-another-service.md).

The route's `callers` list names who may use it, any of `"signature"` (a service that signs the request), `"key"` (an API key that names the action) and `"session"` (a signed-in app user). Without the list, a signed route takes a signature or a key, an unsigned route on a Private Deployment takes a key or a session, and an unsigned route on a Public Deployment answers anyone.

## Steps

1. Declare the route with a signature. This mutation takes a status report from a service that signs with Konae's own scheme. The secret is named here and its value is never written in `actions.json`.

    ```json
    "status.report": {
      "kind": "mutation",
      "access": "anonymous",
      "http": { "method": "POST", "path": "/hooks/status", "signature": { "header": "X-Starter-Signature", "secret": "status_webhook" } },
      "statement": "INSERT INTO status (component, state, detail, updated_at) VALUES (?1, ?2, COALESCE(?3, ''), ?4) ON CONFLICT (component) DO UPDATE SET state = excluded.state, detail = excluded.detail, updated_at = excluded.updated_at WHERE status.state IS NOT excluded.state OR status.detail IS NOT excluded.detail",
      "bindings": [
        { "source": "input", "key": "component", "type": "string", "maxLength": 40 },
        { "source": "input", "key": "state", "type": "string", "maxLength": 12 },
        { "source": "input", "key": "detail", "type": "string", "maxLength": 200, "nullable": true },
        { "source": "kernel", "field": "now" }
      ]
    }
    ```

2. For a provider that signs its own way, use a preset and map the payload with `fields`. The presets are `github`, `shopify`, `stripe` and `svix`, which Resend also uses. Each `fields` entry is a JSON path into the body, and the route leaves out body keys no input names.

    ```json
    "http": {
      "method": "POST",
      "path": "/hooks/github",
      "signature": { "provider": "github", "secret": "github_webhook" },
      "fields": { "id": "$.issue.node_id", "title": "$.issue.title", "repo": "$.repository.full_name" }
    }
    ```

3. Set the secret before you publish. A release that signs with an unset secret is refused, and the refusal names each one. The value is 16 to 512 printable characters with no spaces. Set it in the editor's Access > Integrations view, or call `secret_set` with `projectId`, `name` and `value`. For a folder published with the CLI, the first `publish` registers the project and stops at this refusal, so call `secret_set` then and publish again.
4. Publish, then give the sender the Deployment's URL plus the route path. The Kernel accepts each signed request once.
5. To let a program of yours call a route without a signature, declare `callers` as `["key"]`. This is the starter app's route.

    ```json
    "http": { "method": "POST", "path": "/api/updates", "callers": ["key"] }
    ```

    Then issue a key that names the action with `api_keys_create` and `deploymentId`, `name` and `actions`. The key is shown once. The editor's Access > Integrations view shows a curl for each route the key names.
6. Test a signed route from a shell. The sender puts `t=<unix seconds>,n=<nonce>,v1=<hex HMAC-SHA256>` in the declared header. The MAC covers the time, the nonce, the method, the path, the query string and the raw body, joined by newlines.

    ```sh
    body='{"id":"A-1","total":12}'
    t=$(date +%s)
    n=$(openssl rand -hex 12)
    sig=$(printf '%s\n%s\n%s\n%s\n%s\n%s' "$t" "$n" POST /hooks/orders "" "$body" | openssl dgst -sha256 -hmac "$SECRET" -hex | sed 's/^.* //')
    curl -X POST "https://orders.example.net/hooks/orders" \
      -H 'Content-Type: application/json' -H "X-Signature: t=$t,n=$n,v1=$sig" --data-binary "$body"
    ```

    Change the host, the path and the header name to match your route.

## What the Kernel answers

A route answers `{ result }` with the rows or `{ changes, lastInsertId }`, and `{ error }` with a status code. A repeated signature answers 409 `replayed-signature`. A `t` more than 300 seconds from the Kernel's clock answers 401 `stale-timestamp`. A guard's refusal answers 403. An action whose route does not answer anyone answers only on that route, never through `miniapp.actions.call`.

## Check it worked

- Send one real or test request, then read the row it wrote through a query action. `actions_invoke` is its own read-back.
- After `secret_set`, read the names back with `secrets_list`. It shows names, uses and update times, never values.
- After `api_keys_create` or `api_keys_revoke`, read the keys back with `api_keys_list`.
- After `release_apply`, read the Deployment back with `project_inspect`. Usage counts the route by method and path, and `usage_read` reads it. On `konae dev`, `bun run konae -- events --dir <folder>` lists each route call.

The editor preview has no HTTP routes. Try routes on `konae dev` or on a published Deployment.

## Read next

- [Call another service](./call-another-service.md)
- [Add a Server Action](./add-a-server-action.md)
- [Accounts and access](../accounts-and-access.md)
- [Server Actions](../server-actions.md)
