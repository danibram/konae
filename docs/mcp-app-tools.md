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

Turn on the app's `mcp` module after `actions` and `auth`. The deployment keeps one MCP endpoint at `/_konae/mcp`. On a path address, include the app base path, such as `/p/<app-id>/_konae/mcp`. A generated hostname or custom domain uses `https://<app-host>/_konae/mcp`.

## Connect

The endpoint currently accepts an existing app session cookie. A signed-in app user can call `initialize`, then `tools/list`, then `tools/call` with the published tool name and its input object. The Kernel checks the session on every request, so ending the app session ends MCP access.

App-scoped OAuth discovery, consent, authorization-code exchange and dynamic client registration are not implemented in this slice. The Owner OAuth endpoints grant Owner authority and must not be used for an app MCP client. External clients need the app-scoped OAuth flow before this endpoint is ready for general connection.
