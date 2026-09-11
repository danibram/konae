import { describe, expect, test } from "bun:test";
import { applyLocalAgentEdit } from "./agent";
import { cloneStarter } from "./starter";

describe("local agent edits", () => {
  test("changes the accent without mutating the source project", () => {
    const source = cloneStarter();
    const originalCss = source.files["styles.css"];
    const edit = applyLocalAgentEdit(source, "ponlo azul");

    expect(edit.changedFiles).toEqual(["styles.css"]);
    expect(edit.project.files["styles.css"]).toContain("#3157d5");
    expect(source.files["styles.css"]).toBe(originalCss);
  });
});
