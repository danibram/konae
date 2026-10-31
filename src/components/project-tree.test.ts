import { describe, expect, test } from "bun:test";
import { buildTree, filesUnder, renamePath } from "./project-tree";

const tree = {
  files: { "app.js": "a", "src/features/tasks.ts": "t", "src/label.js": "l" },
  directories: ["src", "src/features", "src/empty"],
};

describe("project tree", () => {
  test("nests paths under their folders, folders first", () => {
    const nodes = buildTree([...Object.keys(tree.files), "schema.sql"], tree.directories);
    expect(nodes.map((node) => node.name)).toEqual(["src", "app.js", "schema.sql"]);
    const src = nodes[0];
    if (src.kind !== "directory") throw new Error("src should be a folder");
    expect(src.children.map((node) => node.name)).toEqual(["empty", "features", "label.js"]);
  });

  test("renames a file and creates the folders its new path needs", () => {
    const next = renamePath(tree, "src/label.js", "lib/text/label.js");
    expect(next.files["lib/text/label.js"]).toBe("l");
    expect(next.files["src/label.js"]).toBeUndefined();
    expect(next.directories).toContain("lib/text");
  });

  test("renames a folder with everything inside it", () => {
    const next = renamePath(tree, "src", "source");
    expect(Object.keys(next.files).sort()).toEqual(["app.js", "source/features/tasks.ts", "source/label.js"]);
    expect(next.directories).toEqual(["source", "source/empty", "source/features"]);
  });

  test("refuses bad or colliding paths", () => {
    expect(() => renamePath(tree, "src/label.js", "../evil.js")).toThrow("relative project path");
    expect(() => renamePath(tree, "src/label.js", "src/features/tasks.ts")).toThrow("already exists");
    expect(() => renamePath(tree, "src", "src/inner")).toThrow("inside itself");
    expect(() => renamePath(tree, "src/features/tasks.ts", "src/label.js/tasks.ts")).toThrow("is a file");
  });

  test("refuses to rename onto or away from a reserved project file", () => {
    expect(() => renamePath(tree, "src/label.js", "schema.sql")).toThrow("schema.sql is a reserved project file");
    expect(() => renamePath(tree, "src/label.js", "app.js")).toThrow("app.js is a reserved project file");
    expect(() => renamePath(tree, "app.js", "main.js")).toThrow("app.js is a reserved project file and cannot be renamed or moved.");
    expect(() => renamePath(tree, "src", "schema.sql")).toThrow("reserved");
  });

  /** The bug this pins: a rename to App.js or to a case variant of a sibling overwrote a file once exported to a case-insensitive filesystem. */
  test("compares reserved paths and collisions without case", () => {
    expect(() => renamePath(tree, "src/label.js", "App.js")).toThrow("app.js is a reserved project file");
    expect(() => renamePath(tree, "src/label.js", "Schema.SQL")).toThrow("schema.sql is a reserved project file");
    expect(() => renamePath(tree, "src/label.js", "SRC/Features/Tasks.ts")).toThrow("A file already exists at src/features/tasks.ts.");
    expect(() => renamePath(tree, "app.js", "x.js")).toThrow("reserved");
    expect(() => renamePath(tree, "src/label.js", "SRC")).toThrow("A folder already exists at src.");
    expect(() => renamePath(tree, "src/label.js", "APP.JS/x.js")).toThrow("reserved");
    const folders = { files: { "a/x.js": "1", "b/X.JS": "2" }, directories: ["a", "b"] };
    expect(() => renamePath(folders, "a", "B")).toThrow("A folder already exists at b.");
    // A case-only rename of the same file is allowed.
    expect(renamePath(tree, "src/label.js", "src/Label.js").files["src/Label.js"]).toBe("l");
    // A user file that only differs in case from a protected one can be moved out of the way.
    const legacy = { files: { "App.js": "old" }, directories: [] };
    expect(renamePath(legacy, "App.js", "legacy.js").files).toEqual({ "legacy.js": "old" });
  });

  /** The bug this pins: a folder only implied by its files did not count as taken, so a move overwrote or merged into it. */
  test("refuses to land on a folder that only exists through its files", () => {
    const implied = { files: { "lib/a.js": "a", "b.js": "b", "old/a.js": "old", "old/c.js": "c" }, directories: ["old"] };
    expect(() => renamePath(implied, "b.js", "lib")).toThrow("A folder already exists at lib.");
    expect(() => renamePath(implied, "old", "lib")).toThrow("A folder already exists at lib.");
    // Nothing collapses: the source tree is unchanged after a refusal.
    expect(implied.files["lib/a.js"]).toBe("a");
  });

  test("refuses a folder move whose contents would collide", () => {
    const nested = { files: { "x/a.js": "1", "y/x/a.js": "2", "z.js": "z" }, directories: ["x", "y", "y/x"] };
    expect(() => renamePath(nested, "x", "y/x")).toThrow("already exists");
    expect(() => renamePath(nested, "z.js", "y/x")).toThrow("A folder already exists at y/x.");
    expect(renamePath(nested, "x", "w").files).toEqual({ "w/a.js": "1", "y/x/a.js": "2", "z.js": "z" });
  });

  test("counts the files a folder delete would remove", () => {
    expect(filesUnder(tree.files, "src").sort()).toEqual(["src/features/tasks.ts", "src/label.js"]);
  });
});
