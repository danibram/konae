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
});
