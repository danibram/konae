import { describe, expect, test } from "bun:test";
import initSqlJs, { type SqlJsStatic } from "sql.js";
import type { CapsuleFiles } from "../../agent/contracts";
import { createCapsuleValidator } from "../../agent/gate";
import { parseServerActionSource } from "../../kernel/actions";
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
      expect(project.databaseModel.source).toBe(project.databaseModel.acceptedSource);
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

  test("members writes its admin check once, as one scalar subquery, and keeps owner-only steps on actor.role", () => {
    const { actions } = JSON.parse(born("members").files["actions.json"]!) as { actions: Record<string, { statement: string; bindings: { source: string; field?: string }[] }> };
    const staff = Object.entries(actions).filter(([, action]) => action.statement.includes(adminCheck)).map(([name]) => name);
    expect(staff).toEqual(["posts.remove", "members.list"]);
    for (const action of Object.values(actions)) {
      expect(action.statement).not.toContain("EXISTS (SELECT 1 FROM profiles");
      expect(action.statement.split(adminCheck).length).toBeLessThanOrEqual(2);
    }
    for (const name of ["members.promote", "members.demote"]) {
      expect(actions[name].statement).toContain("?2 = 'owner'");
      expect(actions[name].bindings[1]).toEqual({ source: "actor", field: "role" });
    }
  });

  test("the digest runs on its schedule, takes no input and emails the owner", () => {
    const action = parseServerActionSource(born("digest").files["actions.json"]!).actions["digest.send"] as unknown as Record<string, unknown>;
    expect(action.schedule).toEqual({ daily: "07:00" });
    expect((action.notify as { to: string }).to).toBe("owner");
    expect(born("digest").files["app.js"]).not.toContain("digest.send");
  });

  test("the digest statement writes one row for yesterday and none when yesterday was empty", async () => {
    const SQL = await loadSql();
    const db = new SQL.Database();
    db.run(born("digest").files["schema.sql"]!);
    const statement = (JSON.parse(born("digest").files["actions.json"]!) as { actions: Record<string, { statement: string }> }).actions["digest.send"].statement;
    const day = 86_400_000;
    const now = 20_000 * day + 7 * 3_600_000;
    db.run(statement, [now]);
    expect(db.getRowsModified()).toBe(0);
    db.run("INSERT INTO entries (user_id, body, created_at) VALUES ('u', 'yesterday', ?), ('u', 'today', ?)", [now - day, now - 60_000]);
    db.run(statement, [now]);
    expect(db.exec("SELECT day, entries FROM digests")[0].values).toEqual([["2024-10-03", 1]]);
    db.run(statement, [now]);
    expect(db.getRowsModified()).toBe(0);
    db.close();
  });
});
