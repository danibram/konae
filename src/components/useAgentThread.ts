import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import type {
  AgentKeyStore,
  AgentProposal,
  AgentSessionConfig,
  KonaeAgent,
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
  ensureAgent: () => Promise<KonaeAgent | null>;
  agent: KonaeAgent | null;
  sessionConfig: AgentSessionConfig;
  keyStore: AgentKeyStore;
  threadResetToken: number;
  keyVersion: number;
  /** Returns false when the proposal could not be landed, so nothing was written. */
  onApply: (proposal: AgentProposal) => boolean;
  onDiscard: () => void;
  providers?: readonly ProviderConfig[];
  /**
   * The project the conversation belongs to. With `draftStorage`, an unsent
   * draft is kept per project in this browser until it is sent, so words typed
   * in New project or in Describe survive leaving the app and a reload.
   */
  draftKey?: string;
  draftStorage?: DraftStorage;
};

export type DraftStorage = { getItem: (key: string) => string | null; setItem: (key: string, value: string) => void; removeItem: (key: string) => void };

/** Where a project's unsent Describe draft waits in this browser. */
export const describeDraftKey = (projectId: string) => `describe-draft:${projectId}`;

function readDraft(storage: DraftStorage | undefined, projectId: string | undefined): string {
  if (!storage || !projectId) return "";
  try { return storage.getItem(describeDraftKey(projectId)) ?? ""; } catch { return ""; }
}

function writeDraft(storage: DraftStorage | undefined, projectId: string | undefined, value: string) {
  if (!storage || !projectId) return;
  try {
    if (value.trim()) storage.setItem(describeDraftKey(projectId), value);
    else storage.removeItem(describeDraftKey(projectId));
  } catch { /* A full or blocked storage keeps the draft in memory only. */ }
}

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
  providers = PROVIDERS,
  draftKey,
  draftStorage,
}: AgentThreadInput): AgentThread {
  const [state, dispatch] = useReducer(agentThreadReducer, initialAgentThreadState);
  const [draft, setDraftState] = useState(() => readDraft(draftStorage, draftKey));
  // Written on each change rather than from an effect, so a project switch never stores one project's draft under another.
  const draftKeyRef = useRef(draftKey);
  draftKeyRef.current = draftKey;
  const setDraft = (value: string) => {
    setDraftState(value);
    writeDraft(draftStorage, draftKeyRef.current, value);
  };
  // Subscription is keyed by instance so a send can attach before it runs.
  const subscription = useRef<{ agent: KonaeAgent; off: () => void } | null>(null);

  const provider = resolveProvider(providers, sessionConfig.provider);
  const hasKey = useMemo(() => Boolean(keyStore.getKey(provider.id)), [keyStore, provider.id, keyVersion]);
  const live = keyState(hasKey) !== "missing";
  const blocker = live ? submitBlocker(provider, hasKey, sessionConfig.modelId) : "";
  const footer = proposalFooter(state);
  const running = state.busy;

  function attach(next: KonaeAgent) {
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

  // A new thread, or another project, opens on that project's unsent draft.
  useEffect(() => {
    dispatch({ type: "reset" });
    setDraftState(readDraft(draftStorage, draftKey));
  }, [threadResetToken, draftKey]);

  async function submit() {
    const text = draft.trim();
    if (!text || running || blocker) return;
    setDraft("");
    dispatch({ type: "prompt", text });

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

function readSnapshot(agent: KonaeAgent): AgentThreadSnapshot {
  return { proposal: agent.proposal(), gate: agent.applyGateStatus() };
}
