import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import type {
  AgentKeyStore,
  AgentProposal,
  AgentSessionConfig,
  LittleboxAgent,
  ProviderConfig,
} from "../agent/contracts";
import {
  agentThreadReducer,
  canSubmit,
  carriesSnapshot,
  initialAgentThreadState,
  proposalFooter,
  submitBlocker,
  type AgentThreadSnapshot,
  type AgentThreadState,
  type ProposalFooter,
} from "./agent-thread-state";

export type AgentThreadInput = {
  ensureAgent: () => Promise<LittleboxAgent | null>;
  agent: LittleboxAgent | null;
  sessionConfig: AgentSessionConfig;
  keyStore: AgentKeyStore;
  threadResetToken: number;
  keyVersion: number;
  /** Returns false when the proposal could not be landed, so nothing was written. */
  onApply: (proposal: AgentProposal) => boolean;
  onDiscard: () => void;
  /** Offline fallback used while no provider key is stored. */
  onDemoSend: (text: string) => Promise<{ message: string; changedFiles: string[] }>;
  providers?: readonly ProviderConfig[];
};

export type AgentThread = {
  state: AgentThreadState;
  draft: string;
  setDraft: (value: string) => void;
  provider: ProviderConfig;
  /** A provider key is stored, so prompts reach a real model. */
  live: boolean;
  blocker: string;
  footer: ProposalFooter;
  running: boolean;
  canSend: boolean;
  submit: () => Promise<void>;
  apply: () => void;
  discard: () => void;
  dismissError: () => void;
  interrupt: () => void;
};

/**
 * The conversation lives here rather than inside the panel: a panel dragged to
 * another group is a different place in the layout tree, and React remounts it.
 * A thread that a move could erase is not a thread anyone would trust.
 */
export function useAgentThread({
  ensureAgent,
  agent,
  sessionConfig,
  keyStore,
  threadResetToken,
  keyVersion,
  onApply,
  onDiscard,
  onDemoSend,
  providers = PROVIDERS,
}: AgentThreadInput): AgentThread {
  const [state, dispatch] = useReducer(agentThreadReducer, initialAgentThreadState);
  const [draft, setDraft] = useState("");
  // Subscription is keyed by instance so a send can attach before it runs.
  const subscription = useRef<{ agent: LittleboxAgent; off: () => void } | null>(null);

  const provider = resolveProvider(providers, sessionConfig.provider);
  const hasKey = useMemo(() => Boolean(keyStore.getKey(provider.id)), [keyStore, provider.id, keyVersion]);
  const live = keyState(provider, hasKey) !== "missing";
  const blocker = live ? submitBlocker(provider, hasKey, sessionConfig.modelId) : "";
  const footer = proposalFooter(state);
  const running = state.busy;

  function attach(next: LittleboxAgent) {
    if (subscription.current?.agent === next) return;
    subscription.current?.off();
    subscription.current = {
      agent: next,
      // Only tool and turn boundaries can move the proposal; a text delta cannot.
      off: next.subscribe((event) =>
        dispatch({ type: "event", event, snapshot: carriesSnapshot(event) ? readSnapshot(next) : undefined }),
      ),
    };
  }

  useEffect(() => {
    if (agent) attach(agent);
  }, [agent]);

  useEffect(() => () => subscription.current?.off(), []);

  useEffect(() => {
    dispatch({ type: "reset" });
    setDraft("");
  }, [threadResetToken]);

  async function submit() {
    const text = draft.trim();
    if (!text || running || blocker) return;
    setDraft("");
    dispatch({ type: "prompt", text });

    if (!live) {
      const reply = await onDemoSend(text);
      dispatch({ type: "event", event: { type: "assistant-delta", text: reply.message } });
      dispatch({ type: "settled" });
      return;
    }

    const handle = await ensureAgent();
    if (!handle) {
      dispatch({ type: "event", event: { type: "error", message: "The agent failed to load." } });
      return;
    }
    // Attaching before send is what guarantees the first delta is not lost.
    attach(handle);
    await handle.send(text);
    dispatch({ type: "settled", snapshot: readSnapshot(handle) });
  }

  function apply() {
    // The gate's verdict travels with the proposal, so the button cannot outrun it.
    if (!footer.canApply || !state.proposal?.validation?.ok) return;
    const files = state.proposal.changedFiles.join(", ");
    if (!onApply(state.proposal)) {
      // Refused, so nothing was written: say so instead of reporting an Apply.
      dispatch({ type: "discard" });
      dispatch({ type: "note", text: "The Capsule changed after this was validated, so nothing was applied. Ask again to redo it against the current files." });
      return;
    }
    dispatch({ type: "discard" });
    dispatch({ type: "note", text: `Applied ${files} to the project.` });
  }

  function discard() {
    dispatch({ type: "discard" });
    dispatch({ type: "note", text: "Discarded the pending changes." });
    onDiscard();
  }

  return {
    state,
    draft,
    setDraft,
    provider,
    live,
    blocker,
    footer,
    running,
    canSend: canSubmit(state, draft, false, blocker),
    submit,
    apply,
    discard,
    dismissError: () => dispatch({ type: "dismiss-error" }),
    interrupt: () => subscription.current?.agent.interrupt(),
  };
}

function readSnapshot(agent: LittleboxAgent): AgentThreadSnapshot {
  return { proposal: agent.proposal(), gate: agent.applyGateStatus() };
}
