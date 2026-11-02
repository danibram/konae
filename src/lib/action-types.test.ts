import { describe, expect, test } from "bun:test";
import ts from "typescript";
import { generateActionContract } from "./action-contract";
import { renderActionTypes } from "./action-types";

const schemaSource = `CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, title TEXT NOT NULL, note TEXT, owner_id TEXT NOT NULL);`;
const actionsSource = JSON.stringify({
  version: 1,
  actions: {
    "tasks.list": { kind: "query", access: "authenticated", statement: "SELECT id, title, note FROM tasks WHERE owner_id = ?1 LIMIT 50", bindings: [{ source: "actor", field: "id" }] },
    "tasks.create": { kind: "mutation", access: "authenticated", statement: "INSERT INTO tasks(title, note, owner_id) VALUES (?1, ?2, ?3)", bindings: [{ source: "input", key: "title", type: "string", maxLength: 200 }, { source: "input", key: "note", type: "string", nullable: true }, { source: "actor", field: "id" }] },
    "tasks.sweep": { kind: "mutation", access: "anonymous", statement: "DELETE FROM tasks WHERE note IS NULL", bindings: [], schedule: { every: "6h" } },
  },
});

/** The errors TypeScript reports for `page` compiled beside the generated declarations. */
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
