import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import initSqlJs from "sql.js";
import { parseServerActionSource } from "../kernel/actions";
import {
  checkActionContract,
  deriveOutputColumns,
  generateActionContract,
  parseSchemaTables,
  programFromContract,
  renderActionContractMarkdown,
  serializeActionContract,
  type ActionContract,
} from "./action-contract";

const repo = path.resolve(import.meta.dir, "../..");
const examplesRoot = path.join(repo, "examples/product-fit");
const examples = readdirSync(examplesRoot, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => ({
    name: entry.name,
    actions: readFileSync(path.join(examplesRoot, entry.name, "actions.json"), "utf8"),
    schema: readFileSync(path.join(examplesRoot, entry.name, "schema.sql"), "utf8"),
  }));

/** The parser keeps an explicit `"nullable": false`; the contract says the same thing by omitting it. */
function withoutFalseNullable(program: ReturnType<typeof parseServerActionSource>) {
  return JSON.parse(JSON.stringify(program, (key, value) => key === "nullable" && value === false ? undefined : value));
}

describe("action contract for the product-fit examples", () => {
  test("covers all eight examples", () => {
    expect(examples.map((example) => example.name).sort()).toEqual(["board", "docs", "feed", "journal", "launch", "poll", "portfolio", "probe"]);
  });

  for (const example of examples) {
    test(`${example.name}: validates against the schema and round-trips`, () => {
      const { contract, warnings } = generateActionContract({ actionsSource: example.actions, schemaSource: example.schema });
      expect(warnings).toEqual([]);
      const written = serializeActionContract(contract);
      const read = JSON.parse(written);
      expect(checkActionContract(read)).toEqual([]);
      const { $schema, ...body } = read;
      expect($schema).toBe("./action-contract.schema.json");
      expect(body).toEqual(contract);
      // The contract describes the same program the Kernel would load.
      expect(programFromContract(body as ActionContract)).toEqual(withoutFalseNullable(parseServerActionSource(example.actions)));
      // Generating twice gives the same bytes, so a checked-in contract only changes with its sources.
      expect(serializeActionContract(generateActionContract({ actionsSource: example.actions, schemaSource: example.schema }).contract)).toBe(written);
      expect(renderActionContractMarkdown(contract)).toContain(`## ${contract.actions[0].name}`);
    });
  }

  test("every query names exactly the columns SQLite returns", async () => {
    const SQL = await initSqlJs();
    let queries = 0;
    for (const example of examples) {
      const db = new SQL.Database();
      db.run(example.schema);
      const { contract } = generateActionContract({ actionsSource: example.actions, schemaSource: example.schema });
      for (const action of contract.actions) {
        if (action.output.kind !== "rows") continue;
        const statement = db.prepare(action.statement);
        expect({ action: action.name, columns: action.output.columns.map((column) => column.name) })
          .toEqual({ action: action.name, columns: statement.getColumnNames() });
        statement.free();
        queries += 1;
      }
      db.close();
    }
    expect(queries).toBe(17);
  });

  test("the feed's routes carry method, path, signature scheme and the callers each takes", () => {
    const feed = examples.find((example) => example.name === "feed")!;
    const { contract } = generateActionContract({ actionsSource: feed.actions, schemaSource: feed.schema });
    const pings = contract.actions.find((action) => action.name === "pings.receive")!;
    expect(pings.http).toMatchObject({
      method: "POST",
      path: "/hooks/pings",
      signature: { header: "X-Signature", secret: "pings_webhook", windowSeconds: 300, acceptedOnce: true },
    });
    // A signed route takes the signature or a key, and is the action's only door.
    expect(pings.http!.callers.map((caller) => caller.kind)).toEqual(["visitor", "service"]);
    expect(pings.http!.onlyDoor).toBe("always");
    expect(pings.runtime.callers).toEqual([]);
    expect(pings.errors.find((error) => error.message === "Action pings.receive answers only on its HTTP route, POST /hooks/pings.")).toMatchObject({ via: "runtime" });
    expect(pings.errors.find((error) => error.message === "stale-timestamp")).toMatchObject({ status: 401, via: "http" });
    expect(pings.output).toEqual({ kind: "changes", fields: ["changes", "lastInsertId"] });
    expect(pings.errors.find((error) => error.message === "invalid-signature")).toMatchObject({ status: 401, via: "http" });
    expect(pings.errors.find((error) => error.message === "Missing action input: source")).toMatchObject({ status: 400, via: "http" });
    expect(pings.limits.maxRequestBytes).toBe(64 * 1024);
    const recent = contract.actions.find((action) => action.name === "incidents.recent")!;
    expect(recent.http).toMatchObject({ method: "GET", path: "/api/incidents" });
    expect(recent.http!.signature).toBeUndefined();
    expect(recent.errors.some((error) => error.message === "invalid-signature")).toBe(false);
    // Unsigned, it answers anyone on a public Deployment and takes a key or a session on a private one.
    expect(recent.http!.onlyDoor).toBe("on a private Deployment");
    expect(recent.http!.callers.map((caller) => caller.kind)).toEqual(["visitor", "service", "person"]);
    expect(recent.errors.find((error) => error.message === "authentication-required" && error.via === "http")).toMatchObject({ status: 401 });
    expect(recent.limits).toMatchObject({ bound: "limit", limit: 50, maxRows: 1000 });
    // A GET route refuses HEAD before any SQL; a POST route has no HEAD to refuse.
    expect(recent.errors.find((error) => error.message === "A route does not answer HEAD. Send GET.")).toMatchObject({ status: 405, via: "http" });
    expect(pings.errors.some((error) => error.message === "A route does not answer HEAD. Send GET.")).toBe(false);
  });

  test("the Markdown names each Signal with its properties, and a GET route's 405 for HEAD", () => {
    const { contract } = generateActionContract({
      schemaSource: "CREATE TABLE IF NOT EXISTS photos (id INTEGER PRIMARY KEY, title TEXT NOT NULL);",
      actionsSource: JSON.stringify({ version: 1, actions: {
        "photos.add": {
          kind: "mutation", access: "authenticated",
          statement: "INSERT INTO photos (title) VALUES (?1)",
          bindings: [{ source: "input", key: "title", type: "string", maxLength: 80 }, { source: "actor", field: "id" }],
          signal: { name: "photo.added", properties: ["title", "actor"] },
        },
        "photos.viewed": {
          kind: "signal", access: "anonymous",
          bindings: [{ source: "input", key: "photo", type: "number" }],
          signal: { name: "photo.viewed", properties: ["photo"] },
        },
        "photos.feed": {
          kind: "query", access: "anonymous", http: { method: "GET", path: "/feed.json" },
          statement: "SELECT id, title FROM photos ORDER BY id DESC LIMIT 20",
          bindings: [],
        },
      } }),
    });
    const markdown = renderActionContractMarkdown(contract);
    const section = (name: string) => markdown.slice(markdown.indexOf(`## ${name}\n`), markdown.indexOf("### Callers", markdown.indexOf(`## ${name}\n`)));
    expect(section("photos.add")).toContain("### Signal\n\nRecords `photo.added` with `title`, `actor` (the app user's ID, the API key's ID, or null for a visitor or a signed request) after the statement changes a row. A refused call records none.");
    expect(section("photos.viewed")).toContain('`{ "recorded": true }`. It runs no SQL and records the Signal `photo.viewed`.');
    expect(section("photos.viewed")).toContain("Records `photo.viewed` with `photo` on every call the bindings accept.");
    expect(section("photos.feed")).not.toContain("### Signal");
    expect(markdown).toMatch(/\| `A route does not answer HEAD\. Send GET\.` \| 405 \| http \| a HEAD reaches the route; .*Allow: GET\./);
    expect(checkActionContract(JSON.parse(serializeActionContract(contract)))).toEqual([]);
  });

  test("inputs, access and actor refusals follow the bindings", () => {
    const probe = examples.find((example) => example.name === "probe")!;
    const { contract } = generateActionContract({ actionsSource: probe.actions, schemaSource: probe.schema });
    const optional = contract.actions.find((action) => action.name === "items.optional")!;
    expect(optional.inputs).toEqual([{ key: "q", type: "string", required: false, nullable: true, maxLength: 10_000 }]);
    expect(optional.errors.some((error) => error.message === "Missing action input: q")).toBe(false);
    const mine = contract.actions.find((action) => action.name === "items.mine")!;
    expect(mine).toMatchObject({ access: "authenticated", actorBound: true });
    expect(mine.runtime.callers.map((caller) => caller.kind)).toEqual(["person"]);
    expect(mine.errors.map((error) => error.message)).toContain("authentication-required");
    const add = contract.actions.find((action) => action.name === "items.add")!;
    expect(add.errors.map((error) => error.message)).toEqual([
      "Sign in to use Littlebox MCP.",
      "Unexpected action input: <key>",
      "Missing action input: title",
      "title must be a non-empty string.",
      "title is too long.",
      "rate-limited",
    ]);
  });
});

describe("output column derivation", () => {
  const tables = parseSchemaTables(`
    CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, note TEXT, owner_id TEXT NOT NULL, score REAL, raw);
    CREATE TABLE IF NOT EXISTS "people" ("user_id" TEXT PRIMARY KEY NOT NULL, name VARCHAR(80) NOT NULL, UNIQUE (name));
    CREATE INDEX IF NOT EXISTS tasks_owner ON tasks(owner_id);
  `);

  test("reads table columns, affinities and nullability from schema.sql", () => {
    expect(tables.get("tasks")).toEqual([
      { name: "id", type: "integer", nullable: false },
      { name: "title", type: "text", nullable: false },
      { name: "done", type: "integer", nullable: false },
      { name: "note", type: "text", nullable: true },
      { name: "owner_id", type: "text", nullable: false },
      { name: "score", type: "real", nullable: true },
      { name: "raw", type: "any", nullable: true },
    ]);
    expect(tables.get("people")).toEqual([
      { name: "user_id", type: "text", nullable: false },
      { name: "name", type: "text", nullable: false },
    ]);
  });

  const cases: [string, string[], { expression: string }[]][] = [
    ["SELECT * FROM tasks LIMIT 5", ["id", "title", "done", "note", "owner_id", "score", "raw"], []],
    ["SELECT t.*, p.name FROM tasks t JOIN people p ON p.user_id = t.owner_id LIMIT 5", ["id", "title", "done", "note", "owner_id", "score", "raw", "name"], []],
    ["SELECT title AS heading, note memo, \"done\" FROM tasks LIMIT 5", ["heading", "memo", "done"], []],
    ["SELECT count(*) FROM tasks", [], [{ expression: "count(*)" }]],
    ["SELECT upper(title), id FROM tasks LIMIT 5", ["id"], [{ expression: "upper(title)" }]],
    ["SELECT id + 1, 'x' FROM tasks LIMIT 5", [], [{ expression: "id + 1" }, { expression: "'x'" }]],
    ["SELECT id FROM tasks WHERE id IN (SELECT id FROM tasks LIMIT 1) UNION SELECT 1 LIMIT 5", ["id"], []],
    ["SELECT DISTINCT owner_id FROM tasks ORDER BY owner_id LIMIT 5", ["owner_id"], []],
    ["SELECT x.n FROM (SELECT count(*) AS n FROM tasks) x", ["n"], []],
    ["SELECT CASE WHEN done THEN 'yes' ELSE 'no' END AS state FROM tasks LIMIT 5", ["state"], []],
  ];

  for (const [sql, names, unnamed] of cases) {
    test(sql, async () => {
      const derived = deriveOutputColumns(sql, tables);
      expect(derived.columns.map((column) => column.name)).toEqual(names);
      expect(derived.unnamed).toEqual(unnamed);
      // Every named column is the key SQLite itself returns.
      const SQL = await initSqlJs();
      const db = new SQL.Database();
      db.run(`CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, done INTEGER NOT NULL DEFAULT 0, note TEXT, owner_id TEXT NOT NULL, score REAL, raw);
        CREATE TABLE "people" ("user_id" TEXT PRIMARY KEY NOT NULL, name VARCHAR(80) NOT NULL, UNIQUE (name));`);
      const statement = db.prepare(sql);
      const sqliteNames = statement.getColumnNames();
      statement.free();
      db.close();
      for (const name of names) expect(sqliteNames).toContain(name);
      expect(sqliteNames.length).toBe(names.length + unnamed.length);
    });
  }

  test("types columns it reads and aliased totals; a LEFT JOIN makes the right side nullable", () => {
    const derived = deriveOutputColumns(
      "SELECT t.id, p.name AS owner, count(*) AS n, avg(t.score) AS mean, t.note FROM tasks t LEFT JOIN people p ON p.user_id = t.owner_id GROUP BY t.id LIMIT 5",
      tables,
    );
    expect(derived.columns).toEqual([
      { name: "id", type: "integer", nullable: false, from: { table: "tasks", column: "id" } },
      { name: "owner", type: "text", nullable: true, from: { table: "people", column: "name" } },
      { name: "n", type: "integer", nullable: false },
      { name: "mean", type: "real", nullable: true },
      { name: "note", type: "text", nullable: true, from: { table: "tasks", column: "note" } },
    ]);
  });

  test("a column of an unknown table keeps its name and an unknown type", () => {
    expect(deriveOutputColumns("SELECT value FROM json_each(?1) LIMIT 5", tables).columns)
      .toEqual([{ name: "value", type: "unknown", nullable: null }]);
  });

  test("an unnamed column becomes a warning and stays in the contract", () => {
    const { contract, warnings } = generateActionContract({
      actionsSource: JSON.stringify({ version: 1, actions: { "tasks.count": { kind: "query", access: "anonymous", statement: "SELECT count(*) FROM tasks", bindings: [] } } }),
      schemaSource: "CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY);",
    });
    expect(warnings).toEqual([{ action: "tasks.count", message: "Server action tasks.count returns count(*) without a name. Add AS <name> so every backend returns the same key." }]);
    expect(contract.actions[0].output).toEqual({ kind: "rows", columns: [], unnamed: [{ expression: "count(*)" }] });
    expect(checkActionContract(JSON.parse(serializeActionContract(contract)))).toEqual([]);
    expect(renderActionContractMarkdown(contract)).toContain("The column `count(*)` has no name.");
  });
});

describe("schema check", () => {
  const launch = examples.find((example) => example.name === "launch")!;
  const valid = () => JSON.parse(serializeActionContract(generateActionContract({ actionsSource: launch.actions, schemaSource: launch.schema }).contract));

  test("refuses a contract that leaves out a field, adds one, or changes a kind", () => {
    const missing = valid();
    delete missing.actions[0].output;
    expect(checkActionContract(missing)).toContain("$.actions[0]: missing output");
    const extra = valid();
    extra.actions[0].secret = "x";
    expect(checkActionContract(extra)).toContain("$.actions[0]: unexpected secret");
    const kind = valid();
    kind.actions[0].kind = "procedure";
    expect(checkActionContract(kind)).toEqual([`$.actions[0].kind: must be one of "query", "mutation", "signal", "call"`]);
    const output = valid();
    output.actions[0].output = { kind: "rows", columns: [] };
    expect(checkActionContract(output)[0]).toMatch(/matches 0 of the allowed shapes/);
  });

  test("a signal action gets a signal output and round-trips", () => {
    const actionsSource = JSON.stringify({ version: 1, actions: {
      "newsletter.join": {
        kind: "signal",
        access: "anonymous",
        bindings: [{ source: "input", key: "source", type: "string", maxLength: 40 }],
        signal: { name: "newsletter.joined", properties: ["source"] },
      },
    } });
    const { contract, warnings } = generateActionContract({ actionsSource, schemaSource: "" });
    expect(warnings).toEqual([]);
    expect(contract.actions[0].kind).toBe("signal");
    expect(contract.actions[0].output).toEqual({ kind: "signal", signal: "newsletter.joined" });
    const read = JSON.parse(serializeActionContract(contract));
    expect(checkActionContract(read)).toEqual([]);
    const { $schema, ...body } = read;
    expect(programFromContract(body as ActionContract)).toEqual(withoutFalseNullable(parseServerActionSource(actionsSource)));
  });

  test("a scheduled mutation carries its schedule, says so in Markdown and round-trips", () => {
    const actionsSource = JSON.stringify({ version: 1, actions: {
      "holds.release": {
        kind: "mutation",
        access: "authenticated",
        schedule: { every: "15m" },
        statement: "UPDATE holds SET released_at = ?1 WHERE released_at IS NULL AND until < ?1 AND ?2 IS NULL",
        bindings: [{ source: "kernel", field: "now" }, { source: "actor", field: "id" }],
      },
      "days.close": {
        kind: "mutation",
        access: "authenticated",
        schedule: { daily: "00:05" },
        statement: "DELETE FROM holds WHERE released_at IS NOT NULL",
        bindings: [],
      },
    } });
    const schemaSource = "CREATE TABLE IF NOT EXISTS holds (id INTEGER PRIMARY KEY, until INTEGER, released_at INTEGER);";
    const { contract } = generateActionContract({ actionsSource, schemaSource });
    expect(contract.actions.map((action) => [action.name, action.schedule])).toEqual([["days.close", { daily: "00:05" }], ["holds.release", { every: "15m" }]]);
    const markdown = renderActionContractMarkdown(contract);
    expect(markdown).toContain("The Kernel's timer also runs it every 15m, with no caller");
    expect(markdown).toContain("The Kernel's timer also runs it daily at 00:05 UTC, with no caller");
    const read = JSON.parse(serializeActionContract(contract));
    expect(checkActionContract(read)).toEqual([]);
    const { $schema, ...body } = read;
    expect(programFromContract(body as ActionContract)).toEqual(withoutFalseNullable(parseServerActionSource(actionsSource)));
    const bad = JSON.parse(serializeActionContract(contract));
    bad.actions[1].schedule = { weekly: "mon" };
    expect(checkActionContract(bad).length).toBeGreaterThan(0);
  });

  test("accepts the reserved signal output", () => {
    const signal = valid();
    signal.actions[0].output = { kind: "signal", signal: "waitlist.joined" };
    expect(checkActionContract(signal)).toEqual([]);
  });

  test("the limits it states are the Kernel's", () => {
    const limits = readFileSync(path.join(repo, "kernel-rs/src/limits.rs"), "utf8");
    const main = readFileSync(path.join(repo, "kernel-rs/src/main.rs"), "utf8");
    const http = readFileSync(path.join(repo, "kernel-rs/src/http.rs"), "utf8");
    expect(limits).toContain("ANONYMOUS_ACTIONS: u32 = 30;");
    expect(limits).toContain("ANONYMOUS_ACTIONS_PERIOD: Duration = Duration::from_secs(10);");
    expect(limits).toContain("SIGNED_IN_ACTIONS: u32 = 120;");
    expect(limits).toContain("SIGNED_IN_ACTIONS_PERIOD: Duration = Duration::from_secs(10);");
    expect(main).toContain("bytes <= 2 * 1024 * 1024 && values.len() < 1000");
    expect(main).toContain("std::time::Duration::from_secs(2)");
    expect(http).toContain("MAX_BODY_BYTES: usize = 64 * 1024;");
    expect(http).toContain("SIGNATURE_WINDOW_SECONDS: u64 = 300;");
    const action = valid().actions[0];
    expect(action.limits).toMatchObject({ maxRows: 1000, maxResultBytes: 2 * 1024 * 1024, statementSeconds: 2 });
    expect(action.limits.rate).toEqual([
      { caller: "visitor, per client address and Deployment", calls: 30, seconds: 10 },
      { caller: "signed-in person or API key, each", calls: 120, seconds: 10 },
    ]);
  });
});

describe("notify", () => {
  test("the contract lists a mutation's notice, keeps its answer and round-trips it", () => {
    const notify = { to: "actor" as const, subject: "Booked {input.room}", body: "You booked {input.room}." };
    const actionsSource = JSON.stringify({ version: 1, actions: {
      "bookings.create": { kind: "mutation", access: "authenticated", notify,
        statement: "INSERT INTO bookings (user_id, room) VALUES (?1, ?2)",
        bindings: [{ source: "actor", field: "id" }, { source: "input", key: "room", type: "string", maxLength: 40 }] },
    } });
    const schemaSource = "CREATE TABLE IF NOT EXISTS bookings (id INTEGER PRIMARY KEY, user_id TEXT NOT NULL, room TEXT NOT NULL);";
    const { contract } = generateActionContract({ actionsSource, schemaSource });
    const [action] = contract.actions;
    expect(action.notify).toEqual(notify);
    expect(action.output).toEqual({ kind: "changes", fields: ["changes", "lastInsertId"] });
    expect(checkActionContract(JSON.parse(serializeActionContract(contract)))).toEqual([]);
    expect(renderActionContractMarkdown(contract)).toContain("queues an email to the caller's verified email with the subject `Booked {input.room}`");
    expect(programFromContract(contract).actions["bookings.create"].notify).toEqual(notify);
  });
});
