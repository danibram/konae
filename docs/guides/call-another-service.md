# Call another service

Declare an action of `"kind": "call"` and the Kernel sends the request from its own process, retries it and runs a `then` mutation with the answer.

## How it works

A call action runs no SQL. The page invokes it with `miniapp.actions.call`, and it answers `{ callId, queued: true }`. The Kernel then sends the request, never the browser, so no secret reaches the page. This is the outbound door. For a service that calls your app, see the guide on webhooks.

A URL is https, or http to an app of this installation. It is never a private address, the editor, or a path under `/_konae`, `/oauth`, `/.well-known` or `/mcp`.

## Steps

1. Declare the call. It takes input bindings only, a `method` of `GET` or `POST`, a `url`, and a `body` that maps each body key to an input key or to a constant written `{ "value": ... }`. This is the starter app's call to its inbox app.

    ```json
    "status.escalate": {
      "kind": "call",
      "access": "authenticated",
      "method": "POST",
      "url": "http://inbox.localhost/hooks/events",
      "bindings": [
        { "source": "input", "key": "component", "type": "string", "maxLength": 40 },
        { "source": "input", "key": "changedAt", "type": "number" },
        { "source": "input", "key": "eventId", "type": "string", "maxLength": 100 },
        { "source": "input", "key": "severity", "type": "string", "maxLength": 10 },
        { "source": "input", "key": "title", "type": "string", "maxLength": 200 }
      ],
      "body": { "source": { "value": "starter" }, "id": "eventId", "severity": "severity", "title": "title" },
      "auth": { "secret": "inbox_events", "header": "X-Inbox-Signature" },
      "then": {
        "action": "status.escalated",
        "inputs": { "component": "/input/component", "changedAt": "/input/changedAt", "status": "/status", "filed": "/body/result/changes", "error": "/body/error", "failure": "/error" }
      }
    }
    ```

    For an outside service, use an `https://` URL. `http://` works only for an app of this installation.
2. Choose `auth`. `{ "secret": "<name>", "header": "X-Inbox-Signature" }` signs the request with a signing secret, in `X-Konae-Signature` unless `header` names another. `{ "key": "<name>" }` sends the API key a secret holds as `Authorization: Bearer`.
3. Set the secret before you publish, with `secret_set` and `"use": "call"`. Without `"use": "call"` the Kernel refuses a value that another of your projects already holds as a route secret. A call secret verifies no route of its own app, so it may repeat the receiving app's route secret.

    ```json
    { "projectId": "<id>", "name": "inbox_events", "value": "<the receiving app's route secret>", "use": "call" }
    ```

4. Declare the `then` mutation. It runs once when the call ends, delivered or failed, with no actor. It must be a mutation with no HTTP route and no `actor` binding. Its inputs are read by JSON pointer from `/ok`, `/status`, `/error`, `/body` and `/input`. This one records the answer.

    ```json
    "status.escalated": {
      "kind": "mutation",
      "access": "authenticated",
      "statement": "INSERT INTO escalations (component, changed_at, status, filed, error, answered_at) VALUES (?1, ?2, ?3, ?4, COALESCE(?5, ?6), ?7)",
      "bindings": [
        { "source": "input", "key": "component", "type": "string", "maxLength": 40 },
        { "source": "input", "key": "changedAt", "type": "number" },
        { "source": "input", "key": "status", "type": "number", "nullable": true },
        { "source": "input", "key": "filed", "type": "number", "nullable": true },
        { "source": "input", "key": "error", "type": "string", "maxLength": 200, "nullable": true },
        { "source": "input", "key": "failure", "type": "string", "maxLength": 40, "nullable": true },
        { "source": "kernel", "field": "now" }
      ]
    }
    ```

5. Call it from the page like any action.

    ```js
    const { callId, queued } = await miniapp.actions.call("status.escalate", {
      component, changedAt, eventId, severity, title
    });
    ```


## Limits

- 20 calls queued per app in any 60 seconds, unless the installation sets `KONAE_CALLS_PER_MINUTE`.
- 10 seconds per attempt and 64 KiB per answer.
- 5 attempts for a network failure, a timeout, 408, 425, 429 or a 5xx, waiting 30 seconds, then 1, 2 and 4 minutes. Any other answer is final. A redirect is not followed.
- A paused app sends no call until its pause ends.

## Check it worked

- Read the outbox with `calls_read` and the Deployment's `deploymentId`. It lists calls newest first with status (`pending`, `delivered` or `failed`), attempts, URL, error kind and what the `then` mutation did.
- Query the table the `then` mutation wrote. In the starter app, that is `status.escalations`.
- After `secret_set`, the read-back is `secrets_list`. After a release, the read-back for `release_apply` is `project_inspect`.

The editor preview answers `{ callId, queued: true }` and sends nothing, and it has no call outbox. Run calls on `konae dev` or on a published Deployment. Pending calls stay on the installation and do not travel in a package.

## Read next

- [Receive a webhook](./receive-a-webhook.md)
- [Add a Server Action](./add-a-server-action.md)
- [Server Actions](../server-actions.md)
- [Preview and Deployment differences](../divergences.md)
