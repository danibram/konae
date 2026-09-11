import { describe, expect, test } from "bun:test";
import { starterProject } from "../lib/starter";
import { mergeProposal } from "./apply";
import type { AgentProposal, CapsuleFiles } from "./contracts";

const capsule = (overrides: Partial<CapsuleFiles> = {}): CapsuleFiles => ({ ...starterProject.files, ...overrides });

const proposalFor = (files: CapsuleFiles, changedFiles: AgentProposal["changedFiles"], ok = true): AgentProposal => ({
  files,
  changedFiles,
  validation: { ok, errors: ok ? [] : [{ stage: "schema", message: "nope" }] },
});

describe("landing a proposal", () => {
  test("writes the changed files and leaves the rest of the Capsule alone", () => {
    const current = capsule();
    const proposed = capsule({ "styles.css": "/* agent */" });
    const merge = mergeProposal(current, proposalFor(proposed, ["styles.css"]));

    expect(merge.ok).toBe(true);
    expect(merge.ok && merge.files["styles.css"]).toBe("/* agent */");
    expect(merge.ok && merge.files["app.js"]).toBe(current["app.js"]);
  });

  test("an unvalidated or empty proposal is never written", () => {
    const current = capsule();
    const proposed = capsule({ "styles.css": "/* agent */" });
    expect(mergeProposal(current, proposalFor(proposed, ["styles.css"], false))).toEqual({ ok: false, reason: "unvalidated" });
    expect(mergeProposal(current, proposalFor(proposed, []))).toEqual({ ok: false, reason: "empty" });
    expect(mergeProposal(current, { files: proposed, changedFiles: ["styles.css"], validation: null }))
      .toEqual({ ok: false, reason: "unvalidated" });
  });

  /**
   * The bug this pins: an action validated against the schema the agent was
   * shown was written onto a schema the owner had edited in the meantime, and
   * the panel still reported the combination as validated.
   */
  test("refuses when a file the proposal did not change has moved since it was validated", () => {
    const seen = capsule({ "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER, due_at INTEGER);" });
    const proposal = proposalFor({ ...seen, "actions.json": "{ \"version\": 1, \"actions\": {} }" }, ["actions.json"]);

    const handEdited = capsule({ "schema.sql": "CREATE TABLE IF NOT EXISTS tasks (id INTEGER);" });
    expect(mergeProposal(handEdited, proposal)).toEqual({ ok: false, reason: "drifted" });
    // The same proposal is still applicable to the Capsule it was validated against.
    expect(mergeProposal(seen, proposal).ok).toBe(true);
  });

  test("a hand edit to a file the proposal also changed is overwritten, not a drift", () => {
    const seen = capsule();
    const proposal = proposalFor(capsule({ "styles.css": "/* agent */" }), ["styles.css"]);
    const merge = mergeProposal(capsule({ "styles.css": "/* by hand */" }), proposal);
    expect(merge.ok && merge.files["styles.css"]).toBe("/* agent */");
    expect(seen["styles.css"]).not.toBe("/* agent */");
  });
});
