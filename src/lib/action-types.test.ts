import { describe, expect, test } from "bun:test";
import ts from "typescript";
import { generateActionContract } from "./action-contract";
import { renderActionTypes } from "./action-types";
import { renderFunctionContextTypes } from "./action-types";
import { parseFunctions } from "../kernel/functions";

const schemaSource = `CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, note TEXT, owner_id TEXT NOT NULL);`;
const actionsSource = JSON.stringify({
  version: 1,
  actions: {
    "tasks.list": { kind: "query", access: "authenticated", statement: "SELECT id, title, note FROM tasks WHERE owner_id = ?1 LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "tasks.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO tasks(title, note, owner_id) VALUES (?1, ?2, ?3)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 200 }, { source: "input", key: "note", type: "string", nullable: true }, { source: "actor", field: "id" }] },
    "tasks.sweep": { kind: "mutation", access: "anonymous", statement: "DELETE FROM tasks WHERE note IS NULL", bindings: [], schedule: { every: "6h" } },
  },
});

function check(page: string): string[] {
  const files: Record<string, string> = {
    "/app/actions.d.ts": renderActionTypes(generateActionContract({ actionsSource, schemaSource }).contract),
    "/app/page.ts": page,
  };
  const options: ts.CompilerOptions = { strict: true, noEmit: true, target: ts.ScriptTarget.ES2022, lib: ["lib.es2022.d.ts", "lib.dom.d.ts"], types: [] };
  const host = ts.createCompilerHost(options);
  const read = host.readFile.bind(host);
  host.readFile = (name) => files[name] ?? read(name);
  const exists = host.fileExists.bind(host);
  host.fileExists = (name) => name in files || exists(name);
  const getSource = host.getSourceFile.bind(host);
  host.getSourceFile = (name, version) => name in files ? ts.createSourceFile(name, files[name], version) : getSource(name, version);
  const program = ts.createProgram(Object.keys(files), options, host);
  return ts.getPreEmitDiagnostics(program).map((diagnostic) => ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"));
}

describe("actions.d.ts", () => {
  test("types a call by name, input and the columns a query returns", () => {
    expect(check(`
      export async function page() {
        const rows = await miniapp.actions.call("tasks.list");
        const title: string = rows[0].title;
        const note: string | null = rows[0].note;
        const id: number = rows[0].id;
        const made = await window.miniapp.actions.call("tasks.create", { title: "Ship", note: null });
        const changed: number = made.changes;
        await miniapp.actions.call("tasks.create", { title: "Ship" });
        await miniapp.auth.session();
        return [title, note, id, changed];
      }
    `)).toEqual([]);
  });

  test("refuses a name no page may call, a wrong input and a column the query does not return", () => {
    const errors = check(`
      export async function page() {
        await miniapp.actions.call("tasks.missing");
        await miniapp.actions.call("tasks.sweep");
        await miniapp.actions.call("tasks.create", { title: 5 });
        await miniapp.actions.call("tasks.create", {});
        await miniapp.actions.call("tasks.list", { extra: "x" });
        const rows = await miniapp.actions.call("tasks.list");
        return rows[0].owner_id;
      }
    `);
    expect(errors).toHaveLength(6);
    expect(errors.join("\n")).toContain('"tasks.missing"');
    expect(errors.join("\n")).toContain('"tasks.sweep"');
    expect(errors.join("\n")).toContain("owner_id");
  });
});

describe("konae:actions declarations", () => {
  const manifest = parseFunctions(JSON.stringify({ version: 1, functions: {
    orders: { entry: "functions/orders.ts", mount: "/orders", methods: ["GET", "POST"], access: "authenticated", actions: ["tasks.list", "tasks.create"] },
    reader: { entry: "functions/reader.ts", mount: "/reader", methods: ["GET"], access: "authenticated", actions: ["tasks.list"] },
    empty: { entry: "functions/empty.ts", mount: "/empty", methods: ["GET"], access: "authenticated", actions: [] },
  } }));
  const declarations = renderFunctionContextTypes(generateActionContract({ actionsSource, schemaSource }).contract, manifest);

  function checkFunction(source: string): string[] {
    const files: Record<string, string> = { "/app/functions.d.ts": declarations, "/app/functions/orders.ts": source };
    const options: ts.CompilerOptions = { strict: true, noEmit: true, target: ts.ScriptTarget.ES2022, lib: ["lib.es2022.d.ts", "lib.dom.d.ts"], types: [] };
    const host = ts.createCompilerHost(options);
    const read = host.readFile.bind(host);
    host.readFile = (name) => files[name] ?? read(name);
    const exists = host.fileExists.bind(host);
    host.fileExists = (name) => name in files || exists(name);
    const getSource = host.getSourceFile.bind(host);
    host.getSourceFile = (name, version) => name in files ? ts.createSourceFile(name, files[name], version) : getSource(name, version);
    const program = ts.createProgram(Object.keys(files), options, host);
    return ts.getPreEmitDiagnostics(program).map((diagnostic) => ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"));
  }

  test("types the same client from each function's allowlist and the Action Contract", () => {
    expect(checkFunction(`
      import { actions, type FunctionContext } from "konae:actions";
      export async function handle(_request: Request, context: FunctionContext<"orders">) {
        const client = actions(context);
        const rows = await client.call("tasks.list");
        const title: string = rows[0]!.title;
        const note: string | null = rows[0]!.note;
        const result = await client.call("tasks.create", { title: "Ship", note: null });
        const count: number = result.changes;
        const noInput = await actions(context).call("tasks.create", { title: "Ship" });
        return [title, note, count, noInput];
      }
    `)).toEqual([]);
    expect(declarations).toContain('"empty": { call(name: never): Promise<never>; }');
    expect(checkFunction(`
      import { type FunctionContext } from "konae:actions";
      export async function handle(_request: Request, context: FunctionContext<"reader">) {
        await context.actions.call("tasks.create", { title: "outside this allowlist" });
      }
    `).join("\n")).toContain('"tasks.create"');
  });

  test("rejects names outside the allowlist, bad inputs and invalid output reads", () => {
    const errors = checkFunction(`
      import { type FunctionContext } from "konae:actions";
      export async function handle(_request: Request, context: FunctionContext<"orders">) {
        await context.actions.call("tasks.sweep");
        await context.actions.call("tasks.missing");
        await context.actions.call("tasks.create", {});
        await context.actions.call("tasks.create", { title: 5 });
        const rows = await context.actions.call("tasks.list");
        return rows[0]!.owner_id;
      }
    `);
    expect(errors.length).toBeGreaterThanOrEqual(5);
    expect(errors.join("\n")).toContain('"tasks.sweep"');
    expect(errors.join("\n")).toContain('"tasks.missing"');
    expect(errors.join("\n")).toContain("owner_id");
  });
});
