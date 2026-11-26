import { expect, test } from "bun:test";
import initSqlJs from "sql.js";
import cases from "../../kernel-rs/kernel/src/functions-cases.json";
import { parseFunctions } from "../kernel/functions";
import { createCapsuleValidator } from "./gate";
const sql = initSqlJs();
const gate = createCapsuleValidator(() => sql);

for (const fixture of cases.filter((fixture) => "code" in fixture)) test(`Gate functions parity: ${fixture.name}`, async () => {
  const files = { ...Object.fromEntries(fixture.files.map((path) => [path, 'throw Error("must not execute"); export function handle() {}'])), "functions.json": fixture.source, "actions.json": JSON.stringify({ version: 1, actions: fixture.actions }) };
  const result = await gate.validate(files, { state: "absent" }, { modules: fixture.modules, staticPaths: fixture.staticPaths });
  expect(result.errors[0]).toMatchObject({ code: fixture.code, message: fixture.message });
});

test("Gate reports server preview absence without executing top-level source", async () => {
  const files = { "functions.json": JSON.stringify({ version: 1, functions: { main: { entry: "functions/main.ts", mount: "/api", methods: ["GET"], access: "anonymous" } } }), "functions/main.ts": 'throw Error("must never run"); export const handle = () => new Response("ok");' };
  const result = await gate.validate(files, { state: "absent" }, { modules: { functions: { on: true } } });
  expect(result.ok).toBe(true);
  expect(result.warnings?.some((warning) => warning.message.includes("does not execute server functions"))).toBe(true);
  expect((await gate.validate(files)).errors[0].code).toBe("module.off");
});

test("Gate refuses an invalid cron and an action that would expose the same write over HTTP", async () => {
  const action = { kind: "mutation", access: "scheduled", statement: "DELETE FROM sessions", bindings: [] };
  const actions = (definition: typeof action & { http?: { method: string; path: string } }) =>
    JSON.stringify({ version: 1, actions: { "sessions.expire": definition } });
  const files = {
    "functions.json": JSON.stringify({ version: 1, functions: { cleanup: { entry: "functions/cleanup.ts", access: "scheduled", actions: ["sessions.expire"] } } }),
    "functions/cleanup.ts": 'export const handle = () => new Response(null, { status: 204 });',
    "actions.json": actions(action),
  };
  const facts = { modules: { functions: { on: true }, actions: { on: true } } };
  const route = await gate.validate({ ...files, "actions.json": actions({ ...action, http: { method: "POST", path: "/expire" } }) }, { state: "absent" }, facts);
  expect(route.errors[0]).toMatchObject({ code: "action.scheduled-shape" });
  const crons = { version: 1, crons: { cleanup: { cron: "0 24 * * *", function: "cleanup" } } };
  const timing = await gate.validate({ ...files, "crons.json": JSON.stringify(crons) }, { state: "absent" }, facts);
  expect(timing.errors[0]).toMatchObject({ code: "cron.expression-invalid" });
});

test("a first release with a cron can infer Actions for an HTTP Function when the setting is omitted", async () => {
  const files = {
    "index.html": "<main>Cron</main>",
    "functions.json": JSON.stringify({ version: 1, functions: {
      cleanup: { entry: "functions/cleanup.js", access: "scheduled" },
      items: { entry: "functions/items.js", access: "anonymous", methods: ["GET"], mount: "/api/items", actions: ["items.list"] },
    } }),
    "functions/cleanup.js": "export function handle() {}",
    "functions/items.js": "export function handle() {}",
    "crons.json": JSON.stringify({ version: 1, crons: { cleanup: { cron: "0 8 * * *", function: "cleanup" } } }),
    "schema.sql": "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY);",
    "actions.json": JSON.stringify({ version: 1, actions: { "items.list": { kind: "query", access: "anonymous", statement: "SELECT id FROM items LIMIT 10", bindings: [] } } }),
  };
  expect((await gate.validate(files, { state: "absent" }, { modules: {} })).ok).toBe(true);
  const disabled = await gate.validate(files, { state: "absent" }, { modules: { actions: { on: false } } });
  expect(disabled.errors[0]).toMatchObject({ code: "module.requires" });
});

test("Gate reserves function receipts from schema and actions", async () => {
  const result = await gate.validate({ "schema.sql": "CREATE TABLE IF NOT EXISTS __konae_function_receipts(id INTEGER);" });
  expect(result.errors[0]).toMatchObject({ code: "schema.statement-refused", message: "schema.sql cannot create the reserved name __konae_function_receipts." });
  const action = await gate.validate({ "actions.json": JSON.stringify({ version: 1, actions: { "receipts.read": { kind: "query", access: "anonymous", statement: "SELECT * FROM __konae_function_receipts LIMIT 1", bindings: [] } } }) });
  expect(action.errors[0]).toMatchObject({ code: "schema-transform.statement-refused", detail: "Action receipts.read cannot read or write a reserved Kernel table." });
});

const artifactCases = (await import("../../kernel-rs/kernel/src/functions-artifact-cases.json")).default;
for (const fixture of artifactCases.filter((item) => item.gate !== false)) test(`Gate artifact parity: ${fixture.name}`, async () => {
  const declared = fixture.source === null ? undefined : parseFunctions(fixture.source);
  const files = declared ? {
    "functions.json": fixture.source,
    ...Object.fromEntries(Object.values(declared.functions).map((fn) => [fn.entry, 'throw Error("must not execute"); export function handle() {}'])),
  } : {};
  const result = await gate.validate(files, { state: "absent" }, { modules: { functions: { on: true }, auth: { on: true }, actions: { on: true } }, serverArtifact: fixture.artifact });
  if (fixture.ok) expect(result.ok).toBe(true);
  else expect(result.errors[0]).toMatchObject({ code: fixture.code, message: fixture.message });
});

test("Gate refuses oversized server metadata before checking the allowlist", async () => {
  const fixture = artifactCases.find((item) => item.metadataBytes === 262145);
  if (!fixture) throw Error("Missing oversized metadata fixture.");
  const result = await gate.validate({ "functions.json": JSON.stringify(fixture.artifact.manifest) }, { state: "absent" }, { modules: { functions: { on: true } } });
  expect(result.errors[0]).toMatchObject({ code: "function.manifest-size", message: "Server manifest exceeds 262144 UTF-8 bytes." });
});
