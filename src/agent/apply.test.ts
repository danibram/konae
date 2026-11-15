import { describe, expect, test } from "bun:test";
import { starterProject } from "../lib/starter";
import { textFiles } from "../lib/source-files";
import type { MiniAppProject } from "../lib/types";
import { applyProposal, mergeProposal } from "./apply";
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

describe("Apply over a project that holds a file that is not text", () => {
  const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
  const project: MiniAppProject = { ...starterProject, files: { ...starterProject.files, "public/logo.png": png }, encodings: { "public/logo.png": "base64" }, directories: ["public"] };

  test("keeps the file and its encoding when the agent changes a text file", () => {
    const proposed = { ...textFiles(project), "styles.css": "/* agent */" };
    const applied = applyProposal(project, proposalFor(proposed, ["styles.css"]), 7);
    expect(applied?.changedFiles).toEqual(["styles.css"]);
    expect(applied?.project.files["public/logo.png"]).toBe(png);
    expect(applied?.project.encodings).toEqual({ "public/logo.png": "base64" });
    expect(applied?.project.files["styles.css"]).toBe("/* agent */");
    expect(applied?.project.updatedAt).toBe(7);
  });

  test("drops the encoding when the agent writes text at the same path", () => {
    const proposed = { ...textFiles(project), "public/logo.png": "<svg/>" };
    const applied = applyProposal(project, proposalFor(proposed, ["public/logo.png"]));
    expect(applied?.project.files["public/logo.png"]).toBe("<svg/>");
    expect(applied?.project.encodings).toBeUndefined();
  });

  test("answers null when the merge refuses", () => {
    expect(applyProposal(project, proposalFor(textFiles(project), []))).toBeNull();
  });
});
