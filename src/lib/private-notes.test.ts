import { describe, expect, test } from "bun:test";
import { parseServerActionSource, resolveServerAction } from "../kernel/actions";
import { cloneStarter } from "./starter";
import { privateNotesDatabaseModelSource, withPrivateNotes } from "./private-notes";

describe("private notes starter", () => {
  test("transforms a project without changing its identity", () => {
    const original = cloneStarter();
    const project = withPrivateNotes(original);

    expect(project).not.toBe(original);
    expect(project.id).toBe(original.id);
    expect(project.name).toBe(original.name);
    expect(project.slug).toBe(original.slug);
    expect(project.databaseModel.source).toBe(privateNotesDatabaseModelSource);
    expect(project.databaseModel.acceptedSource).toBe(privateNotesDatabaseModelSource);
    expect(project.files["schema.sql"]).toContain("owner_id TEXT NOT NULL");
    expect(original.files["schema.sql"]).not.toContain("owner_id");
    expect(project.manifest.runtime.auth.mode).toBe("optional");
  });

  test("binds every notes operation to the authenticated actor", () => {
    const project = withPrivateNotes(cloneStarter());
    const program = parseServerActionSource(project.files["actions.json"]);

    expect(Object.values(program.actions).map(({ name, kind, access }) => ({ name, kind, access }))).toEqual([
      { name: "notes.list", kind: "query", access: "authenticated" },
      { name: "notes.create", kind: "mutation", access: "authenticated" },
      { name: "notes.delete", kind: "mutation", access: "authenticated" },
    ]);
    expect(resolveServerAction(program, "notes.list", {}, { actorId: "alice", now: 10 }).params).toEqual(["alice"]);
    expect(resolveServerAction(program, "notes.create", { title: "Private thought" }, { actorId: "alice", now: 10 }).params)
      .toEqual(["alice", "Private thought", 10]);
    expect(resolveServerAction(program, "notes.delete", { id: 4 }, { actorId: "alice", now: 10 }).params)
      .toEqual([4, "alice"]);
    expect(program.actions["notes.list"].statement).toContain("WHERE owner_id = ?");
    expect(program.actions["notes.delete"].statement).toContain("AND owner_id = ?");
    expect(() => resolveServerAction(program, "notes.create", { title: "Private", ownerId: "bob" }, { actorId: "alice" }))
      .toThrow("Unexpected");
    expect(() => resolveServerAction(program, "notes.delete", { id: 4, ownerId: "bob" }, { actorId: "alice" }))
      .toThrow("Unexpected");
    expect(() => resolveServerAction(program, "notes.list", {}, { actorId: null })).toThrow("authenticated");
  });

  test("offers public sign in and member note controls without file access", () => {
    const project = withPrivateNotes(cloneStarter());
    expect(project.files["index.html"]).toContain('id="landing"');
    expect(project.files["index.html"]).not.toContain("<form");
    expect(project.files["app.js"]).toContain("miniapp.auth.signIn()");
    expect(project.files["app.js"]).toContain("miniapp.auth.signOut()");
    expect(project.files["app.js"]).toContain("miniapp.actions.call('notes.list')");
    expect(project.files["app.js"]).toContain("miniapp.actions.call('notes.delete'");
    expect(project.files["app.js"]).toContain("addEventListener('keydown'");
    expect(project.files["app.js"]).not.toContain("addEventListener('submit'");
    expect(project.files["app.js"]).not.toContain("miniapp.files");
  });
});
