# Add a Server Action

Declare a named query or mutation in `actions.json`, call it from the page with `miniapp.actions.call`, and keep the SQL on the Kernel.

## How it works

A Server Action is the only way a page reaches the database. It runs one SQL statement. A query starts with `SELECT` and a mutation starts with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`. The browser sends only the action's name and its input. The SQL and the typed bindings stay in `actions.json`, and the Kernel runs them.

## Steps

1. Declare the table in `schema.sql`. This one is from the starter app.

    ```sql
    CREATE TABLE IF NOT EXISTS notes (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      title TEXT NOT NULL CHECK (length(trim(title)) BETWEEN 1 AND 120),
      body TEXT NOT NULL DEFAULT '',
      created_at INTEGER NOT NULL
    );
    ```

2. Add a mutation to `actions.json`. Each `?n` in the statement takes the binding at position n. A binding comes from the caller's `input`, from the `actor` (its `id`) or from the `kernel` (`now`). Mark an input `nullable` to bind a missing value as SQL NULL.

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
    "notes.list": {
      "kind": "query",
      "access": "authenticated",
      "statement": "SELECT id, title, body, created_at FROM notes WHERE user_id = ?1 ORDER BY id DESC LIMIT 20",
      "bindings": [
        { "source": "actor", "field": "id" }
      ]
    }
    ```

4. Choose the `access`. `authenticated` needs an app session. `anonymous` also runs for a signed-out visitor on a public Deployment, and the statement still decides which rows that visitor reaches.
5. Call the actions from the page. A mutation answers `{ changes, lastInsertId }`. Read the changed rows with a query, because the Gate and the Kernel refuse `RETURNING` on a mutation.

    ```js
    await miniapp.actions.call("notes.add", { title: "Buy milk" });
    const rows = await miniapp.actions.call("notes.list");
    ```

6. Apply the change in the editor, or save the files and run `bun run konae -- dev --dir <folder>`. The Validation Gate and the Kernel refuse a bad statement, name, access, kind or binding with the same words. A project declares at most 100 actions. When `app/src/` holds TypeScript, the folder's build checks each `miniapp.actions.call` against `actions.d.ts`, which `dev` and `contract` write, and stops on a name or input the action does not have.

## Check it worked

- Call the action through the door the page uses. On the runtime endpoint that is `actions_invoke`, which is its own read-back, so call the query after the mutation and look for the row.
- After a save over MCP, read the sources back with `project_sources_read`. After `release_apply`, read the Deployment back with `project_inspect`.
- Write the Action Contract to see the inputs and returned columns of every action.

    ```sh
    bun run konae -- contract --dir <folder>
    ```

    It writes `action-contract.json`, its schema, `action-contract.md` and `actions.d.ts` from `actions.json` and `schema.sql`.

The editor preview runs actions against its own test rows, not the Deployment's database. It has no Usage store, so read Usage on `konae dev` or a Deployment.

## Read next

- [Refuse a write with guard](./refuse-a-write-with-guard.md)
- [Receive a webhook](./receive-a-webhook.md)
- [Server Actions](../server-actions.md)
- [Limits](../limits.md)
