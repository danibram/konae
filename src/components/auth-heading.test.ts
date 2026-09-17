import { describe, expect, test } from "bun:test";
import { parseServerActionSource } from "../kernel/actions";
import { withPrivateNotes } from "../lib/private-notes";
import { cloneStarter } from "../lib/starter";
import { authHeading } from "./auth-heading";

const list = (access: string, statement = "SELECT id FROM tasks ORDER BY id LIMIT 100") => ({ name: "tasks.list", kind: "query", access, statement });

describe("authHeading", () => {
  test("names anyone when the list is anonymous", () => {
    expect(authHeading([list("anonymous")])).toBe("Anyone can list the rows. The list stops at 100 rows.");
  });

  test("names a signed-in person for the private notes template", () => {
    const actions = Object.values(parseServerActionSource(withPrivateNotes(cloneStarter()).files["actions.json"]).actions);
    expect(authHeading(actions)).toBe("A signed-in person can list the rows. The list stops at 100 rows.");
  });

  test("reads the limit from the top level only", () => {
    expect(authHeading([list("anonymous", "SELECT id FROM tasks WHERE id IN (SELECT id FROM tasks LIMIT 5) LIMIT ?")])).toBe("Anyone can list the rows.");
  });

  test("makes no claim without a query", () => {
    expect(authHeading([{ name: "tasks.create", kind: "mutation", access: "anonymous", statement: "INSERT INTO tasks (title) VALUES (?)" }]))
      .toBe("Choose who can open the app and who can run each action.");
  });
});
