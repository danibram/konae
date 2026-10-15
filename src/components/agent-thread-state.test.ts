import { describe, expect, test } from "bun:test";
import type { AgentProposal, AgentUiEvent, ApplyGateStatus, ValidationResult } from "../agent/contracts";
import { defaultModelIdFor, isCustomModelId, keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import {
  agentThreadReducer,
  canSubmit,
  carriesSnapshot,
  initialAgentThreadState,
  proposalFooter,
  validationSummary,
  submitBlocker,
  type AgentThreadAction,
  type AgentThreadSnapshot,
  type AgentThreadState,
} from "./agent-thread-state";

function run(actions: AgentThreadAction[], from: AgentThreadState = initialAgentThreadState) {
  return actions.reduce(agentThreadReducer, from);
}

function events(list: AgentUiEvent[], snapshot?: AgentThreadSnapshot): AgentThreadAction[] {
  return list.map((event) => ({ type: "event", event, snapshot }));
}

const files = {
  "index.html": "<main></main>",
  "styles.css": "body{}",
  "app.js": "",
  "schema.sql": "create table t (id integer primary key);",
  "actions.json": "[]",
};

function proposal(validation: ValidationResult | null = null): AgentProposal {
  return { files, changedFiles: ["styles.css"], validation };
}

function snapshot(gate: ApplyGateStatus, validation: ValidationResult | null = null): AgentThreadSnapshot {
  return { proposal: proposal(validation), gate };
}

describe("agent thread thread", () => {
  test("accumulates assistant deltas into one streaming message", () => {
    const state = run([
      { type: "prompt", text: "make the accent blue" },
      ...events([
        { type: "assistant-delta", text: "Changing " },
        { type: "assistant-delta", text: "the accent" },
      ]),
    ]);

    expect(state.messages.map((message) => [message.role, message.text])).toEqual([
      ["user", "make the accent blue"],
      ["assistant", "Changing the accent"],
    ]);
    expect(state.messages[1].streaming).toBe(true);
    expect(state.busy).toBe(true);
  });

  test("turn-end closes the stream so the next delta opens a new message", () => {
    const state = run([
      { type: "prompt", text: "hi" },
      ...events([
        { type: "assistant-delta", text: "one" },
        { type: "turn-end" },
        { type: "assistant-delta", text: "two" },
      ]),
    ]);

    expect(state.messages.filter((message) => message.role === "assistant").map((message) => message.text))
      .toEqual(["one", "two"]);
    expect(state.messages.every((message) => !message.streaming || message.text === "two")).toBe(true);
  });

  test("ignores empty deltas and blank prompts", () => {
    const state = run([
      { type: "prompt", text: "   " },
      ...events([{ type: "assistant-delta", text: "" }]),
    ]);
    expect(state).toBe(initialAgentThreadState);
  });

  test("message ids are unique across a conversation", () => {
    const state = run([
      { type: "prompt", text: "a" },
      ...events([{ type: "assistant-delta", text: "x" }, { type: "turn-end" }]),
      { type: "settled" },
      { type: "prompt", text: "b" },
      ...events([{ type: "assistant-delta", text: "y" }]),
    ]);
    expect(new Set(state.messages.map((message) => message.id)).size).toBe(state.messages.length);
  });

  test("a new prompt clears the previous turn's tool chips", () => {
    const state = run([
      { type: "prompt", text: "a" },
      ...events([
        { type: "tool-start", id: "call_0", name: "edit", detail: "styles.css" },
        { type: "tool-end", id: "call_0", name: "edit", ok: true, detail: "1 change" },
      ]),
      { type: "settled" },
      { type: "prompt", text: "b" },
    ]);
    expect(state.tools).toEqual([]);
    expect(state.messages).toHaveLength(2);
  });
});

describe("agent thread tool chips", () => {
  test("a chip moves from running to ok and keeps the end detail", () => {
    const started = run([
      { type: "prompt", text: "edit" },
      ...events([{ type: "tool-start", id: "call_0", name: "edit", detail: "styles.css" }]),
    ]);
    expect(started.tools).toMatchObject([{ name: "edit", detail: "styles.css", state: "running" }]);

    const ended = agentThreadReducer(started, {
      type: "event",
      event: { type: "tool-end", id: "call_0", name: "edit", ok: true, detail: "1 replacement" },
    });
    expect(ended.tools).toMatchObject([{ name: "edit", detail: "1 replacement", state: "ok" }]);
  });

  test("a failed tool keeps its start detail when the end carries none", () => {
    const state = run([
      { type: "prompt", text: "edit" },
      ...events([
        { type: "tool-start", id: "call_0", name: "validate_capsule", detail: "schema.sql" },
        { type: "tool-end", id: "call_0", name: "validate_capsule", ok: false, detail: "" },
      ]),
    ]);
    expect(state.tools).toMatchObject([{ name: "validate_capsule", detail: "schema.sql", state: "fail" }]);
  });

  test("concurrent calls of one tool settle on their own chip", () => {
    const state = run([
      { type: "prompt", text: "read both" },
      ...events([
        { type: "tool-start", id: "call_0", name: "read", detail: "index.html" },
        { type: "tool-start", id: "call_1", name: "read", detail: "app.js" },
        { type: "tool-end", id: "call_1", name: "read", ok: true, detail: "app.js read" },
        { type: "tool-end", id: "call_0", name: "read", ok: false, detail: "index.html failed" },
      ]),
    ]);
    expect(state.tools.map((chip) => [chip.detail, chip.state])).toEqual([
      ["index.html failed", "fail"],
      ["app.js read", "ok"],
    ]);
    expect(new Set(state.tools.map((chip) => chip.id)).size).toBe(2);
  });

  test("an unmatched tool-end changes nothing", () => {
    const started = run([{ type: "prompt", text: "x" }, ...events([{ type: "tool-start", id: "call_0", name: "edit", detail: "a" }])]);
    const same = agentThreadReducer(started, {
      type: "event",
      event: { type: "tool-end", id: "call_9", name: "edit", ok: true, detail: "b" },
    });
    expect(same.tools).toBe(started.tools);
  });

  test("settling a run never leaves a chip spinning", () => {
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "tool-start", id: "call_0", name: "edit", detail: "styles.css" }]),
      { type: "settled" },
    ]);
    expect(state.tools).toMatchObject([{ state: "fail" }]);
    expect(state.busy).toBe(false);
  });
});

describe("agent thread errors", () => {
  test("an error event ends the run, closes the stream and can be dismissed", () => {
    const failed = run([
      { type: "prompt", text: "x" },
      ...events([
        { type: "assistant-delta", text: "partial" },
        { type: "tool-start", id: "call_0", name: "edit", detail: "styles.css" },
        { type: "error", message: "401 invalid api key" },
      ]),
    ]);

    expect(failed.error).toBe("401 invalid api key");
    expect(failed.busy).toBe(false);
    expect(failed.messages[1]).toMatchObject({ text: "partial", streaming: false });
    expect(failed.tools).toMatchObject([{ state: "fail" }]);

    expect(agentThreadReducer(failed, { type: "dismiss-error" }).error).toBe("");
  });

  test("a new prompt clears a previous error", () => {
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "error", message: "boom" }]),
      { type: "prompt", text: "again" },
    ]);
    expect(state.error).toBe("");
    expect(state.busy).toBe(true);
  });

  test("reset returns the initial state", () => {
    const state = run([{ type: "prompt", text: "x" }, { type: "reset" }]);
    expect(state).toEqual(initialAgentThreadState);
  });
});

describe("agent thread apply gate", () => {
  const failing: ValidationResult = {
    ok: false,
    checked: ["schema", "actions"],
    errors: [{ stage: "migration", message: "table task is missing column due_at", detail: "after migration" }],
  };
  const passing: ValidationResult = { ok: true, errors: [], checked: ["schema", "actions", "migration"] };

  test("no proposal means an empty footer", () => {
    expect(proposalFooter(initialAgentThreadState)).toEqual({ kind: "empty", canApply: false });
  });

  test("an unvalidated proposal cannot be applied", () => {
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "turn-end" }], snapshot({ state: "unvalidated", changedFiles: ["styles.css"] })),
    ]);
    expect(proposalFooter(state)).toEqual({ kind: "unvalidated", changedFiles: ["styles.css"], canApply: false });
    expect(state.proposal).not.toBeNull();
  });

  test("a blocked gate surfaces the first error with its stage", () => {
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "validation", result: failing }], snapshot({ state: "blocked", changedFiles: ["schema.sql"], result: failing }, failing)),
    ]);
    expect(proposalFooter(state)).toEqual({
      kind: "blocked",
      changedFiles: ["schema.sql"],
      canApply: false,
      stage: "migration",
      message: "table task is missing column due_at",
      detail: "after migration",
    });
  });

  test("a pass with a secret only the owner can set says so, and Apply stays open", () => {
    const warned: ValidationResult = { ok: true, errors: [], checked: ["schema", "actions"], warnings: [
      { stage: "actions", message: "A result column has no name." },
      { stage: "actions", message: "Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.", detail: "Deploy refuses this until the owner sets the secret in Auth > Integrations. No edit to the files clears it.", owner: true },
    ] };
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "validation", result: warned }], snapshot({ state: "ready", changedFiles: ["actions.json"], result: warned }, warned)),
    ]);
    expect(proposalFooter(state)).toEqual({
      kind: "validated",
      changedFiles: ["actions.json"],
      canApply: true,
      checked: ["schema", "actions"],
      owner: {
        message: "Action orders.receive signs with the secret orders_webhook, which is not set for this project. Set it with secret_set before publishing.",
        detail: "Deploy refuses this until the owner sets the secret in Auth > Integrations. No edit to the files clears it.",
      },
    });
  });

  test("a blocked gate carries the reason under the stage message, so the Kernel's sentence reaches the person", () => {
    const refused: ValidationResult = { ok: false, checked: ["schema", "actions"], errors: [{
      stage: "actions",
      message: "actions.json is not a valid server action program.",
      detail: "Action tasks.list is a query, so it cannot declare a guard. Declare it on a mutation.",
    }] };
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "validation", result: refused }], snapshot({ state: "blocked", changedFiles: ["actions.json"], result: refused }, refused)),
    ]);
    expect(proposalFooter(state)).toMatchObject({
      kind: "blocked",
      message: "actions.json is not a valid server action program.",
      detail: "Action tasks.list is a query, so it cannot declare a guard. Declare it on a mutation.",
    });
  });

  test("a failing validation event blocks a gate that is still unvalidated", () => {
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "validation", result: failing }], snapshot({ state: "unvalidated", changedFiles: ["schema.sql"] })),
    ]);
    expect(proposalFooter(state).kind).toBe("blocked");
  });

  test("a passing validation event never upgrades an unvalidated gate", () => {
    const state = run([
      { type: "prompt", text: "x" },
      ...events([{ type: "validation", result: passing }], snapshot({ state: "unvalidated", changedFiles: ["schema.sql"] })),
    ]);
    expect(proposalFooter(state).canApply).toBe(false);
  });

  test("only a ready gate enables Apply", () => {
    const state = run([
      { type: "prompt", text: "x" },
      { type: "settled", snapshot: snapshot({ state: "ready", changedFiles: ["styles.css"], result: passing }, passing) },
    ]);
    expect(proposalFooter(state)).toEqual({
      kind: "validated",
      changedFiles: ["styles.css"],
      canApply: true,
      checked: ["schema", "actions", "migration"],
    });
    expect(state.proposal?.changedFiles).toEqual(["styles.css"]);
  });

  /**
   * The bug this pins: the footer said "schema, migration and actions hold" for
   * any pass, including one where the live database could not be read and the
   * migration stage never ran.
   */
  test("a pass only claims the stages that ran", () => {
    expect(validationSummary(["schema", "actions", "migration"])).toBe("Validated — schema, migration and actions hold.");
    expect(validationSummary(["schema", "actions"])).toBe("Validated — schema and actions hold — migration not checked.");
    expect(validationSummary([])).toBe("Validated.");
  });

  test("discard empties the footer but keeps the thread", () => {
    const state = run([
      { type: "prompt", text: "x" },
      { type: "settled", snapshot: snapshot({ state: "ready", changedFiles: ["styles.css"], result: passing }, passing) },
      { type: "discard" },
    ]);
    expect(proposalFooter(state)).toEqual({ kind: "empty", canApply: false });
    expect(state.proposal).toBeNull();
    expect(state.messages).toHaveLength(1);
  });

  test("a blocked result without errors still reports a message", () => {
    const state = run([
      { type: "settled", snapshot: snapshot({ state: "blocked", changedFiles: ["app.js"], result: { ok: false, errors: [], checked: [] } }) },
    ]);
    expect(proposalFooter(state)).toMatchObject({ kind: "blocked", stage: "schema", message: "Validation failed." });
  });
});

describe("agent thread composer and providers", () => {
  test("submission needs text and an idle agent", () => {
    expect(canSubmit(initialAgentThreadState, "hola", false)).toBe(true);
    expect(canSubmit(initialAgentThreadState, "  ", false)).toBe(false);
    expect(canSubmit(initialAgentThreadState, "hola", true)).toBe(false);
    expect(canSubmit({ ...initialAgentThreadState, busy: true }, "hola", false)).toBe(false);
  });

  test("submission also needs a key and a model, before the draft is lost", () => {
    const anthropic = resolveProvider(PROVIDERS, "anthropic");
    expect(submitBlocker(anthropic, false, "claude-sonnet-4-6")).toContain("API key");
    expect(submitBlocker(anthropic, true, "   ")).toContain("model id");
    expect(submitBlocker(anthropic, true, "claude-sonnet-4-6")).toBe("");
    expect(submitBlocker({ ...anthropic, needsKey: false }, false, "claude-sonnet-4-6")).toBe("");

    const blocker = submitBlocker(anthropic, false, "claude-sonnet-4-6");
    expect(canSubmit(initialAgentThreadState, "hola", false, blocker)).toBe(false);
  });

  test("only tool and turn boundaries carry a workspace snapshot", () => {
    expect(carriesSnapshot({ type: "assistant-delta", text: "x" })).toBe(false);
    expect(carriesSnapshot({ type: "tool-start", id: "call_0", name: "edit", detail: "" })).toBe(false);
    expect(carriesSnapshot({ type: "tool-end", id: "call_0", name: "edit", ok: true, detail: "" })).toBe(true);
    expect(carriesSnapshot({ type: "turn-end" })).toBe(true);
    expect(carriesSnapshot({ type: "validation", result: { ok: true, errors: [], checked: [] } })).toBe(true);
    expect(carriesSnapshot({ type: "error", message: "boom" })).toBe(true);
  });

  test("a note lands as a closed assistant line without reopening the stream", () => {
    const state = run([
      { type: "prompt", text: "make it blue" },
      { type: "event", event: { type: "assistant-delta", text: "Done." } },
      { type: "note", text: "Applied styles.css." },
    ]);
    expect(state.messages.map((message) => message.text)).toEqual(["make it blue", "Done.", "Applied styles.css."]);
    expect(state.messages.every((message) => !message.streaming)).toBe(true);
  });

  test("every default provider offers models and a resolvable default", () => {
    for (const provider of PROVIDERS) {
      expect(provider.defaultModels.length).toBeGreaterThan(0);
      expect(isCustomModelId(provider, defaultModelIdFor(provider))).toBe(false);
    }
    expect(resolveProvider(PROVIDERS, "openrouter").label).toBe("OpenRouter");
  });

  test("an unknown model id counts as custom", () => {
    expect(isCustomModelId(resolveProvider(PROVIDERS, "anthropic"), "claude-from-the-future")).toBe(true);
  });

  test("key state follows custody, not the provider list", () => {
    const anthropic = resolveProvider(PROVIDERS, "anthropic");
    expect(keyState(anthropic, false)).toBe("missing");
    expect(keyState(anthropic, true)).toBe("stored");
    expect(keyState({ ...anthropic, needsKey: false }, false)).toBe("not-needed");
  });
});
