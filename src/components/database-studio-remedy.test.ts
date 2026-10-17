import { describe, expect, test } from "bun:test";
import { hintOf, refusal } from "../kernel/refusal";
import { McpError } from "../mcp/client";
import { refusalRemedy } from "./DatabaseStudio";

const sentence = "Table habits has a CHECK that names column note, which this migration removes. Rebuild habits without that CHECK in the SQL console, or keep note, then apply the migration again.";

describe("the Database Studio shows the remedy beside a refused migration", () => {
  const remedy = hintOf("schema.check-names-removed-column").remedy;

  test("from the preview or the plan, which refuse with a RefusalError", () => {
    expect(refusalRemedy(refusal("schema.check-names-removed-column", sentence))).toBe(remedy);
  });

  test("from the Kernel, which answers the code through MCP", () => {
    const answered = new McpError(sentence, undefined, { code: "schema.check-names-removed-column", message: sentence, hint: hintOf("schema.check-names-removed-column") });
    expect(refusalRemedy(answered)).toBe(remedy);
  });

  test("and nothing for a failure no rule refused", () => {
    expect(refusalRemedy(new McpError("Unknown database operation."))).toBe("");
    expect(refusalRemedy(new Error("The migration plan has no statements."))).toBe("");
  });
});
