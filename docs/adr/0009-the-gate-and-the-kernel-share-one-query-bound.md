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

## Note, 2026-11-06: the other statement rules share one case file

The showcase apps found four rules the two doors refused in different words: a leading `WITH`, a second statement, a trigger or view written as an action, and a trigger or view in `schema.sql`. The Gate said `Runtime query only allows: SELECT. Use schema.sql for structural changes.` where the Kernel said `Action habits.streaks contains disallowed SQL.`, and the Gate's hint sent a CTE to `schema.sql`. Both doors now take the rules from one place each, `src/kernel/sql-rules.ts` and `kernel-rs/src/sql_rules.rs`, and run the cases in `kernel-rs/src/sql-rule-cases.json`, as they already did for the query bound. The first keyword is read after leading comments on both sides, the `;` check ignores strings and comments on both sides, and the Kernel refuses `RETURNING` when it parses an action, not only when it prepares it. A refusal names the action, the rule and where the statement belongs: a `WITH` clause inside the statement, a table or index in `schema.sql`. The Kernel reads `schema.sql` before its authorizer runs and names a trigger, a view or a row the way the Gate does; anything else the authorizer refuses keeps its message.

## Note, 2026-11-07: the shape of an action shares one case file too

A review of the editor door found that the Kernel published actions the Gate refused. It took a name with a capital or a hyphen, a binding of an unknown source or type, a `maxLength` of 0 and a boolean literal, and it allowed 100 actions where the Gate and the build agent's prompt allowed 50. Where both refused, they used different words, such as `Invalid nullable option in notes.add.` and `Action notes.add has an invalid nullable input binding.` Both doors now check the name, access, kind, bindings and size in one order with one set of messages, `kernel-rs/src/action_shape.rs` and `parseServerActionSource`, and run `kernel-rs/src/action-cases.json`. The limit is 100 actions, the number docs/mcp.md already gave, because lowering the Kernel's limit could refuse a republish that works today. Both check actions in sorted name order, so with two bad actions they name the same one. An External Project that published a name or a binding the Kernel now refuses fails its next publish with the sentence the editor shows.
