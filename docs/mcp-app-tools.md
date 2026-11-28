# App MCP tools

An app can expose selected authenticated query Actions as read-only MCP tools. The MCP endpoint uses the active release's `actions.json`; drafts do not change the published catalog.

## Declare a tool

Add an `mcp` object with a short description to an authenticated query Action. The Action's input bindings define the MCP input schema.

```json
{
  "version": 1,
  "actions": {
    "notes.mine": {
      "kind": "query",
      "access": "authenticated",
      "statement": "SELECT id, body FROM notes WHERE user_id = ?1 LIMIT 20",
      "bindings": [{ "source": "actor", "field": "id" }],
      "mcp": { "description": "List the signed-in member's notes." }
    }
  }
}
```

The Validation Gate and Kernel accept only `mcp` declarations on authenticated queries. Mutations, anonymous Actions and other Action kinds fail validation. The runtime hides the tool when the MCP module or Actions module is off. It uses the same Action implementation, actor binding and row rules as `miniapp.actions.call`.

Follow [Turn on an app module](./guides/manage-modules.md#turn-on-an-app-module). Enable `actions` and `auth`, then enable `mcp`. Publish the declaration. The deployment keeps one MCP endpoint at `/_konae/mcp`. On a path address, include the app base path, such as `/p/<app-id>/_konae/mcp`. A generated hostname or custom domain uses `https://<app-host>/_konae/mcp`.

## Connect an external client

The endpoint is `https://<app-host>/_konae/mcp`. A path deployment includes its mount, for example `https://apps.example.com/p/<app-id>/_konae/mcp`. The same endpoint works through a generated app host or a verified custom domain. Use the app's canonical address for discovery and OAuth throughout one connection.

The installation operator registers public OAuth clients in `<KONAE_DATA_DIR>/mcp-app-clients.json`. Use the app ID from `project_inspect` or `konae.json` and list exact callback URLs. The Kernel reads the file on OAuth requests, so edits apply without a restart. Konae accepts HTTPS callbacks and loopback HTTP callbacks for local clients. This v1 flow does not use dynamic client registration or a client secret.

```json
{
  "version": 1,
  "clients": [
    {
      "clientId": "desktop-notes",
      "appId": "app_notes",
      "name": "Notes desktop client",
      "redirectUris": ["http://127.0.0.1:8765/callback"]
    }
  ]
}
```

The client discovers the protected resource at `/.well-known/oauth-protected-resource/_konae/mcp` on the app's address. Include the app base path for a path-mounted app. It then opens the advertised authorization endpoint with `response_type=code`, its registered `client_id` and `redirect_uri`, a random `state`, a PKCE S256 challenge, the exact `resource` URL, and the names of the requested tools as space-separated scopes. The app asks the user to sign in with their app account, then shows a consent page naming each requested tool. The client exchanges the returned code using its verifier and the same redirect URI and resource. The code lasts ten minutes and can be used once. Access tokens last one hour.

Use the returned bearer token at the MCP endpoint. OAuth clients see only their granted app tools, and the Kernel checks the app, canonical resource, current release catalog, client registration, token revocation and active app account on each request. Each tool call runs its published authenticated query as that app user. The Owner OAuth flow and browser SDK credentials do not grant app-user MCP access.

With an app user's bearer token, call `tools/list` and check that the client sees the tools that user granted. Call one tool and verify the rows match that user's access. The owner's `/mcp` endpoint manages the app. Use the app's `/_konae/mcp` endpoint for this check.

The operator file and token store belong to the installation, not the app folder. Export and import preserve the app's MCP Action declarations. Register the app ID on the destination installation and have each user grant access again. Removing a client registration, disabling the `mcp` module, suspending the user or revoking the token stops its access. A client can revoke a token at the discovered revocation endpoint with its `client_id`.
