# Runtime endpoint and action rules

An app's pages call the runtime endpoint, and each Server Action's statement and declaration must meet the rules the Gate and the Kernel share.

## Runtime endpoint

The SDK calls `/_konae/mcp` on the app's host with the app session. [The miniapp SDK](../sdk.md) lists every call with its answer, its refusals and its limits. That page, the `sdk` topic of `platform_guide` and the build agent's prompt come from one contract, `src/sdk/contract.ts`.

On a public app a visitor may call actions declared `anonymous` and read files uploaded as `public`. Uploads, private files and `authenticated` actions need a session. On a private app nothing loads or runs without one.

A signed-out visitor may call `actions_invoke` 30 times per 10 seconds per client address and Deployment, and each app user 120 times per 10 seconds. Past that the call answers `rate-limited` with `Retry-After`, and so does a call that finds four statements of its Deployment already running and no slot free within 2 seconds. The Kernel counts the socket's peer address and reads `X-Forwarded-For` only from peers in `KONAE_TRUSTED_PROXIES`.

Set `manifest.fileAccess` to `"app-users"` to require an app session for every file, including existing uploads. The uploader keeps deletion rights. Removing the policy restores each original file visibility. Files uploaded under the policy then become private to the uploader.

By default, a file is private to the app user who uploaded it unless the upload set `visibility` to `public`. `files_update { fileId, visibility }` and `files_delete { fileId }` work only for the uploader. A user who may not read the file gets `File not found.`, as for a missing one, and a user who reads it but did not upload it gets `Only the person who uploaded this file may change or delete it.` A deleted file stops counting against the quota. A row that still names it keeps the ID, and the app removes that row with its own action.

### Statement rules

An action runs one SQL statement. A query starts with `SELECT` and a mutation with `INSERT`, `UPDATE`, `DELETE` or `REPLACE`, after any leading comment. Each refusal carries a code from `kernel-rs/kernel/src/refusal-codes.json`, and topic `refusals` of `platform_guide` lists them with their fix. The Gate and the Kernel run the cases in `kernel-rs/kernel/src/sql-rule-cases.json`.

| Statement | Message |
| --- | --- |
| A query that opens with `WITH` | `Action <name> is a query, so it must start with SELECT. Move the WITH clause into a subquery, as in SELECT ... FROM (WITH ... SELECT ...).` |
| A mutation that opens with `WITH` | `Action <name> is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE. Put the WITH clause after INSERT INTO <table> (<columns>), or inside a subquery.` |
| A `CREATE`, `ALTER` or `DROP` | `Action <name> is a query, so it must start with SELECT. Tables and indexes belong in schema.sql, and Konae runs no triggers or views.`, or the same for a mutation |
| Any other first word | `Action <name> is a query, so it must start with SELECT.` or `Action <name> is a mutation, so it must start with INSERT, UPDATE, DELETE or REPLACE.` |
| Text after the first `;` other than comments and whitespace | `Action <name> must contain one statement.` |
| `RETURNING` on a mutation | `Action <name> cannot use RETURNING. Mutations return changes and lastInsertId; use a query to read rows.` |
| A query that reads rows with no top-level `LIMIT` | `Action <name> reads rows and has no LIMIT. Add a LIMIT. ...`, with the reason, such as `uses OVER, which keeps one result row per stored row`. See [The Gate and the Kernel](../gate-and-kernel.md) |
| A trigger, a view or a row in `schema.sql` | `schema.sql may only create tables and indexes, but it contains CREATE TRIGGER.`, naming each refused statement |

A `WITH` clause may sit inside the statement: `INSERT INTO bookings (room_id, slot) WITH span(slot) AS (...) SELECT ...` passes. A keyword or a `;` inside a string, a quoted name or a comment does not count.

### Action shape

Before it reads a statement, each door checks the number of actions, then each action in sorted name order: its name, access, kind, bindings and size. `kernel-rs/kernel/src/action-cases.json` holds the cases.

| Declaration | Message |
| --- | --- |
| More than 100 actions | `A project may declare at most 100 actions.` |
| A name that is not lowercase dotted words of letters and digits, or is over 100 characters | `Invalid server action name: <name>` |
| `"access": "granted"` | `Action <name> uses "access": "granted", which the Kernel does not support yet. Use "anonymous" or "authenticated".` |
| Any other access | `Action <name> has an invalid access. Use "anonymous" or "authenticated".` |
| A kind other than `query`, `mutation`, `signal` or `call` | `Action <name> has an invalid kind. Use "query", "mutation", "signal" or "call".` |
| `"nullable": true` on a binding that is not an input | `Action <name> has an invalid nullable input binding.` |
| A binding of another source, an input with no key or another type, an actor field other than `id`, a kernel field other than `now`, or a literal that is not a string, a number or null | `Invalid binding <index> in <name>.` |
| A string input's `maxLength` outside 1 to 100,000 | `Invalid maxLength in <name>.` |
| A statement over 20,000 bytes, or more than 100 bindings | `Action <name> exceeds statement or binding limits.` |
| A top-level key the kind does not take, checked last | `Action <name> has a key <key> that a <kind> action does not take.` |

| Kind | Keys |
| --- | --- |
| `query` | `kind`, `access`, `statement`, `bindings`, `http` |
| `mutation` | `kind`, `access`, `statement`, `bindings`, `http`, `signal`, `guard`, `notify`, `schedule` |
| `signal` | `kind`, `access`, `bindings`, `signal`, and `statement` only when it is empty |
| `call` | `kind`, `access`, `bindings`, `url`, `method`, `body`, `headers`, `auth`, `then`, and `statement` only when it is empty |

A key set to `null` counts as absent. A key with its own refusal keeps it, so a `guard` on a query gets the guard's sentence. A field of the wrong JSON type, such as `"bindings": {}`, is refused by both doors, but the Kernel answers with its JSON reader's message, `Invalid actions.json: ...`.

## Read next

- [Add a Server Action](../guides/add-a-server-action.md)
- [Upload files](../guides/upload-files.md)
- [Server Actions](../server-actions.md)
- [The miniapp SDK](../sdk.md)
