# The Validation Gate and the Kernel share one query bound

Status: accepted.

A person who does not write SQL and a person who does both produce a Capsule. If only the editor refuses an unbounded query, an exported Kernel will publish it and fail later, when a caller asks for the rows. The app has to leave with the same rule it was built under.

## Decision

A query Server Action that reads rows must have a `LIMIT` at the top of the statement. The Validation Gate refuses it in the editor, and `validate_action` refuses it on every Kernel, including one unpacked from an export. A `LIMIT` inside a subquery, a string or a comment does not count.

Two queries are exempt because they cannot grow with the table. A query with no `FROM` returns one row. A query whose select list is only `count`, `sum`, `avg`, `min`, `max`, `total` or `group_concat`, and which has no `GROUP BY`, returns one row. A `GROUP BY`, an `OVER` window and a `UNION`, `INTERSECT` or `EXCEPT` still need a top-level `LIMIT`.

The 1000-row runtime cap stays. A `LIMIT` larger than that still fails when the action runs.

The gate does not infer which rows belong to a person from a column name. A shared board and a private journal can both store `user_id`. Personal rows still need an actor binding on the read and the write, and the agent has to say whether each new action is personal or shared. Guessing would forbid the board.

## Consequences

Capsules that listed rows without a `LIMIT` no longer publish. The examples, the Kernel's starter files and the integration fixtures name a limit. Republishing an older app with an unbounded query fails with the same sentence the editor shows.

Owner SQL Studio is not a Server Action, so this rule does not apply there. The runtime cap still does.

## Note, 2026-11-06: the bound is stated in the Action Contract

Delivery 8 of the [self-use plan](../plans/self-use-and-sdk-modules.md) writes an Action Contract for every Capsule, `action-contract.json`, so a backend other than this Kernel can be checked against the same app. Each query in it carries the bound this ADR sets: `bound` is `limit` with the top-level `LIMIT` value, or `single-row` for the two exempt forms, and `maxRows` and `maxResultBytes` repeat the 1000-row and 2 MiB cap. A replacement backend that returns more rows than the statement's `LIMIT`, or refuses a single-row total, does not serve the same app.

The contract also names the columns a query returns, read from its select list with the tokenizer this ADR's check uses. An expression without `AS` gets a key only SQLite would print, so the contract lists it as unnamed. The Validation Gate warns about it and does not refuse it, and the Kernel does not check it: it changes no row a caller may read and no bound, and refusing it would break published apps that read such a key today. Making it a refusal would need the same check in `kernel-rs/src/main.rs` and a case in both test suites, as the bound has.
