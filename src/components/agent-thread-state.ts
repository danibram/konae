import { keyState } from "../agent/catalog";
import type {
  AgentProposal,
  AgentUiEvent,
  ApplyGateStatus,
  ProviderConfig,
  ValidationResult,
  ValidationStage,
} from "../agent/contracts";
import type { ProjectFile } from "../lib/types";

/**
 * View model for the agent thread. The component paints this and nothing else;
 * every transition lives in `agentThreadReducer` so it can be tested without a DOM.
 */
export type ThreadMessage = {
  id: string;
  role: "user" | "assistant";
  text: string;
  /** Assistant messages stay open so deltas keep appending until the turn ends. */
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
  /** Last snapshot read from the agent; the payload handed to onApply. */
  proposal: AgentProposal | null;
  /** The Validation Gate's own verdict — the only source allowed to enable Apply. */
  gate: ApplyGateStatus;
  /** Last `validation` event, used only to block earlier than the gate would. */
  validation: ValidationResult | null;
  seq: number;
};

export type AgentThreadSnapshot = {
  proposal: AgentProposal | null;
  gate: ApplyGateStatus;
};

export type AgentThreadAction =
  | { type: "prompt"; text: string }
  | { type: "event"; event: AgentUiEvent; snapshot?: AgentThreadSnapshot }
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
      return applyEvent(withSnapshot(state, action.snapshot), action.event);
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
      // Drops the footer only; the thread is the record of what was asked.
      return { ...state, proposal: null, gate: { state: "empty" }, validation: null };
    case "reset":
      return initialAgentThreadState;
  }
}

function applyEvent(state: AgentThreadState, event: AgentUiEvent): AgentThreadState {
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
      return { ...settleRun(state), busy: false, error: event.message };
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

/** Matched by tool call id: two concurrent calls of one tool must not be confused. */
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

/** A run that ends without a tool-end left that tool unfinished — never spinning. */
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

// --- Derived view model -------------------------------------------------------

export type ProposalFooter =
  | { kind: "empty"; canApply: false }
  | { kind: "unvalidated"; changedFiles: ProjectFile[]; canApply: false }
  /** `detail` carries the reason, such as the Kernel's sentence for an action it would refuse. */
  | { kind: "blocked"; changedFiles: ProjectFile[]; canApply: false; stage: ValidationStage; message: string; detail?: string }
  /**
   * `checked` is what the pass covers; the panel may not claim more than this.
   * `owner` is a release refusal only the owner clears: Apply stays open, Deploy refuses.
   */
  | { kind: "validated"; changedFiles: ProjectFile[]; canApply: true; checked: ValidationStage[]; owner?: OwnerRefusal };

/** The Kernel's sentence for a refusal the owner clears in Auth > Integrations, and who acts. */
export type OwnerRefusal = { message: string; detail?: string };

const stageOrder: ValidationStage[] = ["schema", "migration", "actions"];

/**
 * What a passing gate is entitled to say. Naming the stages that ran is the
 * difference between "migration holds" and "nobody looked": the migration stage
 * is skipped whenever the live database could not be read.
 */
export function validationSummary(checked: ValidationStage[]): string {
  const ran = stageOrder.filter((stage) => checked.includes(stage));
  if (ran.length === 0) return "Validated.";
  const held = ran.length === 1 ? ran[0] : `${ran.slice(0, -1).join(", ")} and ${ran[ran.length - 1]}`;
  const skipped = checked.includes("migration") ? "" : " — migration not checked";
  return `Validated — ${held} hold${skipped}.`;
}

/**
 * Apply is enabled only when the Validation Gate itself reports "ready". A
 * `validation` event may downgrade an unvalidated proposal to blocked, never
 * upgrade one to applicable.
 */
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
  };
}

export function firstValidationError(result: ValidationResult | null) {
  return result?.errors[0] ?? null;
}

export function canSubmit(state: AgentThreadState, draft: string, busy: boolean, blocker = ""): boolean {
  return !busy && !state.busy && blocker.length === 0 && draft.trim().length > 0;
}

/**
 * Why the composer cannot send yet, in the owner's words. Without this the model
 * layer's own failure ("Provider is not configured", "No model id for provider")
 * only arrives after the draft was cleared and shown as if it had been sent.
 */
export function submitBlocker(provider: ProviderConfig, hasKey: boolean, modelId: string): string {
  if (keyState(provider, hasKey) === "missing") return `Add your ${provider.label} API key first.`;
  if (modelId.trim().length === 0) return "Choose a model id first.";
  return "";
}

/**
 * A proposal can only change when a tool finished or the run settled, so text
 * deltas must not trigger a full workspace snapshot — that is a copy of all five
 * files per streamed token.
 */
export function carriesSnapshot(event: AgentUiEvent): boolean {
  return event.type === "tool-end" || event.type === "turn-end" || event.type === "validation" || event.type === "error";
}

/**
 * The panel's own line after an Apply or a Descartar — UI feedback, not model
 * output, so it never streams and never re-opens a closed turn.
 */
export function noteAction(text: string): AgentThreadAction {
  return { type: "note", text };
}
