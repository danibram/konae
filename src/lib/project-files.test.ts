import { describe, expect, test } from "bun:test";
import { createDirectory, createFile, normalizeProjectPath, removePath } from "./project-files";

describe("project file tree", () => {
  test("an owner creates nested files and empty directories through project paths", () => {
    expect(normalizeProjectPath(" /src/features/tasks.ts ")).toBe("src/features/tasks.ts");
    expect(normalizeProjectPath("../../secret")).toBeNull();

    const withDirectory = createDirectory({ files: {}, directories: [] }, "src/features");
    const withFile = createFile(withDirectory, "src/features/tasks.ts", "export const tasks = [];");

    expect(withFile.directories).toEqual(["src", "src/features"]);
    expect(withFile.files).toEqual({ "src/features/tasks.ts": "export const tasks = [];" });
  });

  test("removing a directory removes its descendants without touching sibling paths", () => {
    const tree = {
      directories: ["src", "src/features", "src/shared"],
      files: {
        "src/features/tasks.ts": "tasks",
        "src/shared/dom.ts": "dom",
        "index.html": "home",
      },
    };

    expect(removePath(tree, "src/features")).toEqual({
      directories: ["src", "src/shared"],
      files: { "src/shared/dom.ts": "dom", "index.html": "home" },
    });
  });

  test("refuses to create a reserved project file or anything inside one", () => {
    const tree = { files: { "app.js": "app" }, directories: [] };
    expect(() => createFile(tree, "schema.sql")).toThrow("schema.sql is a reserved project file. Choose another name.");
    expect(() => createFile(tree, "/schema.sql/")).toThrow("schema.sql is a reserved project file");
    expect(() => createDirectory(tree, "schema.sql")).toThrow("schema.sql is a reserved project file");
    expect(() => createFile(tree, "actions.json/x.json")).toThrow("actions.json is a reserved project file");
    expect(createFile(tree, "src/schema.ts").files["src/schema.ts"]).toBe("");
  });

  /** The bug this pins: App.js and SCHEMA.SQL passed, then overwrote the real files on a case-insensitive filesystem. */
  test("compares reserved paths and collisions without case", () => {
    const tree = { files: { "app.js": "app", "src/Label.js": "l" }, directories: ["docs"] };
    expect(() => createFile(tree, "App.js")).toThrow("app.js is a reserved project file");
    expect(() => createFile(tree, "SCHEMA.SQL")).toThrow("schema.sql is a reserved project file");
    expect(() => createFile(tree, "Index.html")).toThrow("index.html is a reserved project file");
    expect(() => createDirectory(tree, "Actions.JSON/x")).toThrow("actions.json is a reserved project file");
    expect(() => createFile(tree, "src/label.js")).toThrow("A file already exists at src/Label.js.");
    expect(() => createFile(tree, "SRC/LABEL.JS")).toThrow("A file already exists at src/Label.js.");
    expect(() => createFile(tree, "Docs")).toThrow("A directory already exists at docs.");
    expect(() => createFile(tree, "Src")).toThrow("A directory already exists at src.");
    expect(() => createFile(tree, "src/Label.JS/x.js")).toThrow("src/Label.js is a file");
    expect(() => createDirectory(tree, "SRC/label.js")).toThrow("A file already exists at src/Label.js.");
    expect(createFile(tree, "src/label2.js").files["src/label2.js"]).toBe("");
  });

  test("path errors read as one sentence", () => {
    expect(() => createFile({ files: {}, directories: [] }, "../x.js")).toThrow('Use a relative project path, without "." or ".." segments.');
  });

  test("refuses a file or directory under an existing file", () => {
    const tree = { files: { "lib.js": "lib" }, directories: [] };
    expect(() => createFile(tree, "lib.js/x.js")).toThrow("lib.js is a file");
    expect(() => createDirectory(tree, "lib.js/sub")).toThrow("lib.js is a file");
  });
});
