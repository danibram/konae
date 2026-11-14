import { describe, expect, test } from "bun:test";
import { cloneStarter } from "../lib/starter";
import { suggestedChanges } from "./agent-suggestions";

describe("suggestedChanges", () => {
  test("a public list anyone can add to is not offered that change again", () => {
    const source = JSON.parse(cloneStarter().files["actions.json"]);
    for (const name of Object.keys(source.actions)) source.actions[name].access = "anonymous";
    const files = { "actions.json": JSON.stringify(source) };
    const offered = suggestedChanges({ files, manifest: { visibility: "public" } });
    expect(offered).not.toContain("Let anyone with the link add a row.");
    expect(offered).not.toContain("Let people mark a row as done.");
    expect(offered).toHaveLength(3);
  });

  test("a private app is not offered a sign-in it already asks for", () => {
    const offered = suggestedChanges({ files: {}, manifest: { visibility: "private" } });
    expect(offered).not.toContain("Ask people to sign in before the app opens.");
    expect(offered).not.toContain("Let anyone with the link add a row.");
  });

  test("a message form offers changes about messages", () => {
    const actions = { version: 1, actions: { "messages.list": { kind: "query", access: "authenticated", statement: "SELECT * FROM messages LIMIT 100", bindings: [] } } };
    const offered = suggestedChanges({ files: { "actions.json": JSON.stringify(actions) }, manifest: { visibility: "public" } });
    expect(offered).toEqual(["Email me when someone sends a message.", "Let me mark messages as read.", "Add a search box above the messages."]);
  });

  test("a file the check refuses still gets suggestions", () => {
    expect(suggestedChanges({ files: { "actions.json": "{" }, manifest: { visibility: "public" } })).toHaveLength(3);
  });
});
