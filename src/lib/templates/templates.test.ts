import { describe, expect, test } from "bun:test";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import type { CapsuleFiles } from "../../agent/contracts";
import { createCapsuleValidator } from "../../agent/gate";
import { notifyReturning, notifyReturningStatement, parseServerActionSource } from "../../kernel/actions";
import { cloneStarter } from "../starter";
import { adminCheck } from "./members";
import { applyTemplate, templateChoices, type TemplateId } from "./index";

let sql: Promise<SqlJsStatic> | undefined;
const loadSql = () => (sql ??= initSqlJs());
const validator = createCapsuleValidator(loadSql);

async function exportDatabase(schema: string): Promise<Uint8Array> {
  const SQL = await loadSql();
  const db = new SQL.Database();
  db.run(schema);
  const bytes = db.export();
  db.close();
  return bytes;
}

const born = (id: TemplateId) => applyTemplate({ ...cloneStarter(), id: "app_template_test", name: "Test", slug: "test" }, id);

describe("a project born from a template", () => {
  test("the dialog offers seven templates with one sentence each", () => {
    expect(templateChoices.map((choice) => choice.id)).toEqual(["tasks", "notes", "members", "form", "webhook", "digest", "gallery"]);
    for (const choice of templateChoices) expect(choice.sentence).toMatch(/^[^.]+\.$/);
  });

  test("only the templates made for visitors without an app account are born public", () => {
    const visibility = Object.fromEntries(templateChoices.map(({ id }) => [id, born(id).manifest.visibility]));
    expect(visibility).toEqual({ tasks: "public", notes: "private", members: "private", form: "public", webhook: "private", digest: "private", gallery: "public" });
    for (const { id } of templateChoices) {
      const { visibility: chosen, runtime } = born(id).manifest;
      expect(runtime.auth.mode).toBe(chosen === "private" ? "required" : "optional");
    }
  });

  test("a project described from scratch starts private", () => {
    expect(cloneStarter().manifest.visibility).toBe("private");
    expect(cloneStarter().manifest.runtime.auth.mode).toBe("required");
  });

  for (const { id } of templateChoices) {
    test(`${id} ships an app.js the browser can parse`, () => {
      const AsyncFunction = Object.getPrototypeOf(async () => {}).constructor as new (body: string) => unknown;
      expect(() => new AsyncFunction(born(id).files["app.js"] as string)).not.toThrow();
    });
  }

  for (const { id } of templateChoices) {
    test(`${id} passes the Gate, empty and against a database built from its schema`, async () => {
      const project = born(id);
      const files = project.files as CapsuleFiles;
      const fresh = await validator.validate(files);
      expect(fresh.errors).toEqual([]);
      expect(fresh.ok).toBe(true);
      const live = await validator.validate(files, { state: "loaded", bytes: await exportDatabase(files["schema.sql"]) });
      expect(live.errors).toEqual([]);
      expect(live.checked).toEqual(["schema", "actions", "migration"]);
    });

    test(`${id} declares exactly its own actions in the manifest and keeps the project's identity`, () => {
      const project = born(id);
      const declared = project.manifest.runtime.database.actions.map((action) => action.name).sort();
      expect(declared).toEqual(Object.keys(parseServerActionSource(project.files["actions.json"]!).actions).sort());
      expect(project.id).toBe("app_template_test");
      expect(project.slug).toBe("test");
      for (const name of ["index.html", "styles.css", "app.js", "schema.sql", "actions.json"]) expect(project.files[name as keyof typeof project.files]).toBeTruthy();
    });

    test(`${id} calls only the actions it declares`, () => {
      const project = born(id);
      const declared = new Set(Object.keys(parseServerActionSource(project.files["actions.json"]!).actions));
      const called = [...project.files["app.js"]!.matchAll(/actions\.call\('([^']+)'/g)].map((match) => match[1]);
      expect(called.length).toBeGreaterThan(0);
      for (const name of called) expect(declared.has(name)).toBe(true);
    });
  }

  test("members writes its admin check once, as one scalar subquery, and checks the owner in its own rows", () => {
    const { actions } = JSON.parse(born("members").files["actions.json"]!) as { actions: Record<string, { statement: string; bindings: { source: string; field?: string }[] }> };
    const staff = Object.entries(actions).filter(([, action]) => action.statement.includes(adminCheck)).map(([name]) => name);
    expect(staff).toEqual(["posts.remove", "members.list"]);
    for (const action of Object.values(actions)) {
      expect(action.statement).not.toContain("EXISTS (SELECT 1 FROM profiles");
      expect(action.statement.split(adminCheck).length).toBeLessThanOrEqual(2);
    }
    for (const name of ["members.promote", "members.demote"]) {
      expect(actions[name].statement).toContain("(SELECT role FROM profiles WHERE user_id = ?2) = 'owner'");
      expect(actions[name].bindings[1]).toEqual({ source: "actor", field: "id" });
    }
  });

  test("the digest runs on its schedule, takes no input and emails each author their own entries", () => {
    const action = parseServerActionSource(born("digest").files["actions.json"]!).actions["digest.send"];
    expect(action.schedule).toEqual({ daily: "07:00" });
    expect(action.notify?.to).toEqual({ column: "user_id" });
    expect(action.notify?.changes).toEqual(["body"]);
    expect(action.notify?.body).toContain("{changes}");
    expect(action.notify?.body).toContain("{kernel.now}");
    expect(born("digest").files["app.js"]).not.toContain("digest.send");
  });

  test("the digest statement stamps entries from before today once, with the rows the notice lists", async () => {
    const SQL = await loadSql();
    const db = new SQL.Database();
    db.run(born("digest").files["schema.sql"]!);
    const action = parseServerActionSource(born("digest").files["actions.json"]!).actions["digest.send"];
    const statement = notifyReturningStatement(action.statement, notifyReturning(action.notify));
    const day = 86_400_000;
    const now = 20_000 * day + 7 * 3_600_000;
    expect(db.exec(statement, [now])).toEqual([]);
    db.run("INSERT INTO entries (user_id, body, created_at) VALUES ('ana', 'yesterday', ?), ('ben', 'also yesterday', ?), ('ana', 'today', ?)",
      [now - day, now - day + 1, now - 60_000]);
    expect(db.exec(statement, [now])[0].values).toEqual([["yesterday", "ana"], ["also yesterday", "ben"]]);
    expect(db.exec(statement, [now])).toEqual([]);
    expect(db.exec("SELECT body FROM entries WHERE digested_at IS NULL")[0].values).toEqual([["today"]]);
    db.close();
  });
});
