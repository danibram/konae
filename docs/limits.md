# Limits

A Capsule and a Deployment run into these numbers. The Validation Gate and the Kernel both enforce the declaration limits, and the Kernel alone enforces the runtime ones.

[The MCP guide](./mcp.md) states each rule with its refusal. The operator can change the file, email and call limits, and [Operate an installation](./operate.md#environment-variables) lists the variables.

## Declarations

| Limit | Value |
| --- | --- |
| Actions per project | 100 |
| Action name | Lowercase dotted words, at most 100 characters |
| Statement | One statement, at most 20,000 bytes |
| Bindings per action | At most 100 |
| String input `maxLength` | 1 to 100,000 |
| Guard message | 1 to 200 characters on one line |
| Schedule `every` | `5m` to `24h` |
| Schedule `daily` | A UTC time, `00:00` to `23:59` |
| Signal properties | At most 8 |
| Notice subject and body | 1 to 120 characters, and 1 to 2000 characters |
| HTTP route path | At most 200 characters |
| Webhook `fields` | 1 to 32 keys, each a JSON path of at most 16 steps and 200 characters |
| Call `body` | At most 32 keys |
| Call URL | At most 2048 characters |
| `connects` | At most 10 origins |
| Allowlist entries | At most 100 |

## Queries and databases

| Limit | Value |
| --- | --- |
| Rows | A top-level `LIMIT`, or one row, and a 1000-row cap |
| Response size | 2 MiB, counted as each column is read, a blob as its hex. The Kernel stops reading the row at the column that passes it |
| Statement time | 2 seconds, checked between SQLite steps |
| `LIKE` or `GLOB` pattern | 500 bytes |
| One value | 2 MiB |
| SQLite memory, all apps together | Half of the machine's memory and at least 256 MiB, or `KONAE_SQLITE_HEAP_BYTES`. A statement past it fails with SQLite's out-of-memory error |
| App database | 256 MiB |
| Accounts database, `identity.sqlite` | 64 MiB |
| Usage store, `usage.sqlite` | 128 MiB |
| Applying `schema.sql` | 30 seconds |

## Requests and rates

| Limit | Value |
| --- | --- |
| Route request body | 64 KiB |
| Actions and routes without a session or key | 30 per 10 seconds per client address and Deployment |
| Actions and routes with a session or key | 120 per 10 seconds per app user or key |
| Action statements running at once | 4 per Deployment, queries and mutations together. A call that gets no slot within 2 seconds answers `rate-limited` with `Retry-After`, as a call over its address limit does. Scheduled runs and the mutations a call's answer triggers are not counted |
| Unknown or revoked API keys | 10 per minute per client address and Deployment |
| Signature clock window | 300 seconds |
| Replay window for one signature | 600 seconds |
| Replay window for a provider's delivery | 72 hours |
| App or owner login and signup | 10 attempts per minute per client address, and 5 per minute for one email from one client address |
| Wrong passwords for one email | 100 per hour from all clients, at most 20 of them from one client address |
| OAuth client registrations | 10 per hour per client address |

An IPv6 client counts as its /64, and as its /48 for OAuth registration.

## Files

| Limit | Value |
| --- | --- |
| One file | 2 MiB, or `KONAE_FILE_MAX_BYTES` |
| Deployment file quota | 20 MiB, the sum of the files it holds, or `KONAE_FILES_QUOTA_BYTES` |
| Files per Deployment | 10,000 |
| File name | First 180 characters kept |

## Email

| Limit | Value |
| --- | --- |
| Account email to one address | 3 a minute across the installation, and 10 a day for the owners or for each app |
| Account email asked for by one client address | 20 an hour |
| Account email per owner's apps, and per app | 100 a day each |
| Notices per app, per owner, and per recipient from one app | 200, 500 and 50 a day |
| Every email from the installation | 1,000 a day |
| Recipients of one notice run | 50 accounts, from at most 1,000 changed rows |
| A notice not sent | Fails after one day |

## Calls

| Limit | Value |
| --- | --- |
| Calls queued per app | 20 in any 60 seconds, or `KONAE_CALLS_PER_MINUTE` |
| One attempt | 10 seconds, and an answer of at most 64 KiB |
| Attempts | 5 |
| Finished calls kept in the outbox | 1000 per app |

## Accounts and credentials

| Limit | Value |
| --- | --- |
| Sessions per account | 20 |
| Verification link | 24 hours |
| Invitation link | Seven days |
| Recovery link | 30 minutes |
| Entering as a user | One use within two minutes |
| Emails per `app_invitations_create` | 1 to 50 |
| API keys per Deployment | 50 live keys |
| `konae login` token, or one from the owner sign-in page | 24 hours |
| `token_create` | Up to 30 days, or one hour under Clerk |
| OAuth token | One hour, no refresh token |
| Owner invitation code | 14 days by default, `1h` to `90d` |

## Transfers and publishing

| Limit | Value |
| --- | --- |
| Transfer chunk | At most 64 KiB, base64 |
| Owner transfers | Expire after one hour, eight at a time per owner, 128 per installation |
| App user transfers | Expire after 10 minutes, two at a time per app user, 32 per Deployment |
| Visitor transfers | Two at a time per client address, 16 per public Deployment |
| Owner transfer size | Up to 32 MiB, or an export component's limit |
| Publishing payload | 26 MB, with its artifact at most 16 MiB after base64 is decoded |
| Export components | Database 256 MiB, accounts 64 MiB, Usage 128 MiB, files within the destination's quota |

## Usage store

The store keeps 400 days and at most 100,000 rows of totals, the newest 50,000 Signals and the newest 10,000 scheduled runs. A string property keeps its first 128 bytes.

## Read next

- [The Gate and the Kernel](./gate-and-kernel.md)
- [Server Actions](./server-actions.md)
- [Named divergences](./divergences.md)
