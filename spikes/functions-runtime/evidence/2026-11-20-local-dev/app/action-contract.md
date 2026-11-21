# Action Contract for Functions local demo

Generated from actions.json and schema.sql. action-contract.json is the machine-readable form and action-contract.schema.json its schema. Do not edit either; change the Capsule and generate them again.

3 actions. Every action runs on the runtime endpoint `/_konae/mcp` as the `actions_invoke` tool, which `miniapp.actions.call(name, input)` calls.

- A query answers an array of row objects keyed by the column names listed. A mutation answers { changes, lastInsertId }.
- Column types are SQLite affinities read from schema.sql. SQLite does not enforce them, so a value can still arrive as another JSON type when it was stored as one.
- Nullability follows NOT NULL and INTEGER PRIMARY KEY in schema.sql. A column read through the right side of a LEFT JOIN is nullable.
- A SQLite error, such as a UNIQUE constraint, is returned with SQLite's own text.
- On a private Deployment every runtime call needs a session, including anonymous actions. An HTTP route answers the callers it takes: without a callers list, a signed route takes the signature or an API key, and an unsigned one anyone on a public Deployment and an API key or a session on a private one. An action whose route does not answer anyone answers only there.

| Action | Kind | Access | HTTP |
| --- | --- | --- | --- |
| `notes.create` | mutation | authenticated |  |
| `notes.delete` | mutation | authenticated |  |
| `notes.list` | query | authenticated |  |

## notes.create

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
INSERT INTO notes (owner_id, title, created_at) VALUES (?, ?, ?)
```

### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `title` | string | yes | 500 UTF-16 code units |

### Output

`{ changes, lastInsertId }`, both integers.

### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to the login page under the base path |
| `Sign in to use Konae MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: title` |  | runtime | a required input is omitted |
| `title must be a non-empty string.` |  | runtime | a string input is not a string, or is empty or only whitespace |
| `title is too long.` |  | runtime | a string input is longer than its maxLength in UTF-16 code units |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, 120 from one signed-in user, or four statements of the Deployment already running and no slot free within 2 seconds |

### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

## notes.delete

A mutation declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
DELETE FROM notes WHERE id = ? AND owner_id = ?
```

### Input

| Key | Type | Required | Limit |
| --- | --- | --- | --- |
| `id` | number | yes |  |

### Output

`{ changes, lastInsertId }`, both integers.

### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to the login page under the base path |
| `Sign in to use Konae MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Missing action input: id` |  | runtime | a required input is omitted |
| `id must be a number.` |  | runtime | a number input is not a JSON number |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, 120 from one signed-in user, or four statements of the Deployment already running and no slot free within 2 seconds |

### Limits

- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

## notes.list

A query declared authenticated. It binds the caller's ID, so a caller without one is refused.

```sql
SELECT id, title, created_at FROM notes WHERE owner_id = ? ORDER BY created_at DESC LIMIT 100
```

### Input

None. Send `{}`.

### Output

An array of row objects.

| Column | Type | Nullable | Reads |
| --- | --- | --- | --- |
| `id` | integer | no | notes.id |
| `title` | text | no | notes.title |
| `created_at` | integer | no | notes.created_at |

### Callers

Runtime endpoint: person (the app session, as a cookie from the app's own origin or Authorization: Bearer <session token>; actor user id).

### Errors

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `authentication-required` |  | runtime | the action is not declared anonymous and no one is signed in; the published SDK also sends the browser to the login page under the base path |
| `Sign in to use Konae MCP.` |  | runtime | the Deployment is private and the caller has no session |
| `Unexpected action input: <key>` |  | runtime | input has a key no input binding declares |
| `Query result exceeds 2 MiB or 1000 rows.` |  | runtime | a query produces more than 1000 rows or more than 2 MiB of JSON |
| `rate-limited` |  | runtime | more than 30 calls in 10 seconds from one signed-out client address on one Deployment, 120 from one signed-in user, or four statements of the Deployment already running and no slot free within 2 seconds |

### Limits

- LIMIT 100 at the top of the statement; at most 1000 rows and 2 MiB of JSON in any case.
- 2 seconds per statement.
- 30 calls per 10 seconds for a visitor, per client address and Deployment.
- 120 calls per 10 seconds for a signed-in person or API key, each.

## Any action

| Message | Status | Seen on | When |
| --- | --- | --- | --- |
| `Server action not declared.` |  | runtime | actions.json declares no action with that name |
| `Sign in to use Konae MCP.` |  | runtime | the Deployment is private and the caller has no session, whatever the action's name |
| `Arguments must be an object.` |  | runtime | input is not an object, e.g. null |
| `No route accepts POST on this path.` | 405 | http | a POST reaches a path no action binds (405) |
