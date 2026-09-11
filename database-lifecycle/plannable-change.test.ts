import { describe, expect, test } from "bun:test";
import { assertPlannableChange } from "./plannable-change";

const snapshot = (tables: Record<string, string[]>, views: string[] = []) => ({
  tables: Object.fromEntries(
    Object.entries(tables).map(([name, columns]) => [name, { name, columns: Object.fromEntries(columns.map((c) => [c, {}])) }]),
  ),
  views: Object.fromEntries(views.map((name) => [name, {}])),
});

describe("deciding whether a change can be planned without asking", () => {
  test("accepts an added column", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id", "title"] }), snapshot({ tasks: ["id", "title", "due_at"] }))).not.toThrow();
  });

  test("accepts a removal on its own", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id", "title"], notes: ["id"] }), snapshot({ tasks: ["id"] }))).not.toThrow();
  });

  test("accepts an addition on its own", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id"] }), snapshot({ tasks: ["id"], notes: ["id"] }))).not.toThrow();
  });

  test("refuses a column that disappears while another arrives in the same table", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id", "title"] }), snapshot({ tasks: ["id", "label"] })))
      .toThrow(/a column of tasks/);
  });

  test("refuses a table that disappears while another arrives", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id"] }), snapshot({ notes: ["id"] }))).toThrow(/a table/);
  });

  test("refuses a view that disappears while another arrives", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id"] }, ["open"]), snapshot({ tasks: ["id"] }, ["closed"]))).toThrow(/a view/);
  });

  test("ignores columns of a table that is itself only added", () => {
    expect(() => assertPlannableChange(snapshot({ tasks: ["id"] }), snapshot({ tasks: ["id"], notes: ["body"] }))).not.toThrow();
  });

  test("tolerates a snapshot without tables or views", () => {
    expect(() => assertPlannableChange({}, snapshot({ tasks: ["id"] }))).not.toThrow();
  });
});
