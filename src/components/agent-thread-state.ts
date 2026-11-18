import { keyState } from "../agent/catalog";
import type {
  AgentProposal,
  AgentUiEvent,
  ApplyGateStatus,
  ProviderConfig,
  ValidationResult,
  ValidationStage,
  ValidationWarning,
} from "../agent/contracts";
import type { ProjectFile } from "../lib/types";

export type ThreadMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  streaming: boolean;
};

export type ToolChipState = "running" | "ok" | "fail";

export type ToolChip = {
  id: string;
  name: string;
  detail: string;
  state: ToolChipState;
};

export type AgentThreadState = {
  messages: ThreadMessage[];
  tools: ToolChip[];
  busy: boolean;
  error: string;
  proposal: AgentProposal | null;
  gate: ApplyGateStatus;
  validation: ValidationResult | null;
  seq: number;
};

export type AgentThreadSnapshot = {
  proposal: AgentProposal | null;
  gate: ApplyGateStatus;
};

export type AgentThreadAction =
  | { type: "prompt"; text: string }
  | { type: "event"; event: AgentUiEvent; snapshot?: AgentThreadSnapshot; keepBusy?: boolean }
  | { type: "settled"; snapshot?: AgentThreadSnapshot }
  | { type: "note"; text: string }
  | { type: "dismiss-error" }
  | { type: "discard" }
  | { type: "reset" };

export const initialAgentThreadState: AgentThreadState = {
  messages: [],
  tools: [],
  busy: false,
  error: "",
  proposal: null,
  gate: { state: "empty" },
  validation: null,
  seq: 0,
};

export function agentThreadReducer(state: AgentThreadState, action: AgentThreadAction): AgentThreadState {
  switch (action.type) {
    case "prompt": {
      const text = action.text.trim();
      if (!text) return state;
      return {
        ...state,
        messages: [...closeStream(state.messages), { id: `m${state.seq}`, role: "user", text, streaming: false }],
        tools: [],
        busy: true,
        error: "",
        validation: null,
        seq: state.seq + 1,
      };
    }
    case "event":
      return applyEvent(withSnapshot(state, action.snapshot), action.event, action.keepBusy);
    case "settled":
      return { ...settleRun(withSnapshot(state, action.snapshot)), busy: false };
    case "note": {
      const text = action.text.trim();
      if (!text) return state;
      return {
        ...state,
        messages: [...closeStream(state.messages), { id: `m${state.seq}`, role: "assistant", text, streaming: false }],
        seq: state.seq + 1,
      };
    }
    case "dismiss-error":
      return state.error ? { ...state, error: "" } : state;
    case "discard":
      return { ...state, proposal: null, gate: { state: "empty" }, validation: null };
    case "reset":
      return initialAgentThreadState;
  }
}

function applyEvent(state: AgentThreadState, event: AgentUiEvent, keepBusy = false): AgentThreadState {
  switch (event.type) {
    case "assistant-delta":
      return appendDelta(state, event.text);
    case "tool-start":
      return {
        ...state,
        tools: [...state.tools, { id: event.id, name: event.name, detail: event.detail, state: "running" }],
      };
    case "tool-end":
      return { ...state, tools: endTool(state.tools, event.id, event.ok, event.detail) };
    case "turn-end":
      return { ...state, messages: closeStream(state.messages) };
    case "error":
      return { ...settleRun(state), busy: keepBusy ? state.busy : false, error: event.message };
    case "validation":
      return { ...state, validation: event.result };
  }
}

function appendDelta(state: AgentThreadState, text: string): AgentThreadState {
  if (!text) return state;
  const last = state.messages[state.messages.length - 1];
  if (last?.role === "assistant" && last.streaming) {
    return {
      ...state,
      messages: [...state.messages.slice(0, -1), { ...last, text: last.text + text }],
    };
  }
  return {
    ...state,
    messages: [...state.messages, { id: `m${state.seq}`, role: "assistant", text, streaming: true }],
    seq: state.seq + 1,
  };
}

function endTool(tools: ToolChip[], id: string, ok: boolean, detail: string): ToolChip[] {
  const index = tools.findIndex((chip) => chip.id === id);
  if (index < 0) return tools;
  const chip = tools[index];
  const next = [...tools];
  next[index] = { ...chip, state: ok ? "ok" : "fail", detail: detail || chip.detail };
  return next;
}

function closeStream(messages: ThreadMessage[]): ThreadMessage[] {
  return messages.some((message) => message.streaming)
    ? messages.map((message) => (message.streaming ? { ...message, streaming: false } : message))
    : messages;
}

function settleRun(state: AgentThreadState): AgentThreadState {
  return {
    ...state,
    messages: closeStream(state.messages),
    tools: state.tools.map((chip) => (chip.state === "running" ? { ...chip, state: "fail" } : chip)),
  };
}

function withSnapshot(state: AgentThreadState, snapshot?: AgentThreadSnapshot): AgentThreadState {
  if (!snapshot) return state;
  return { ...state, proposal: snapshot.proposal, gate: snapshot.gate };
}


export type ProposalFooter =
  | { kind: "empty"; canApply: false }
  | { kind: "unvalidated"; changedFiles: ProjectFile[]; canApply: false }
  /** `detail` carries the reason, such as the Kernel's sentence for an action it would refuse. */
  | { kind: "blocked"; changedFiles: ProjectFile[]; canApply: false; stage: ValidationStage; message: string; detail?: string; code?: string; hint?: ValidationResult["errors"][number]["hint"]; remainingErrors: number }
  /**
   * `checked` is what the pass covers; the panel may not claim more than this.
   * `owner` is a release refusal only the owner clears: Apply stays open, Deploy refuses.
   */
  | { kind: "validated"; changedFiles: ProjectFile[]; canApply: true; checked: ValidationStage[]; owner?: OwnerRefusal; warnings: ValidationWarning[] };

export type OwnerRefusal = { message: string; detail?: string };

const stageOrder: ValidationStage[] = ["sources", "schema", "preview-rows", "actions"];

export function validationSummary(checked: ValidationStage[]): string {
  const ran = stageOrder.filter((stage) => checked.includes(stage)).map((stage) => stage.replace("-", " "));
  if (ran.length === 0) return "Validated.";
  const held = ran.length === 1 ? ran[0] : `${ran.slice(0, -1).join(", ")} and ${ran[ran.length - 1]}`;
  const skipped = checked.includes("preview-rows") ? "" : " The preview rows were not checked.";
  return `Validated. ${held[0].toUpperCase()}${held.slice(1)} hold.${skipped}`;
}

export function proposalFooter(state: AgentThreadState): ProposalFooter {
  const gate = state.gate;
  if (gate.state === "empty") return { kind: "empty", canApply: false };
  if (gate.state === "ready") {
    const owner = gate.result.warnings?.find((warning) => warning.owner);
    return {
      kind: "validated",
      changedFiles: gate.changedFiles,
      canApply: true,
      checked: gate.result.checked,
      warnings: (gate.result.warnings ?? []).filter((warning) => !warning.owner),
      ...(owner ? { owner: { message: owner.message, ...(owner.detail ? { detail: owner.detail } : {}) } } : {}),
    };
  }
  if (gate.state === "blocked") return blocked(gate.changedFiles, gate.result);
  return state.validation && !state.validation.ok
    ? blocked(gate.changedFiles, state.validation)
    : { kind: "unvalidated", changedFiles: gate.changedFiles, canApply: false };
}

function blocked(changedFiles: ProjectFile[], result: ValidationResult): ProposalFooter {
  const first = firstValidationError(result);
  return {
    kind: "blocked",
    changedFiles,
    canApply: false,
    stage: first?.stage ?? "schema",
    message: first ? first.message : "Validation failed.",
    ...(first?.detail ? { detail: first.detail } : {}),
    ...(first?.code ? { code: first.code } : {}),
    ...(first?.hint ? { hint: first.hint } : {}),
    remainingErrors: Math.max(0, result.errors.length - 1),
  };
}

export function firstValidationError(result: ValidationResult | null) {
  return result?.errors[0] ?? null;
}

export function canSubmit(state: AgentThreadState, draft: string, busy: boolean, blocker = ""): boolean {
  return !busy && !state.busy && blocker.length === 0 && draft.trim().length > 0;
}

export function submitBlocker(provider: ProviderConfig, hasKey: boolean, modelId: string): string {
  if (keyState(hasKey) === "missing") return `Add your ${provider.label} API key first.`;
  if (modelId.trim().length === 0) return "Choose a model id first.";
  return "";
}

export function carriesSnapshot(event: AgentUiEvent): boolean {
  return event.type === "tool-end" || event.type === "turn-end" || event.type === "validation" || event.type === "error";
}

export function noteAction(text: string): AgentThreadAction {
  return { type: "note", text };
}
