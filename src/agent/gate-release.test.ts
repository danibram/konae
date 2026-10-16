import { describe, expect, test } from "bun:test";
import initSqlJs from "sql.js";
import { parseServerActionSource } from "../kernel/actions";
import type { DomainRules } from "../lib/domain-name";
import { createCapsuleValidator, refusalNeedsSecret, releaseRefusal, type ReleaseFacts } from "./gate";

type Case = { case: string; actions: Record<string, unknown>; secrets: NonNullable<ReleaseFacts["secrets"]>; error: string | null };
const file: { rules: DomainRules; cases: Case[] } = await Bun.file(new URL("../../kernel-rs/src/release-cases.json", import.meta.url)).json();

const schema = "CREATE TABLE IF NOT EXISTS orders (id INTEGER PRIMARY KEY, external_id TEXT);";
const validator = createCapsuleValidator(() => initSqlJs());

function capsule(actions: Record<string, unknown>) {
  return { "index.html": "<!doctype html><title>t</title>", "schema.sql": schema, "actions.json": JSON.stringify({ version: 1, actions }) } as never;
}

describe("the release plan at the editor's door", () => {
  // kernel-rs/src/http.rs `require_secrets` and calls.rs `check_release` give these answers at release_plan.
  test("every case in kernel-rs/src/release-cases.json gets the Kernel's answer", () => {
    expect(file.cases.length).toBeGreaterThanOrEqual(20);
    for (const item of file.cases) {
      const program = parseServerActionSource(JSON.stringify({ version: 1, actions: item.actions }));
      expect(releaseRefusal(program, { secrets: item.secrets, rules: file.rules }), item.case).toEqual(item.error);
    }
  });

  test("the Validation Gate gives every case the Kernel's words when it knows the facts", async () => {
    for (const item of file.cases) {
      const result = await validator.validate(capsule(item.actions), { state: "absent" }, { secrets: item.secrets, rules: file.rules });
      const said = [...result.errors.map((error) => error.message), ...(result.warnings ?? []).map((warning) => warning.message)];
      expect(said, item.case).toEqual(item.error === null ? [] : [item.error]);
    }
  });

  // The owner's decision: no edit to the files clears a secret, so the agent may
  // apply and says what to set. Deploy still refuses (action-readiness.ts).
  test("a refusal only the owner can clear is a warning; one the files clear stays an error", async () => {
    let owner = 0;
    for (const item of file.cases) {
      if (item.error === null) continue;
      const result = await validator.validate(capsule(item.actions), { state: "absent" }, { secrets: item.secrets, rules: file.rules });
      if (refusalNeedsSecret(item.error)) {
        owner += 1;
        expect(result.ok, item.case).toBe(true);
        expect(result.errors, item.case).toEqual([]);
        expect(result.warnings, item.case).toEqual([{ stage: "actions", message: item.error, detail: "Deploy refuses this until the owner sets the secret in Auth > Integrations. No edit to the files clears it.", owner: true }]);
      } else {
        expect(result.ok, item.case).toBe(false);
        expect(result.warnings, item.case).toBeUndefined();
      }
    }
    expect(owner).toBeGreaterThan(0);
  });

  test("without secret names the Gate says nothing about secrets", async () => {
    const unset = file.cases.find((item) => item.case === "a signed route whose secret is not set")!;
    expect((await validator.validate(capsule(unset.actions))).warnings).toBeUndefined();
    expect((await validator.validate(capsule(unset.actions), { state: "absent" }, { rules: file.rules })).warnings).toBeUndefined();
    const keyed = file.cases.find((item) => item.case === "a call that sends an API key that is not set")!;
    expect((await validator.validate(capsule(keyed.actions), { state: "absent" }, { rules: file.rules })).warnings).toBeUndefined();
  });

  test("without the installation's hosts the Gate says nothing about a call's host", async () => {
    for (const name of ["a call to the editor's public host", "a call over http outside the installation"]) {
      const item = file.cases.find((candidate) => candidate.case === name)!;
      expect((await validator.validate(capsule(item.actions), { state: "absent" }, { secrets: [] })).errors, name).toEqual([]);
    }
  });

  test("a refusal says who must act", async () => {
    const unset = file.cases.find((item) => item.case === "a signed route whose secret is not set")!;
    const [owner] = (await validator.validate(capsule(unset.actions), { state: "absent" }, { secrets: [] })).warnings!;
    expect(owner.owner).toBe(true);
    expect(owner.detail).toBe("Deploy refuses this until the owner sets the secret in Auth > Integrations. No edit to the files clears it.");
    const svix = file.cases.find((item) => item.case === "a svix route whose secret is not a whsec_ key")!;
    const [key] = (await validator.validate(capsule(svix.actions), { state: "absent" }, { secrets: svix.secrets })).warnings!;
    expect(key.owner).toBe(true);
    expect(key.detail).toBe("Deploy refuses this until the owner sets the secret in Auth > Integrations. No edit to the files clears it.");
    const editor = file.cases.find((item) => item.case === "a call to the editor's public host")!;
    const [author] = (await validator.validate(capsule(editor.actions), { state: "absent" }, { rules: file.rules })).errors;
    expect(author.detail).toBe("The Kernel refuses this at publish. Change actions.json.");
  });

  test("a parse refusal stops the run before the release check", async () => {
    const result = await validator.validate(capsule({ "x.y": { kind: "query", access: "anonymous", statement: "SELECT 1", bindings: [], guard: "No." } }), { state: "absent" }, { secrets: [] });
    expect(result.errors).toHaveLength(1);
    expect(result.errors[0].message).toBe("actions.json is not a valid server action program.");
  });
});
