import { describe, expect, test } from "bun:test";
import { cloneStarter, starterProject } from "../lib/starter";
import { assembleStaticArtifact, runtimeFile, servedPath } from "../lib/static-artifact.js";
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
      "At most 100 actions",
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

describe("what the prompt says a Deployment serves", () => {
  const prompt = buildSystemPrompt(cloneStarter());

  test("names pages under pages/, public/ and 404.html, with the paths the build serves them at", () => {
    expect(prompt).toContain("pages/guide/install.html answers at /guide/install.html and /guide/install");
    expect(servedPath("pages/guide/install.html")).toBe("guide/install.html");
    expect(prompt).toContain("public/robots.txt answers at /robots.txt");
    expect(servedPath("public/robots.txt")).toBe("robots.txt");
    expect(prompt).toContain("/workspace/404.html");
    expect(servedPath("404.html")).toBe("404.html");
    // An .html outside the root and pages/ is not a page, as the prompt says.
    expect(prompt).toContain("An .html file anywhere else is not a page.");
    expect(servedPath("src/partial.html")).toBeNull();
  });

  test("warns about the build's refusals the Gate does not check, in the build's words", async () => {
    const files: Record<string, string> = { "index.html": "<main id=app></main>", "about.html": '<img src="/team.jpg" alt="">' };
    const build = assembleStaticArtifact({
      paths: Object.keys(files),
      read: async (path: string) => files[path],
      compile: async () => ({ styles: "", script: "" }),
      bundle: async () => ({ script: "", styles: "" }),
      runtime: "",
    });
    await expect(build).rejects.toThrow("about.html references /team.jpg, but the Capsule holds no team.jpg.");
    expect(prompt).toContain("never create runtime.js or public/runtime.js");
    expect(runtimeFile).toBe("runtime.js");
    expect(prompt).toContain("about.html references /team.jpg, but the Capsule holds no team.jpg.");
    expect(prompt).toContain("This editor holds text files only.");
  });

  test("shows guard, signal and http in the actions.json format, and that a signal action has no statement", () => {
    for (const key of ['"guard":', '"signal":', '"http":', "a signal action has none"]) expect(prompt).toContain(key);
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
