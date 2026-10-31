# Add a Server Action

Declare a named query or mutation in `actions.json`, call it from the page with `miniapp.actions.call`, and keep SQL on the Kernel.

## How it works

A Server Action is the deployed app's only database interface. It runs one SQL statement. A query starts with `SELECT` and a mutation starts with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`. The browser sends only the action name and its input. The SQL and the typed bindings stay in `actions.json` on the server.

## Steps

1. Declare the table in `schema.sql`. This is from the starter app.

    ```sql
    CREATE TABLE IF NOT EXISTS notes (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 120),
      body TEXT NOT NULL DEFAULT '',
      created_at INTEGER NOT NULL
    );
    ```

2. Add a mutation to `actions.json`. Each `?n` in the statement takes the binding at position n. A binding comes from the caller's `input`, the `actor` (its `id` or `role`) or the `kernel` (`now`). Mark an input `nullable` to bind a missing value as SQL NULL.

    ```json
    "notes.add": {
      "kind": "mutation",
      "access": "authenticated",
      "statement": "INSERT INTO notes (user_id, title, body, created_at) VALUES (?1, ?2, COALESCE(?3, ''), ?4)",
      "bindings": [
        { "source": "actor", "field": "id" },
        { "source": "input", "key": "title", "type": "string", "maxLength": 120 },
        { "source": "input", "key": "body", "type": "string", "maxLength": 4000, "nullable": true },
        { "source": "kernel", "field": "now" }
      ]
    }
    ```

3. Add a query that reads the rows back. A query that reads rows names a top-level `LIMIT`. A total with `count`, `sum`, `avg`, `min` or `max` and no `GROUP BY` does not need one.

    ```json
    "attachments.list": {
      "kind": "query",
      "access": "authenticated",
      "statement": "SELECT id, file_id, name, size, public, created_at FROM attachments WHERE user_id = ?1 ORDER BY id DESC LIMIT 50",
      "bindings": [
        { "source": "actor", "field": "id" }
      ]
    }
    ```

4. Choose the `access`. `authenticated` needs an app session. `anonymous` also runs for a signed-out visitor on a public Deployment, and the row it touches is still the app's decision.
5. Call it from the page. A mutation answers `{ changes, lastInsertId }`. Read the changed rows with a separate query, because publication rejects mutation `RETURNING`.

    ```js
    await miniapp.actions.call("notes.add", { title: "Buy milk" });
    const rows = await miniapp.actions.call("notes.list");
    ```

6. Apply the change in the editor, or save the files and run `konae dev`. The Validation Gate and the Kernel refuse a bad statement, name, access, kind or binding with the same words. A project declares at most 100 actions.

## Check it worked

- After a save over MCP, the read-back for `project_sources_save` is `project_sources_read`. After a release, the read-back for `release_apply` is `project_inspect`.
- Call the action through the door the page uses. On the runtime endpoint that is `actions_invoke`, and it is its own read-back, so call the query after the mutation and look for the row.
- Write the Action Contract and read what the Kernel took.

    ```sh
    bun run konae -- contract --dir /tmp/my-app
    ```

    It writes `action-contract.json`, its schema and `action-contract.md` from `actions.json` and `schema.sql`.

The editor preview runs actions against its own local database, not the Deployment's. It has no Usage store, so check that on `konae dev` or a Deployment.

## Read next

- [Refuse a write with guard](./refuse-a-write-with-guard.md)
- [Receive a webhook](./receive-a-webhook.md)
- [Server Actions](../server-actions.md)
- [Limits](../limits.md)
