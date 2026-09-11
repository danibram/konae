import { describe, expect, test } from "bun:test";
import { cloneStarter, starterProject } from "../lib/starter";
import { buildInitialUserMessage, buildSystemPrompt } from "./system-prompt";

describe("system prompt", () => {
  test("is a stable cache prefix: no timestamps, ids or file contents", () => {
    const first = buildSystemPrompt(cloneStarter());
    const second = buildSystemPrompt({ ...cloneStarter(), id: "app_other", updatedAt: 1 });
    expect(first).toBe(second);
    expect(first).not.toContain(starterProject.id);
    expect(first).not.toContain(starterProject.files["actions.json"].trim());
    expect(first).not.toContain("#e65337");
    expect(/\d{10,}/.test(first)).toBe(false);
  });

  test("states the Capsule contract the Validation Gate enforces", () => {
    const prompt = buildSystemPrompt(cloneStarter());
    for (const fragment of [
      "/workspace/index.html",
      "/workspace/actions.json",
      "never receives SQL",
      "miniapp.actions.call",
      "validate_capsule",
      "At most 50 actions",
      '"source": "actor"',
      '"nullable": true',
      "mutation statements using RETURNING fail validation",
      "top-level LIMIT",
      "name each new action as personal or shared",
    ]) {
      expect(prompt).toContain(fragment);
    }
    expect(prompt).toContain("public deployment");
    // The Kernel refuses to deploy "granted", so the prompt must not offer it.
    expect(prompt).not.toContain('"granted"');
  });
});

describe("initial user message", () => {
  test("seeds all project files, including nested files, and the request in one turn", () => {
    const project = cloneStarter();
    project.files["src/features/filter.ts"] = "export const all = () => true;";
    const message = buildInitialUserMessage(project, "  Make the accent ink blue.  ");
    for (const file of ["index.html", "styles.css", "app.js", "schema.sql", "actions.json"]) {
      expect(message).toContain(`/workspace/${file}\n`);
    }
    expect(message).toContain("/workspace/src/features/filter.ts\n```ts");
    expect(message).toContain(starterProject.files["schema.sql"].trim());
    expect(message).toContain("```html");
    expect(message).toContain("```json");
    expect(message.trimEnd().endsWith("Make the accent ink blue.")).toBe(true);
  });

  test("cannot be broken out of by backticks inside a file", () => {
    const project = cloneStarter();
    project.files["app.js"] = "const template = ```\nnot a fence\n```;";
    const message = buildInitialUserMessage(project, "tidy up");
    expect(message).toContain("````js\n");
    expect(message).toContain("\n````");
  });
});
