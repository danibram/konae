# Server Actions

A Server Action is a named operation declared in `actions.json`. It is the deployed app's only database interface. The browser sends the action name and its input. The SQL and the typed bindings stay on the Kernel.

## Shape

This is the `actions` topic of `platform_guide`, as a file.

```json
{
  "version": 1,
  "actions": {
    "items.list": {
      "kind": "query",
      "access": "authenticated",
      "statement": "SELECT id,title FROM items WHERE user_id=?1 LIMIT 50",
      "bindings": [{ "source": "actor", "field": "id" }]
    },
    "items.create": {
      "kind": "mutation",
      "access": "authenticated",
      "statement": "INSERT INTO items(title,user_id) VALUES(?1,?2)",
      "bindings": [
        { "source": "input", "key": "title", "type": "string", "maxLength": 200 },
        { "source": "actor", "field": "id" }
      ]
    }
  }
}
```

Bindings fill `?1`, `?2` and so on, in order. A binding takes its value from one of three sources.

| Source | Fields | Value |
| --- | --- | --- |
| `input` | `key`, `type`, `maxLength`, `nullable` | What the caller sent. `nullable: true` binds an omitted value or an explicit null as SQL NULL. |
| `actor` | `field` of `id` | The app user's ID or the API key's ID, and null for a visitor or a schedule's run. Roles live in the app's own tables. See [Accounts and access](./accounts-and-access.md). |
| `kernel` | `field` of `now` | The time of the call. |

A fourth form, `{ "source": "literal", "value": ... }`, binds a constant string, number or null.

## Kinds

| Kind | Runs | Keys it takes |
| --- | --- | --- |
| `query` | One `SELECT` | `kind`, `access`, `statement`, `bindings`, `http` |
| `mutation` | One `INSERT`, `UPDATE`, `DELETE` or `REPLACE` | The query keys, plus `signal`, `guard`, `notify` and `schedule` |
| `signal` | No SQL. It records a Signal. | `kind`, `access`, `bindings`, `signal` |
| `call` | No SQL. The Kernel sends an HTTP request. | `kind`, `access`, `bindings`, `url`, `method`, `body`, `headers`, `auth`, `then` |

A key the kind does not take is refused. A misspelled `notfy` does not pass and do nothing.

## Statement rules

- An action runs one statement. A `WITH` clause sits inside the statement, never in front of it.
- A mutation has no `RETURNING`. It answers `{ changes, lastInsertId }`. Read rows with a query.
- A query that reads rows names a top-level `LIMIT`. See [The Gate and the Kernel](./gate-and-kernel.md).
- `CREATE`, `ALTER` and `DROP` are refused. Tables and indexes belong in `schema.sql`, and Konae runs no triggers or views.

## Access

`access` is `anonymous` or `authenticated`. An `anonymous` action is open to a signed-out visitor on a Public Deployment. `granted` is refused. On a Private Deployment nothing loads or runs without a signed-in Runtime Actor, apart from an HTTP route that answers its callers.

## guard

A mutation writes its rules into its `WHERE`. When the rule says no, the statement changes no row. A `guard` turns that into a refusal with the app's own sentence.

```json
"waitlist.join": {
  "kind": "mutation",
  "access": "anonymous",
  "guard": "That address is already on the list.",
  "statement": "INSERT INTO waitlist(email,name,source,created_at) VALUES(lower(trim(?1)),?2,?3,?4) ON CONFLICT(email) DO NOTHING",
  "bindings": [
    { "source": "input", "key": "email", "type": "string", "maxLength": 254 },
    { "source": "input", "key": "name", "type": "string", "maxLength": 80 },
    { "source": "input", "key": "source", "type": "string", "maxLength": 80 },
    { "source": "kernel", "field": "now" }
  ]
}
```

The guard is a string of 1 to 200 characters on one line. A refusal is a tool error on the runtime endpoint and a 403 on a route. It counts under `refused` in Usage and records no Signal.

## notify

A mutation may queue one email when its statement changes a row.

```json
"waitlist.join": {
  "kind": "mutation",
  "access": "anonymous",
  "statement": "INSERT INTO waitlist (email, source, joined_at) VALUES (?1, ?2, ?3) ON CONFLICT (email) DO NOTHING",
  "bindings": [
    { "source": "input", "key": "email", "type": "string", "maxLength": 254 },
    { "source": "input", "key": "source", "type": "string", "maxLength": 40 },
    { "source": "kernel", "field": "now" }
  ],
  "notify": {
    "to": "owner",
    "subject": "New on the waitlist from {input.source}",
    "body": "{input.email} joined the waitlist.\nThey came from {input.source}."
  }
}
```

`to` is `owner`, `actor` or `{ "column": "<name>" }`. The Kernel never sends to an address from the input. Templates take `{input.<key>}`, `{actor.id}`, `{kernel.now}`, `{changes.count}` and, in the body, `{changes}`, which lists the columns a `changes` list names. The mutation's answer never depends on the notice.

## schedule

A mutation that takes no input may declare that the Kernel's timer runs it.

```json
"notes.surface": {
  "kind": "mutation",
  "access": "authenticated",
  "schedule": { "every": "5m" },
  "statement": "UPDATE notes SET surfaced_at = ?1 WHERE surfaced_at IS NULL AND remind_at <= ?1",
  "bindings": [{ "source": "kernel", "field": "now" }],
  "signal": { "name": "notes.surfaced" }
}
```

`every` takes `5m` to `24h`. `daily` takes a UTC time such as `02:00`. A run has no caller, so an `actor` binding is null and the guard does not apply. Publishing does not run it, and the editor preview has no timer. Nothing is pushed to a page, so a page polls a query.

## signal

A mutation may record a named occurrence for the owner when it changes a row. An action of kind `signal` records one and runs no SQL.

```json
"downloads.opened": {
  "kind": "signal",
  "access": "anonymous",
  "bindings": [{ "source": "input", "key": "fileId", "type": "string", "maxLength": 64 }],
  "signal": { "name": "download.opened", "properties": ["fileId", "actor"] }
}
```

Properties are input keys or `actor`. Only the owner reads Signals, with `signals_read`.

## http

An action declared `anonymous` may answer one method and path on its Deployment host. A query binds `GET` and a mutation binds `POST`.

```json
"status.report": {
  "kind": "mutation",
  "access": "anonymous",
  "http": {
    "method": "POST",
    "path": "/hooks/status",
    "signature": { "header": "X-Starter-Signature", "secret": "status_webhook" }
  },
  "statement": "INSERT INTO status (component, state, detail, updated_at) VALUES (?1, ?2, COALESCE(?3, ''), ?4) ON CONFLICT (component) DO UPDATE SET state = excluded.state, detail = excluded.detail, updated_at = excluded.updated_at WHERE status.state IS NOT excluded.state OR status.detail IS NOT excluded.detail",
  "bindings": [
    { "source": "input", "key": "component", "type": "string", "maxLength": 40 },
    { "source": "input", "key": "state", "type": "string", "maxLength": 12 },
    { "source": "input", "key": "detail", "type": "string", "maxLength": 200, "nullable": true },
    { "source": "kernel", "field": "now" }
  ]
}
```

`callers` lists which proofs the route takes, from `signature`, `key` and `session`. Without it, a signed route takes a signature or a key. An unsigned route on a Private Deployment takes a key or a session. An unsigned route on a Public Deployment answers anyone. A provider's scheme (`hmac-body`, `timestamped`, `svix`) and `fields`, a JSON path per input, take a webhook as GitHub, Shopify, Stripe or Resend send it. An action whose route does not answer anyone answers only there, never through `miniapp.actions.call`.

## kind: call

A call action queues one HTTP request that the Kernel sends from its own process, never from the browser. It answers `{ callId, queued: true }`.

```json
"status.escalate": {
  "kind": "call",
  "access": "authenticated",
  "method": "POST",
  "url": "http://inbox.localhost/hooks/events",
  "bindings": [
    { "source": "input", "key": "eventId", "type": "string", "maxLength": 100 },
    { "source": "input", "key": "severity", "type": "string", "maxLength": 10 },
    { "source": "input", "key": "title", "type": "string", "maxLength": 200 }
  ],
  "body": { "source": { "value": "starter" }, "id": "eventId", "severity": "severity", "title": "title" },
  "auth": { "secret": "inbox_events", "header": "X-Inbox-Signature" },
  "then": {
    "action": "status.escalated",
    "inputs": { "status": "/status", "error": "/body/error" }
  }
}
```

This is a shortened form of `status.escalate` in `examples/showcase/starter/actions.json`, which also passes the component and its time through to the `then` mutation. Each `body` entry is the name of an input or `{ "value": constant }`. `auth` is `{ secret, header? }` or `{ key }`. `then` names a mutation that runs with the answer. A URL is https, or http to an app of this installation, and never a private address. The owner reads the outbox with `calls_read`.

## Limits in short

A project declares at most 100 actions. A statement is at most 20,000 bytes with at most 100 bindings. The [limits page](./limits.md) has the rest.

## Read next

- [The Gate and the Kernel](./gate-and-kernel.md)
- [Add a Server Action](./guides/add-a-server-action.md)
- [Refuse a write with guard](./guides/refuse-a-write-with-guard.md)
- [Receive a webhook](./guides/receive-a-webhook.md)
- [Pick the smallest piece](./pieces.md)
