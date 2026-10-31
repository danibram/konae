# Walkthrough: from a credential to a migration over MCP

This page takes one app through every operation an external agent performs on Konae: get an Agent Credential, create a project, run it on a Development Kernel, give it a second page, a signed webhook, an API key, a guarded mutation, a Signal and a private file, publish it with a signup policy, invite a member, check it against its Action Contract, read its Usage and Signals, export it, import it on a second Kernel, and begin and cancel a migration.

Every command and every answer below was recorded from real Kernels by `tests/mcp-walkthrough.integration.test.ts`, which fails when this page differs from what the Kernel answers. Values that change on every run appear as placeholders in angle brackets, such as `<project id>`. Kernel A answers on `http://localhost:4173`, Kernel B on `http://localhost:4174`, and the Development Kernel on `http://localhost:8796`. An app's host is `<slug>-<suffix>.localhost` on its Kernel's port.

A tool call is shown as the `params` of a JSON-RPC `tools/call` request to `<Kernel URL>/mcp` with `Authorization: Bearer <token>`, followed by its answer. The answer is the tool's `structuredContent` as `{ "result": ... }`, a tool error as `{ "refused": "<text>" }`, or a JSON-RPC error as `{ "error": "<message>" }`. Some long answers show only the fields the step needs. [Develop with authenticated MCP](mcp.md) explains every tool, and [the miniapp SDK](sdk.md) every call an app makes.

## 1. Get an Agent Credential with an invitation code

A new creator needs an invitation code from the installation's operator. The operator issues one on the server, where `KONAE_DATA_DIR` names the Kernel's data:

<!-- walkthrough:operator -->
```sh
konae-kernel creator-code-create --note 'Walkthrough agent'
```

```text
Invitation code, shown once: <invitation code>
ID: <code id>
Expires: <expiry>
It creates one creator account. The setup key no longer creates accounts on this installation.
```
<!-- /walkthrough:operator -->

`konae login` creates the account with that code and saves a 24-hour token in the folder. It always asks for `read`, `write` and `publish`. A delegated token cannot add a permission to itself later, so ask now for every permission the work needs: `--manage-users` to invite people, `--export-data` to export, `--manage-lifecycle` to migrate.

<!-- walkthrough:login -->
```sh
KONAE_EMAIL=agent@walkthrough.test KONAE_PASSWORD=walkthrough-agent-password KONAE_INVITATION_CODE=<invitation code> bun run konae -- login --dir /tmp/orders --url http://localhost:4173 --manage-users --export-data --manage-lifecycle
```

```text
Saved a 24-hour MCP credential in .konae/credentials.json. Keep .konae out of Git.
```
<!-- /walkthrough:login -->

`session_inspect` shows what the token holds. The token is in `/tmp/orders/.konae/credentials.json`; `init`, `publish` and `export` read it from there, and `KONAE_MCP_TOKEN` overrides it.

<!-- walkthrough:session -->
```json
{
  "name": "session_inspect",
  "arguments": {}
}
```

```json
{
  "result": {
    "browserSession": false,
    "can": {
      "createTokens": false,
      "export_data": true,
      "manage_lifecycle": true,
      "manage_users": true,
      "publish": true,
      "read": true,
      "write": true
    },
    "displayName": null,
    "ownerId": "<owner id>",
    "projectIds": [],
    "scopes": [
      "read",
      "write",
      "publish",
      "manage_users",
      "export_data",
      "manage_lifecycle"
    ]
  }
}
```
<!-- /walkthrough:session -->

## 2. Read what the platform says about itself

`platform_describe` lists the yardstick, first run, workflow and guide topics. `platform_guide` returns one topic; start with `rules`, then `pieces`, then `credentials` for permissions.

<!-- walkthrough:describe -->
```json
{
  "name": "platform_describe",
  "arguments": {}
}
```

```json
{
  "result": {
    "chunkBytes": 65536,
    "files": {
      "maxFileBytes": 2097152,
      "quotaBytes": 20971520
    },
    "firstRun": "platform_guide topic rules, then pieces, then credentials. Publish with the publishing topic or bun run konae -- publish, then project_inspect and one actions_invoke.",
    "guideTopics": [
      "rules",
      "pieces",
      "credentials",
      "development",
      "actions",
      "sdk",
      "publishing",
      "users",
      "portability",
      "refusals"
    ],
    "localDevelopment": "Use bun run konae -- init, dev, build, contract, login, publish, pull, sql, conform or export; the development topic of platform_guide lists them. Development uses an isolated local Kernel. All platform operations use authenticated MCP. On a public app, actions declared anonymous run without a session, and visitors can list and read files uploaded as public. Uploads and private files require sign-in. Private apps require sign-in for everything.",
    "maxTransferBytes": 33554432,
    "name": "Konae",
    "protocolVersion": "2025-11-25",
    "runtimeEndpoint": "/_konae/mcp",
    "runtimeTools": [
      "auth_session",
      "actions_invoke",
      "files_list",
      "files_read",
      "files_upload",
      "files_update",
      "files_delete"
    ],
    "workflow": [
      "platform_guide topic rules",
      "platform_guide topic pieces",
      "a credential: platform_guide topic credentials",
      "project_create",
      "secret_set for each secret a route signs with, and google_client_id and google_client_secret when the app offers Google sign-in",
      "build locally",
      "transfer_begin/write/complete",
      "release_plan",
      "release_apply",
      "project_inspect"
    ],
    "yardstick": "A person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged."
  }
}
```
<!-- /walkthrough:describe -->

<!-- walkthrough:guide -->
```json
{
  "name": "platform_guide",
  "arguments": {
    "topic": "credentials"
  }
}
```

```json
{
  "result": {
    "flags": {
      "--export-data": "adds export_data",
      "--manage-lifecycle": "adds manage_lifecycle",
      "--manage-users": "adds manage_users"
    },
    "login": "bun run konae -- login --dir <folder> --url <Kernel URL> reads KONAE_EMAIL and KONAE_PASSWORD. Add KONAE_INVITATION_CODE, the code the operator issued with konae-kernel creator-code-create, to create the account, or KONAE_SETUP_KEY for an installation's first owner. It saves a 24-hour token with read, write and publish in <folder>/.konae/credentials.json, which publish, pull, sql and export read. KONAE_MCP_TOKEN overrides that file. On a Kernel that verifies email, creating the account sends a link; on a development Kernel on the same machine, --verify-from <data folder>/mailbox.jsonl answers it with KONAE_PASSWORD in the same run.",
    "oauth": "An MCP client with OAuth reads /.well-known/oauth-authorization-server, registers at /oauth/register, and sends the owner to /oauth/authorize with PKCE S256, resource <origin>/mcp and a scope of read, write, publish and manage_users. The token lasts one hour and there is no refresh token. OAuth does not offer export_data or manage_lifecycle.",
    "permissions": {
      "export_data": "package_export, project_export, database_export, and migration_begin together with manage_lifecycle",
      "manage_lifecycle": "the migration tools",
      "manage_users": "signup_policy_set, sign_in_methods_set, app_users_list, app_user_suspend, app_user_resume and the app_invitations tools",
      "publish": "release_plan, release_apply, secret_set, secret_delete, api_keys_create, api_keys_revoke, and initialSignupPolicy on a first release",
      "read": "projects_list, project_inspect, signup_policy_read, sign_in_methods_read, secrets_list, api_keys_list, usage_read, signals_read, calls_read, notices_read, migration_status",
      "write": "project_create, project_sources_save, project_delete, schema_plan, and database_execute changes"
    },
    "refusals": "A tool this credential may not call answers 'Unknown tool or insufficient permission.' and names the permission it needs. A delegated token cannot create tokens or add permissions to itself, so run login again with the flag. session_inspect lists scopes and can for what the current credential holds."
  }
}
```
<!-- /walkthrough:guide -->

## 3. Or get an OAuth credential

An MCP client that speaks OAuth registers itself, sends the owner to the consent screen with PKCE, and exchanges the code for a one-hour token. Registration takes a name and the redirect URIs:

<!-- walkthrough:oauth-register -->
```sh
curl -i -X POST 'http://localhost:4173/oauth/register' -H 'content-type: application/json' --data '{"client_name":"Walkthrough agent","redirect_uris":["http://127.0.0.1:9999/callback"]}'
```

```text
HTTP 201
content-type: application/json

{"client_id":"<client id>","client_name":"Walkthrough agent","grant_types":["authorization_code"],"redirect_uris":["http://127.0.0.1:9999/callback"],"response_types":["code"],"token_endpoint_auth_method":"none"}
```
<!-- /walkthrough:oauth-register -->

A signed-out owner is sent to sign in first, and `returnTo` brings them back. Signed in, the consent screen names the client and what the token grants, and approving redirects to the client with a code:

<!-- walkthrough:oauth-authorize -->
```sh
curl -i 'http://localhost:4173/oauth/authorize?client_id=<client id>&redirect_uri=http%3A%2F%2F127.0.0.1%3A9999%2Fcallback&response_type=code&code_challenge_method=S256&code_challenge=<code challenge>&scope=read+write+publish+manage_users&resource=http%3A%2F%2Flocalhost%3A4173%2Fmcp&state=walkthrough'
```

```text
HTTP 303
location: /_konae/owner/login?returnTo=%2Foauth%2Fauthorize%3F<the same parameters, URL-encoded>
```

```text
Allow access for 127.0.0.1:9999?
Approving sends an access token to 127.0.0.1:9999. Only continue if you started this sign-in from an app you trust at that address.
The client calls itself "Walkthrough agent". Clients register themselves, so Konae has not verified this name.
The token grants read write publish manage_users access to all of your projects for one hour. You can revoke it from Platform access.
```

```sh
curl -i -X POST 'http://localhost:4173/oauth/authorize' -H 'cookie: konae_owner_session=<owner session>' --data '<the same parameters>&csrf=<csrf>'
```

```text
HTTP 303
location: http://127.0.0.1:9999/callback?code=<authorization code>&state=walkthrough&iss=http%3A%2F%2Flocalhost%3A4173
```
<!-- /walkthrough:oauth-authorize -->

The client exchanges the code with its verifier:

<!-- walkthrough:oauth-token -->
```sh
curl -i -X POST 'http://localhost:4173/oauth/token' -H 'content-type: application/x-www-form-urlencoded' --data 'grant_type=authorization_code&client_id=<client id>&redirect_uri=http%3A%2F%2F127.0.0.1%3A9999%2Fcallback&code=<authorization code>&resource=http%3A%2F%2Flocalhost%3A4173%2Fmcp&code_verifier=walkthrough-verifier-0123456789-0123456789-0123456789'
```

```text
HTTP 200
content-type: application/json

{"access_token":"<token>","expires_in":3600,"scope":"read write publish manage_users","token_type":"Bearer"}
```

```json
{
  "name": "session_inspect",
  "arguments": {}
}
```

```json
{
  "result": {
    "browserSession": false,
    "can": {
      "createTokens": false,
      "export_data": false,
      "manage_lifecycle": false,
      "manage_users": true,
      "publish": true,
      "read": true,
      "write": true
    },
    "displayName": null,
    "ownerId": "<owner id>",
    "projectIds": [],
    "scopes": [
      "read",
      "write",
      "publish",
      "manage_users"
    ]
  }
}
```
<!-- /walkthrough:oauth-token -->

OAuth offers `read`, `write`, `publish` and `manage_users`. Exporting and migration need a token from `konae login`:

<!-- walkthrough:oauth-scope -->
```sh
curl -i 'http://localhost:4173/oauth/authorize?client_id=<client id>&redirect_uri=http%3A%2F%2F127.0.0.1%3A9999%2Fcallback&response_type=code&code_challenge_method=S256&code_challenge=<code challenge>&scope=read+export_data&resource=http%3A%2F%2Flocalhost%3A4173%2Fmcp&state=walkthrough' -H 'cookie: konae_owner_session=<owner session>'
```

```text
HTTP 400
content-type: application/json

{"error":"invalid_request","error_description":"Invalid scope. Request one or more of read, write, publish and manage_users, separated by spaces. OAuth does not offer export_data or manage_lifecycle; run konae login with --export-data or --manage-lifecycle for a token that holds them."}
```
<!-- /walkthrough:oauth-scope -->

## 4. Create the project

`init` writes a template and `konae.json` into an empty folder. It makes the project ID itself and calls no Kernel, so it needs no credential. The first `publish` registers the project. Dotfiles, such as the `.konae` folder `login` wrote, do not count. The project starts private, and its `signupPolicy` admits only people the owner invites.

<!-- walkthrough:init -->
```sh
bun run konae -- init --dir /tmp/orders --name Orders --url http://localhost:4173
```

```text
Created Orders in /tmp/orders. Run it with: bun run konae -- dev --dir /tmp/orders
```

```sh
cat /tmp/orders/konae.json
```

```json
{
  "url": "http://localhost:4173",
  "project": {
    "id": "<project id>",
    "name": "Orders",
    "slug": "orders",
    "manifest": {
      "version": 1,
      "visibility": "private",
      "signupPolicy": {
        "mode": "invitation"
      }
    }
  }
}
```

```sh
ls /tmp/orders
```

```text
actions.json
app.js
index.html
konae.json
schema.sql
styles.css
```
<!-- /walkthrough:init -->

## 5. Run it on a Development Kernel

`dev` starts a Kernel with its own data in `.konae/dev-data`, publishes the folder there as Open so test accounts can sign up, prints the app's URL and republishes when a file changes. Stop it with Ctrl-C.

<!-- walkthrough:dev -->
```sh
bun run konae -- dev --dir /tmp/orders --port 8796
```

```text
http://orders-<dev suffix>.localhost:8796
```

```sh
curl -i 'http://orders-<dev suffix>.localhost:8796/'
```

```text
HTTP 200
content-type: text/html; charset=utf-8
```
<!-- /walkthrough:dev -->

## 6. Write the app

The app keeps orders that a shop posts to a signed webhook. Members sign in, see the orders and claim one; claiming an order twice is refused by the guard. An accounting program reads the orders with an API key. `pages/about.html` is the second page and answers at `/about`.

<!-- walkthrough:files -->
```sh
cat /tmp/orders/index.html
```

```html
<main>
  <h1>Orders</h1>
  <p><a href="about">About this app</a></p>
  <button id="claim">Claim the oldest open order</button>
  <ul id="orders"></ul>
</main>
```

```sh
cat /tmp/orders/pages/about.html
```

```html
<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>About Orders</title><link rel="stylesheet" href="styles.css"></head>
<body><main><h1>About</h1><p>Orders arrive from the shop's webhook. Members claim them.</p><p><a href="./index.html">Back</a></p></main></body>
</html>
```

```sh
cat /tmp/orders/styles.css
```

```css
body { font: 16px system-ui; max-width: 48rem; margin: 3rem auto; padding: 1rem; }
```

```sh
cat /tmp/orders/app.js
```

```js
const list = document.querySelector("#orders");
const user = await miniapp.auth.session();
if (user.role === "anonymous") await miniapp.auth.signIn();
async function show() {
  const orders = await miniapp.actions.call("orders.list");
  list.replaceChildren(...orders.map((order) =>
    Object.assign(document.createElement("li"), { textContent: `${order.external_id}: ${order.total} (${order.status})` })));
}
document.querySelector("#claim").addEventListener("click", async () => {
  const [open] = (await miniapp.actions.call("orders.list")).filter((order) => order.status === "open");
  if (!open) return;
  try { await miniapp.actions.call("orders.claim", { order: open.id }); } catch (error) { alert(error.message); }
  await show();
});
await show();
```

```sh
cat /tmp/orders/schema.sql
```

```sql
CREATE TABLE IF NOT EXISTS orders(
  id INTEGER PRIMARY KEY,
  external_id TEXT NOT NULL UNIQUE,
  total REAL NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  claimed_by TEXT
);
```

```sh
cat /tmp/orders/actions.json
```

```json
{
  "version": 1,
  "actions": {
    "orders.receive": {
      "kind": "mutation",
      "access": "anonymous",
      "http": {
        "method": "POST",
        "path": "/hooks/orders",
        "signature": {
          "header": "X-Signature",
          "secret": "orders_webhook"
        }
      },
      "statement": "INSERT INTO orders (external_id, total) VALUES (?1, ?2) ON CONFLICT (external_id) DO NOTHING",
      "bindings": [
        {
          "source": "input",
          "key": "id",
          "type": "string",
          "maxLength": 64
        },
        {
          "source": "input",
          "key": "total",
          "type": "number"
        }
      ],
      "signal": {
        "name": "order.received",
        "properties": [
          "id"
        ]
      }
    },
    "orders.feed": {
      "kind": "query",
      "access": "anonymous",
      "http": {
        "method": "GET",
        "path": "/api/orders",
        "callers": [
          "key"
        ]
      },
      "statement": "SELECT id, external_id, total, status FROM orders WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT 100",
      "bindings": [
        {
          "source": "input",
          "key": "after",
          "type": "number",
          "nullable": true
        }
      ]
    },
    "orders.list": {
      "kind": "query",
      "access": "authenticated",
      "statement": "SELECT id, external_id, total, status FROM orders ORDER BY id LIMIT 50",
      "bindings": []
    },
    "orders.claim": {
      "kind": "mutation",
      "access": "authenticated",
      "guard": "This order is already claimed.",
      "statement": "UPDATE orders SET status = 'claimed', claimed_by = ?2 WHERE id = ?1 AND status = 'open'",
      "bindings": [
        {
          "source": "input",
          "key": "order",
          "type": "number"
        },
        {
          "source": "actor",
          "field": "id"
        }
      ],
      "signal": {
        "name": "order.claimed",
        "properties": [
          "order",
          "actor"
        ]
      }
    }
  }
}
```
<!-- /walkthrough:files -->

`contract` writes the Action Contract beside `konae.json`: `action-contract.json`, its JSON Schema, and `action-contract.md` to read.

<!-- walkthrough:contract -->
```sh
bun run konae -- contract --dir /tmp/orders
```

```text
Wrote the Action Contract for 4 actions to /tmp/orders/action-contract.json, with its schema and Markdown.
```
<!-- /walkthrough:contract -->

## 7. Set the signing secret, then publish

`orders.receive` signs with `orders_webhook`, which the project does not have yet, so the first publish is refused with what to do:

<!-- walkthrough:publish-refused -->
```sh
bun run konae -- publish --dir /tmp/orders
```

```text
Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.
```
<!-- /walkthrough:publish-refused -->

A signing secret belongs to the project, never leaves the Kernel and is not part of the folder. Set it with `secret_set`:

<!-- walkthrough:secret -->
```json
{
  "name": "secret_set",
  "arguments": {
    "projectId": "<project id>",
    "name": "orders_webhook",
    "value": "walkthrough-orders-secret-0123"
  }
}
```

```json
{
  "result": {
    "name": "orders_webhook",
    "projectId": "<project id>",
    "updatedAt": <time ms>,
    "use": "route"
  }
}
```
<!-- /walkthrough:secret -->

`publish` builds the folder, uploads the release and applies it. On the first release it sends `project.manifest.signupPolicy` from `konae.json` as `initialSignupPolicy`, so the app starts as Invitation only. A visitor without a session gets the sign-in page on every path of a private app.

<!-- walkthrough:publish -->
```sh
bun run konae -- publish --dir /tmp/orders
```

```text
http://orders-<suffix>.localhost:4173
```

```json
{
  "name": "signup_policy_read",
  "arguments": {
    "deploymentId": "<deployment id>"
  }
}
```

```json
{
  "result": {
    "allowlist": [],
    "deploymentId": "<deployment id>",
    "mode": "invitation",
    "visibility": "private"
  }
}
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/about'
```

```text
HTTP 200
content-type: text/html; charset=utf-8
```
<!-- /walkthrough:publish -->

## 8. Invite a member

Inviting needs `manage_users`. A token from `login` without `--manage-users` is refused with the permission and the flag:

<!-- walkthrough:invite-refused -->
```sh
KONAE_EMAIL=agent@walkthrough.test KONAE_PASSWORD=walkthrough-agent-password bun run konae -- login --dir /tmp/publisher --url http://localhost:4173
```

```text
Saved a 24-hour MCP credential in .konae/credentials.json. Keep .konae out of Git.
```

```json
{
  "name": "app_invitations_create",
  "arguments": {
    "deploymentId": "<deployment id>",
    "emails": [
      "member@walkthrough.test"
    ],
    "idempotencyKey": "walkthrough-invite-1"
  }
}
```

```json
{
  "error": "Unknown tool or insufficient permission. app_invitations_create needs a credential with manage_users. Run konae login with --manage-users, or use a token from token_create that includes it."
}
```
<!-- /walkthrough:invite-refused -->

Check that the installation sends email, then invite. Call `app_invitations_create` only when the owner asked for those invitations, because each one sends a message. The same `idempotencyKey` on a retry returns the first answer and sends nothing.

<!-- walkthrough:invite -->
```json
{
  "name": "mail_status",
  "arguments": {}
}
```

```json
{
  "result": {
    "transport": "development",
    "sending": true
  }
}
```

```json
{
  "name": "app_invitations_create",
  "arguments": {
    "deploymentId": "<deployment id>",
    "emails": [
      "member@walkthrough.test"
    ],
    "idempotencyKey": "walkthrough-invite-1"
  }
}
```

```json
{
  "result": {
    "deploymentId": "<deployment id>",
    "policy": "invitation",
    "results": [
      {
        "acceptable": true,
        "delivery": "queued",
        "email": "member@walkthrough.test",
        "expiresAt": <unix seconds>,
        "invitationId": "<invitation id>",
        "status": "invited"
      }
    ]
  }
}
```
<!-- /walkthrough:invite -->

The member opens the link from the email. This Kernel uses the development mailbox, which writes each message to `mailbox.jsonl` in its data folder. The page the link opens reads the invitation, then posts the chosen password, which creates the account and signs the member in:

<!-- walkthrough:accept -->
```sh
tail -n 1 /tmp/kernel-a/mailbox.jsonl
```

```json
{"to":"member@walkthrough.test","purpose":"invitation","scope":"deployment:<deployment id>","link":"http://orders-<suffix>.localhost:4173/_konae/invitation#invitation=<invitation token>","expiresAt": <unix seconds>}
```

```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/_konae/auth/invitation' -H 'content-type: application/json' --data '{"token":"<invitation token>"}'
```

```text
HTTP 200
content-type: application/json

{"result":{"app":"Orders","email":"member@walkthrough.test","status":"new-account"}}
```

```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/_konae/auth/accept' -H 'content-type: application/json' --data '{"token":"<invitation token>","password":"walkthrough-member-password"}'
```

```text
HTTP 200
content-type: application/json

{"result":{"expiresAt": <unix seconds>,"user":{"name":"member@walkthrough.test","role":"member","userId":"<member id>"}}}
```

```text
set-cookie: konae_app_session=<app session>; ...
```
<!-- /walkthrough:accept -->

## 9. Use the app as the member

The member's calls go to the app's runtime endpoint, `/_konae/mcp` on the app's host, with the app session cookie. `miniapp.auth.session()` and `miniapp.actions.call()` in `app.js` make these calls. The second page answers at `/about`, and a path the artifact does not hold answers 404:

<!-- walkthrough:runtime -->
```json
{
  "name": "auth_session",
  "arguments": {}
}
```

```json
{
  "result": {
    "name": "member@walkthrough.test",
    "role": "member",
    "userId": "<member id>"
  }
}
```

```json
{
  "name": "actions_invoke",
  "arguments": {
    "name": "orders.list"
  }
}
```

```json
{
  "result": []
}
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/about' -H 'cookie: konae_app_session=<app session>'
```

```text
HTTP 200
content-type: text/html; charset=utf-8
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/missing' -H 'cookie: konae_app_session=<app session>'
```

```text
HTTP 404
content-type: text/html; charset=utf-8
```
<!-- /walkthrough:runtime -->

The shop signs each webhook with the secret over the timestamp, a fresh nonce, the method, the path, the query and the body. `src/lib/route-signature.ts` signs, and [HTTP routes](mcp.md#signatures) gives the same in shell. The same signature twice is a replay, and an unsigned request is refused:

<!-- walkthrough:signed-route -->
```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/hooks/orders' -H 'content-type: application/json' -H 'x-signature: t=<unix seconds>,n=walkthrough-nonce-1,v1=<hmac>' --data '{"id":"A-1","total":12}'
```

```text
HTTP 200
content-type: application/json

{"result":{"changes":1,"lastInsertId":1}}
```

```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/hooks/orders' -H 'content-type: application/json' -H 'x-signature: t=<unix seconds>,n=walkthrough-nonce-1,v1=<hmac>' --data '{"id":"A-1","total":12}'
```

```text
HTTP 409
content-type: application/json

{"error":"replayed-signature"}
```

```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/hooks/orders' -H 'content-type: application/json' --data '{"id":"A-1","total":12}'
```

```text
HTTP 401
content-type: application/json

{"error":"invalid-signature"}
```
<!-- /walkthrough:signed-route -->

The member claims order 1. The second claim changes no row, so the Kernel answers the guard instead of `{ changes: 0 }`:

<!-- walkthrough:guard -->
```json
{
  "name": "actions_invoke",
  "arguments": {
    "name": "orders.claim",
    "input": {
      "order": 1
    }
  }
}
```

```json
{
  "result": {
    "changes": 1,
    "lastInsertId": 0
  }
}
```

```json
{
  "name": "actions_invoke",
  "arguments": {
    "name": "orders.claim",
    "input": {
      "order": 1
    }
  }
}
```

```json
{
  "refused": "This order is already claimed."
}
```
<!-- /walkthrough:guard -->

An upload is a transfer of at most the installation's per-file limit, 2 MiB by default, then `files_upload`. The file is private to the member who uploaded it. `miniapp.files.url(fileId)` returns its path, `/_konae/files/<id>`, which the member reads with the session cookie; a visitor of a private app gets 401 before any file is looked up.

<!-- walkthrough:file -->
```json
{
  "name": "transfer_begin",
  "arguments": {
    "size": 19,
    "sha256": "<sha256>"
  }
}
```

```json
{
  "result": {
    "chunkBytes": 65536,
    "expiresAt": <time ms>,
    "sha256": "<sha256>",
    "size": 19,
    "transferId": "<transfer id>"
  }
}
```

```json
{
  "name": "transfer_write",
  "arguments": {
    "transferId": "<transfer id>",
    "offset": 0,
    "data": "SW52b2ljZSBBLTE6IDEyLjAwCg=="
  }
}
```

```json
{
  "result": {
    "nextOffset": 19
  }
}
```

```json
{
  "name": "transfer_complete",
  "arguments": {
    "transferId": "<transfer id>"
  }
}
```

```json
{
  "result": {
    "complete": true,
    "sha256": "<sha256>",
    "transferId": "<transfer id>"
  }
}
```

```json
{
  "name": "files_upload",
  "arguments": {
    "transferId": "<transfer id>",
    "name": "invoice-a1.txt",
    "type": "text/plain"
  }
}
```

```json
{
  "result": {
    "createdAt": <time ms>,
    "id": "<file id>",
    "name": "invoice-a1.txt",
    "ownerId": "<member id>",
    "size": 19,
    "type": "text/plain",
    "visibility": "private"
  }
}
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/_konae/files/<file id>' -H 'cookie: konae_app_session=<app session>'
```

```text
HTTP 200
content-type: text/plain
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/_konae/files/<file id>'
```

```text
HTTP 401
content-type: application/json

{"error":"authentication-required"}
```
<!-- /walkthrough:file -->

## 10. Issue an API key

`api_keys_create` returns the key once. This key may call `orders.feed` and reads no files, so the private invoice answers 404 to it, the same as a missing file.

<!-- walkthrough:key -->
```json
{
  "name": "api_keys_create",
  "arguments": {
    "deploymentId": "<deployment id>",
    "name": "Accounting export",
    "actions": [
      "orders.feed"
    ],
    "files": []
  }
}
```

```json
{
  "result": {
    "actions": [
      "orders.feed"
    ],
    "createdAt": <time ms>,
    "files": [],
    "key": "<api key>",
    "keyId": "<key id>",
    "name": "Accounting export",
    "note": "The key appears only in this answer. Send it as Authorization: Bearer <key>."
  }
}
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/api/orders' -H 'authorization: Bearer <api key>'
```

```text
HTTP 200
content-type: application/json

{"result":[{"external_id":"A-1","id":1,"status":"claimed","total":12.0}]}
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/api/orders'
```

```text
HTTP 401
content-type: application/json

{"error":"authentication-required"}
```

```sh
curl -i 'http://orders-<suffix>.localhost:4173/_konae/files/<file id>' -H 'authorization: Bearer <api key>'
```

```text
HTTP 404
content-type: application/json

{"error":"File not found."}
```
<!-- /walkthrough:key -->

## 11. Check the app against its contract

`conform` calls every action through the runtime endpoint and the routes, and checks each answer and each refusal against `action-contract.json`. `--token` is the value of the app session cookie, `--key` an API key, and `--secret` signs the webhook. It runs each mutation once, so point it at a test Deployment.

<!-- walkthrough:conform -->
```sh
bun run konae -- conform --url http://orders-<suffix>.localhost:4173 --contract /tmp/orders/action-contract.json --token <app session> --key <api key> --secret orders_webhook=<secret>
```

```text
PASS conform.undeclared: refuses an undeclared action. Refused with "Server action not declared.".
PASS orders.claim: answers a valid call. Refused with its guard: the generated input changed no row.
PASS orders.claim: refuses with its guard when no row changes. Refused with "This order is already claimed.".
PASS orders.claim: refuses a caller without a session. Refused with "Sign in to use Konae MCP.".
PASS orders.claim: refuses an undeclared input. Refused with "Unexpected action input: conformProbe".
PASS orders.claim: refuses a missing order. Refused with "Missing action input: order".
PASS orders.claim: refuses order when it is not a number. Refused with "order must be a number.".
PASS orders.feed: answers only on its route. Refused with "Action orders.feed answers only on its HTTP route, GET /api/orders.".
PASS orders.feed: answers GET /api/orders with an API key. Answered an empty array; no row to check the columns against.
PASS orders.feed: refuses a caller without a key or a session. Answered 401 "authentication-required".
PASS orders.feed: refuses HEAD. Answered 405; a HEAD answer carries no body.
PASS orders.feed: refuses an unknown API key. Answered 401 "invalid-api-key".
PASS orders.feed: refuses an undeclared input on the route. Answered 400 "Unexpected action input: conformProbe".
PASS orders.list: answers a valid call. Answered 1 row with the contract's columns.
PASS orders.list: refuses a caller without a session. Refused with "Sign in to use Konae MCP.".
PASS orders.list: refuses an undeclared input. Refused with "Unexpected action input: conformProbe".
PASS orders.receive: answers only on its route. Refused with "Action orders.receive answers only on its HTTP route, POST /hooks/orders.".
PASS orders.receive: answers POST /hooks/orders. Answered {"changes":1,"lastInsertId":2}.
PASS orders.receive: refuses a replayed signature. Answered 409 "replayed-signature".
PASS orders.receive: refuses an unsigned request. Answered 401 "invalid-signature".
PASS orders.receive: refuses a stale timestamp. Answered 401 "stale-timestamp".
PASS orders.receive: refuses an unknown API key. Answered 401 "invalid-api-key".
PASS orders.receive: refuses an undeclared input on the route. Answered 400 "Unexpected action input: conformProbe".
PASS orders.receive: refuses a body that is not JSON. Answered 400 "The request body must be a JSON object.".
PASS routes: refuses a POST to a path no action binds. Answered 405 "No route accepts POST on this path.".
25 passed, 0 failed, 0 skipped.
```
<!-- /walkthrough:conform -->

## 12. Read Usage and Signals

`usage_read` counts what the Kernel allowed, by day, and each guard refusal under `refused`:

<!-- walkthrough:usage -->
```json
{
  "name": "usage_read",
  "arguments": {
    "deploymentId": "<deployment id>",
    "from": "<today>",
    "to": "<today>"
  }
}
```

```json
{
  "result": {
    "from": "<today>",
    "to": "<today>",
    "totals": [
      {
        "count": 1,
        "kind": "action",
        "name": "orders.claim"
      },
      {
        "count": 2,
        "kind": "action",
        "name": "orders.list"
      },
      {
        "count": 1,
        "kind": "file_read",
        "name": ""
      },
      {
        "count": 2,
        "kind": "refused",
        "name": "orders.claim"
      },
      {
        "count": 2,
        "kind": "route",
        "name": "GET /api/orders"
      },
      {
        "count": 2,
        "kind": "route",
        "name": "POST /hooks/orders"
      },
      {
        "count": 1,
        "kind": "session",
        "name": ""
      },
      {
        "count": 1,
        "kind": "signup",
        "name": ""
      },
      {
        "count": 1,
        "kind": "upload",
        "name": ""
      }
    ]
  }
}
```
<!-- /walkthrough:usage -->

`signals_read` returns the Signals the actions recorded, newest first. `order.claimed` lists `actor`, so it holds the member's ID:

<!-- walkthrough:signals -->
```json
{
  "name": "signals_read",
  "arguments": {
    "deploymentId": "<deployment id>"
  }
}
```

```json
{
  "result": {
    "nextCursor": null,
    "signals": [
      {
        "at": <time ms>,
        "name": "order.received",
        "properties": {
          "id": "<conform order>"
        }
      },
      {
        "at": <time ms>,
        "name": "order.claimed",
        "properties": {
          "actor": "<member id>",
          "order": 1
        }
      },
      {
        "at": <time ms>,
        "name": "order.received",
        "properties": {
          "id": "A-1"
        }
      }
    ]
  }
}
```
<!-- /walkthrough:signals -->

## 13. Export the app

Exporting needs `export_data`, which OAuth does not grant:

<!-- walkthrough:export-refused -->
```json
{
  "name": "package_export",
  "arguments": {
    "deploymentId": "<deployment id>"
  }
}
```

```json
{
  "error": "Unknown tool or insufficient permission. package_export needs a credential with export_data. Run konae login with --export-data, or use a token from token_create that includes it."
}
```
<!-- /walkthrough:export-refused -->

`package_export` captures the database, the accounts, the files and the Usage store under the app's locks, and returns a manifest with one download per component. Read each with `transfer_read` from offset 0 until `nextOffset` equals `size`, check the SHA-256, then `transfer_cancel` it.

<!-- walkthrough:export -->
```json
{
  "name": "package_export",
  "arguments": {
    "deploymentId": "<deployment id>"
  }
}
```

```json
{
  "result": {
    "downloads": {
      "database": {
        "chunkBytes": 65536,
        "expiresAt": <time ms>,
        "sha256": "<sha256>",
        "size": 12288,
        "transferId": "<database download>"
      },
      "files": {
        "chunkBytes": 65536,
        "expiresAt": <time ms>,
        "sha256": "<sha256>",
        "size": 3072,
        "transferId": "<files download>"
      },
      "identity": {
        "chunkBytes": 65536,
        "expiresAt": <time ms>,
        "sha256": "<sha256>",
        "size": 110592,
        "transferId": "<identity download>"
      },
      "usage": {
        "chunkBytes": 65536,
        "expiresAt": <time ms>,
        "sha256": "<sha256>",
        "size": 57344,
        "transferId": "<usage download>"
      }
    },
    "manifest": {
      "artifactVersion": "<artifact version>",
      "contents": [
        {
          "name": "database",
          "sha256": "<sha256>",
          "size": 12288
        },
        {
          "name": "identity",
          "sha256": "<sha256>",
          "size": 110592
        },
        {
          "name": "files",
          "sha256": "<sha256>",
          "size": 3072
        },
        {
          "name": "usage",
          "sha256": "<sha256>",
          "size": 57344
        }
      ],
      "createdAt": <time ms>,
      "deploymentId": "<deployment id>",
      "excluded": [
        "app sessions",
        "verification, recovery and invitation links",
        "owner accounts, owner sessions and MCP credentials",
        "Data panel backups",
        "operator email and storage keys"
      ],
      "format": "konae-package",
      "packageId": "<package id>",
      "projectId": "<project id>",
      "publishedRevision": 1,
      "signInMethods": [
        "password"
      ],
      "signupPolicy": {
        "allowlist": [],
        "mode": "invitation"
      },
      "sourcesRevision": null,
      "version": 1,
      "visibility": "private"
    }
  }
}
```

```json
{
  "name": "transfer_read",
  "arguments": {
    "transferId": "<identity download>",
    "offset": 0
  }
}
```

```json
{
  "result": {
    "data": "<base64 of the first 65536 bytes>",
    "nextOffset": 65536,
    "offset": 0,
    "sha256": "<sha256>",
    "size": 110592
  }
}
```
<!-- /walkthrough:export -->

## 14. Import on a second Kernel

The destination needs its own credential, the project under the same ID, and the signing secret, which does not travel. Without it, `release_plan` refuses the release as in step 7.

<!-- walkthrough:second-kernel -->
```sh
KONAE_EMAIL=agent@walkthrough.test KONAE_PASSWORD=walkthrough-agent-password KONAE_SETUP_KEY=walkthrough-second-installation-key bun run konae -- login --dir /tmp/orders-b --url http://localhost:4174 --manage-users
```

```text
Saved a 24-hour MCP credential in .konae/credentials.json. Keep .konae out of Git.
```

```json
{
  "name": "project_create",
  "arguments": {
    "projectId": "<project id>",
    "name": "Orders",
    "slug": "orders",
    "manifest": {
      "version": 1,
      "visibility": "private"
    }
  }
}
```

```json
{
  "result": {
    "id": "<project id>",
    "name": "Orders",
    "slug": "orders"
  }
}
```

```json
{
  "name": "secret_set",
  "arguments": {
    "projectId": "<project id>",
    "name": "orders_webhook",
    "value": "walkthrough-orders-secret-0123"
  }
}
```

```json
{
  "result": {
    "name": "orders_webhook",
    "projectId": "<project id>",
    "updatedAt": <time ms>,
    "use": "route"
  }
}
```
<!-- /walkthrough:second-kernel -->

Upload each component with `transfer_begin` and its `component` name, so the upload may be as large as that component's limit. `McpClient.upload(bytes, projectId, component)` in `src/mcp/client.ts` does the three calls; the database is shown here, and the identity, files and usage components go the same way.

<!-- walkthrough:import-upload -->
```json
{
  "name": "transfer_begin",
  "arguments": {
    "size": 12288,
    "sha256": "<sha256>",
    "projectId": "<project id>",
    "component": "database"
  }
}
```

```json
{
  "result": {
    "chunkBytes": 65536,
    "expiresAt": <time ms>,
    "sha256": "<sha256>",
    "size": 12288,
    "transferId": "<database upload>"
  }
}
```

```json
{
  "name": "transfer_write",
  "arguments": {
    "transferId": "<database upload>",
    "offset": 0,
    "data": "<base64 of up to 65536 bytes from offset 0>"
  }
}
```

```json
{
  "result": {
    "nextOffset": 12288
  }
}
```

```json
{
  "name": "transfer_complete",
  "arguments": {
    "transferId": "<database upload>"
  }
}
```

```json
{
  "result": {
    "complete": true,
    "sha256": "<sha256>",
    "transferId": "<database upload>"
  }
}
```
<!-- /walkthrough:import-upload -->

Build the folder, then upload a release payload: the project, its visibility, the artifact from `.konae/artifact.json`, and `initialPackage` with the manifest and the component uploads. `release_plan` checks every component before anything is created, and `release_apply` installs them. The signup policy comes from the manifest.

<!-- walkthrough:import-release -->
```sh
bun run konae -- build --dir /tmp/orders
```

```text
Built .konae/artifact.json
```

```json
{
  "project": {
    "id": "<project id>",
    "name": "Orders",
    "slug": "orders",
    "manifest": {
      "version": 1,
      "visibility": "private"
    },
    "files": "<path: text of every source file>"
  },
  "visibility": "private",
  "artifact": "<the JSON in .konae/artifact.json>",
  "initialPackage": {
    "manifest": "<the manifest package_export returned>",
    "components": {
      "database": "<database upload>",
      "files": "<files upload>",
      "identity": "<identity upload>",
      "usage": "<usage upload>"
    }
  }
}
```

```json
{
  "name": "release_plan",
  "arguments": {
    "transferId": "<release upload>"
  }
}
```

```json
{
  "result": {
    "artifactSha256": "<sha256>",
    "expectedRevision": 0,
    "expiresAt": <time ms>,
    "planId": "<plan id>",
    "projectId": "<project id>",
    "visibility": "private"
  }
}
```

```json
{
  "name": "release_apply",
  "arguments": {
    "planId": "<plan id>"
  }
}
```

```json
{
  "result": {
    "id": "<destination deployment id>",
    "hostname": "orders-<destination suffix>.localhost",
    "url": "http://orders-<destination suffix>.localhost:4174",
    "signupPolicy": {
      "allowlist": [],
      "mode": "invitation"
    }
  }
}
```
<!-- /walkthrough:import-release -->

Sessions and email links stay behind, so the member signs in again with the same password and finds the same rows. The API key arrives revoked, with its name and scope, so the owner knows which program needs a new one.

<!-- walkthrough:import-check -->
```sh
curl -i -X POST 'http://orders-<destination suffix>.localhost:4174/_konae/auth/login' -H 'content-type: application/json' --data '{"email":"member@walkthrough.test","password":"walkthrough-member-password"}'
```

```text
HTTP 200
content-type: application/json

{"result":{"expiresAt": <unix seconds>,"user":{"name":"member@walkthrough.test","role":"member","userId":"<member id>"}}}
```

```json
{
  "name": "actions_invoke",
  "arguments": {
    "name": "orders.list"
  }
}
```

```json
{
  "result": [
    {
      "external_id": "A-1",
      "id": 1,
      "status": "claimed",
      "total": 12
    },
    {
      "external_id": "<conform order>",
      "id": 2,
      "status": "open",
      "total": 1
    }
  ]
}
```

```json
{
  "name": "api_keys_list",
  "arguments": {
    "deploymentId": "<destination deployment id>"
  }
}
```

```json
{
  "result": {
    "deploymentId": "<destination deployment id>",
    "keys": [
      {
        "actions": [
          "orders.feed"
        ],
        "createdAt": <time ms>,
        "files": [],
        "keyId": "<key id>",
        "name": "Accounting export",
        "revokedAt": <time ms>,
        "revokedReason": "import"
      }
    ]
  }
}
```
<!-- /walkthrough:import-check -->

## 15. Begin and cancel a migration

`migration_begin` pauses the app at once and captures it as a version 2 package. Its answer lists the downloads and what comes next. While paused, the app refuses writes with 503:

<!-- walkthrough:migration-begin -->
```json
{
  "name": "migration_begin",
  "arguments": {
    "deploymentId": "<deployment id>"
  }
}
```

```json
{
  "result": {
    "operationId": "<operation id>",
    "packageId": "<migration package id>",
    "phase": "captured",
    "paused": true,
    "downloads": [
      "database",
      "files",
      "identity",
      "release",
      "usage"
    ],
    "next": [
      "Download every component and the release with transfer_read.",
      "On the destination, create the project, upload each component with transfer_begin and its component name, and release the downloaded release payload with initialPackage naming the manifest and the uploads.",
      "Check the imported app with read tools; it stays paused.",
      "Run migration_confirm here, then migration_activate on the destination with the confirmation it returns."
    ]
  }
}
```

```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/hooks/orders' -H 'content-type: application/json' --data '{}'
```

```text
HTTP 503
content-type: application/json
retry-after: 300

{"error":"This app is paused for migration <operation id>. It accepts no writes, signups, recovery, administrative changes or email until the migration is confirmed or cancelled.","maintenance":true,"operationId":"<operation id>"}
```
<!-- /walkthrough:migration-begin -->

Before `migration_confirm`, `migration_cancel` resumes the app, and the webhook is accepted again:

<!-- walkthrough:migration-cancel -->
```json
{
  "name": "migration_cancel",
  "arguments": {
    "operationId": "<operation id>"
  }
}
```

```json
{
  "result": {
    "operationId": "<operation id>",
    "effects": {
      "destination": "A package already imported elsewhere can never be activated: its confirmation was not issued and is now discarded.",
      "email": "Email ended by the pause is not sent. Resend pending invitations; people ask again for verification or recovery links.",
      "origin": "Accepts writes, signups, recovery and email again."
    }
  }
}
```

```json
{
  "name": "migration_status",
  "arguments": {
    "deploymentId": "<deployment id>"
  }
}
```

```json
{
  "result": {
    "phase": "cancelled",
    "paused": false,
    "operationId": "<operation id>"
  }
}
```

```sh
curl -i -X POST 'http://orders-<suffix>.localhost:4173/hooks/orders' -H 'content-type: application/json' -H 'x-signature: t=<unix seconds>,n=walkthrough-nonce-2,v1=<hmac 2>' --data '{"id":"A-2","total":30}'
```

```text
HTTP 200
content-type: application/json

{"result":{"changes":1,"lastInsertId":3}}
```
<!-- /walkthrough:migration-cancel -->

To finish a migration instead, import the package on the destination as in step 14 with the `release` download as the payload, run `migration_confirm` on the origin, and `migration_activate` with its confirmation on the destination. [Migrate an app](mcp.md#migrate-an-app) has the details.
