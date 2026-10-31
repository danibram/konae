# Limits

A Capsule and a Deployment run into these numbers. Both the Gate and the Kernel enforce the declaration limits. The Kernel alone enforces the runtime ones.

[The MCP guide](./mcp.md) states each rule in full. When a number matters to a decision, read the section there. The operator of an installation can change the file and email limits, and [Operate an installation](./operate.md#environment-variables) lists the variables.

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
| HTTP route path | At most 200 characters |
| Allowlist entries | At most 100 |

## Queries

| Limit | Value |
| --- | --- |
| Rows | A top-level `LIMIT`, or one row, and a 1000-row cap |
| Response size | 2 MiB |
| Statement time | 2 seconds, checked between SQLite steps |
| `LIKE` or `GLOB` pattern | 500 bytes |
| App database | 256 MiB |

## Requests and rates

| Limit | Value |
| --- | --- |
| Route request body | 64 KiB |
| Route calls without a key or session | 30 per 10 seconds per client address |
| Route calls with a user or key | 120 per 10 seconds per user or key, shared with `actions_invoke` |
| Signature clock window | 300 seconds |
| Replay window for one signature | 600 seconds |
| App login and signup | 10 attempts per minute per client address, 5 per minute for one email from one client address |

## Files

| Limit | Value |
| --- | --- |
| One file | 2 MiB |
| Deployment file quota | 20 MiB, the sum of the files it holds |
| Files per Deployment | 10,000 |
| File name | First 180 characters kept |

## Invitations and credentials

| Limit | Value |
| --- | --- |
| Emails per `app_invitations_create` | 1 to 50 |
| Invitation link | Seven days |
| `konae login` token | 24 hours |
| OAuth token | One hour, no refresh token |

## Transfers and publishing

| Limit | Value |
| --- | --- |
| Transfer chunk | At most 64 KiB, base64 |
| Owner transfers | Expire after one hour, eight at a time per owner |
| App user transfers | Expire after 10 minutes, two at a time per app user |
| Administrative transfer | Up to 32 MiB, or a package component's limit |
| Publishing payload | Must fit in 26 MB, with its artifact in 16 MiB |

## Usage store

The store keeps 400 days and at most 100,000 rows of totals, the newest 50,000 Signals and the newest 10,000 scheduled runs. A string property keeps its first 128 bytes.

## Read next

- [The Gate and the Kernel](./gate-and-kernel.md)
- [Server Actions](./server-actions.md)
- [Named divergences](./divergences.md)
