# The Gate and the Kernel refuse with the same code and words

Status: accepted.

A person who does not write SQL and a person who does both produce an app. If only the editor refuses a rule, an exported Kernel publishes the app and it fails later, when a caller asks for rows. The app has to leave with the rules it was built under (job 3), and an agent has to tell which refusals it can fix by editing files and which need the owner.

## Decision

### Two doors, one set of rules

The Validation Gate in the editor (`src/agent/gate.ts`, TypeScript) and the Kernel (Rust) check `actions.json` and `schema.sql` with the same rules, in the same order, with the same sentences. Each rule has a case file under `kernel-rs/kernel/src/`, such as `action-cases.json`, `sql-rule-cases.json`, `call-cases.json` or `notify-cases.json`, and both test suites run every case. A Kernel unpacked from an export runs the same checks when it starts an app folder.

### A query names its bound

- A query Server Action that reads rows has a `LIMIT` at the top level of its statement. A `LIMIT` inside a subquery, a string or a comment does not count, and neither does a parameter named `:limit`.
- A query with no `FROM` is exempt because it returns one row. So is a query whose select list is only `count`, `sum`, `avg`, `min`, `max`, `total` or `group_concat` with no `GROUP BY`. A `GROUP BY`, an `OVER` window and a compound select still need a top-level `LIMIT`.
- The runtime caps an answer at 1,000 rows and 2 MiB, so a larger `LIMIT` still fails when the action runs.
- `src/lib/query-bound.ts` and the Kernel find the bound with the same tokenizer. The Action Contract records it for each query, so a backend other than this Kernel can be checked against the same app with `konae conform`.
- The Gate does not infer from a column name which rows belong to a person. A shared board and a private journal can both store `user_id`. Personal rows need an actor binding on the read and the write.

### The statement rules

An action is one statement. It may not add a second statement or `RETURNING`, or create a trigger or a view. A `WITH` clause belongs inside the statement. `schema.sql` holds tables and indexes, and refuses triggers and views. Konae runs no triggers or views, and an imported database that holds one is refused. A statement runs under a 2-second deadline that SQLite checks between steps. A `LIKE` or `GLOB` pattern is capped at 500 bytes because one comparison happens inside a single step.

### A refusal is a code, a sentence and a hint

Every refusal of an app's files, from either door, is `{ code, message, hint }`.

- `code` is a lowercase dotted name such as `query.limit-missing` or `route.signature-secret-unset`. It never changes once released. A rule that changes meaning gets a new code.
- `message` is the sentence both doors share.
- `hint.fix` says who fixes it: `capsule` when an edit to the app's files does, `owner` when the owner must act outside them, such as setting a secret, and `operator` when only the installation can. It may name a tool.
- The codes live in one table, `kernel-rs/kernel/src/refusal-codes.json`. The Gate imports it and the Kernel embeds it. Every refusal case carries the code it expects, and a case without a code fails both runners.
- An MCP tool that refuses keeps the sentence in its text and adds `{ code, message, hint }` as structured content. `platform_guide` topic `refusals` renders the table.

The parse errors that come before any rule, such as invalid JSON, and the runtime refusals of a call, such as a missing input, have no code.

### One implementation, in part

The Gate is TypeScript and the Kernel is Rust, held together by the case files. A spike compiled the Kernel's checks of `actions.json` to WebAssembly. They are pure, weigh 178 KB gzipped, and give the Gate's code and sentence on every shared case. The checks that compare actions with the schema need SQLite with an authorizer, which the editor's SQLite lacks, so they stay two implementations. The spike is kept at the tag `spike/checks-wasm`. Adopting the pure half is later work.

## Rejected options

- **Let only the editor refuse.** An exported Kernel would publish what the editor refused, and the app would fail when a caller asked.
- **Infer personal rows from column names.** Guessing would forbid a shared board.
- **Keep sentences and let tools match text.** Every rewording would break the build agent, the editor's footer and `conform`, and a sentence cannot say who fixes it.
- **Numeric codes.** A number tells a person reading a log nothing.
- **Codes only on the Kernel.** The editor would branch on text while the Kernel branched on codes, and the two doors would drift.

## Consequences

- Adding a rule means adding a code to the table and a case to a case file first.
- The owner's SQL in the Data panel is not a Server Action, so the `LIMIT` rule does not apply there. The runtime cap does.
- `instr` and `replace` stay unbounded inside one call, short of a lower value cap.
- An expression in a select list without `AS` gets a key only SQLite would print. The Gate warns and the Kernel does not refuse it.
