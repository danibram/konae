import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import type {
  AgentKeyStore,
  AgentProposal,
  AgentSessionConfig,
  KonaeAgent,
  ProviderConfig,
} from "../agent/contracts";
import { isAgentSubmissionNotAdmitted } from "../agent/contracts";
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
  onApply: (proposal: AgentProposal) => boolean | ProposalApplyResult | Promise<boolean | ProposalApplyResult>;
  onDiscard: () => void | Promise<void>;
  providers?: readonly ProviderConfig[];
  draftKey?: string;
  draftStorage?: DraftStorage;
};

export type ProposalApplyResult = boolean | { applied: boolean; message?: string; preserveProposal?: boolean };

export type DraftStorage = { getItem: (key: string) => string | null; setItem: (key: string, value: string) => void; removeItem: (key: string) => void };

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
  live: boolean;
  blocker: string;
  footer: ProposalFooter;
  running: boolean;
  canSend: boolean;
  submit: () => Promise<void>;
  recover: () => Promise<void>;
  recoveryAttempted: boolean;
  apply: () => Promise<void>;
  discard: () => Promise<void>;
  dismissError: () => void;
  interrupt: () => void;
};

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
  const [recoveryAttempted, setRecoveryAttempted] = useState(false);
  const [draft, setDraftState] = useState(() => readDraft(draftStorage, draftKey));
  const submitting = useRef(false);
  const pendingUnsentDraft = useRef<string | null>(null);
  const draftKeyRef = useRef(draftKey);
  draftKeyRef.current = draftKey;
  const setDraft = (value: string) => {
    setDraftState(value);
    writeDraft(draftStorage, draftKeyRef.current, value);
  };
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
      off: next.subscribe((event) => {
        if (event.type === "error" && event.submission === "not-admitted" && pendingUnsentDraft.current !== null) {
          const unsent = pendingUnsentDraft.current;
          pendingUnsentDraft.current = null;
          setDraft(unsent);
          dispatch({ type: "unsubmit", text: unsent });
        }
        dispatch({ type: "event", event, snapshot: carriesSnapshot(event) ? readSnapshot(next) : undefined, keepBusy: submitting.current });
      }),
    };
    if (!submitting.current) dispatch({ type: "settled", snapshot: readSnapshot(next) });
  }

  useEffect(() => {
    if (agent) attach(agent);
  }, [agent]);

  useEffect(() => () => subscription.current?.off(), []);

  useEffect(() => {
    dispatch({ type: "reset" });
    setRecoveryAttempted(false);
    setDraftState(readDraft(draftStorage, draftKey));
  }, [threadResetToken, draftKey]);

  async function submit() {
    const submittedDraft = draft;
    const text = submittedDraft.trim();
    if (!text || running || submitting.current || blocker) return;
    submitting.current = true;
    let handle: KonaeAgent | null = null;
    try {
      handle = await ensureAgent();
      if (!handle) {
        dispatch({ type: "event", event: { type: "error", message: "The agent failed to load." }, keepBusy: true });
        return;
      }
      attach(handle);
      // Loading the shared agent includes the authoritative lease admission.
      // Keep the composer intact until that succeeds; a standby that loses
      // admission has not submitted this prompt and must not lose its draft.
      setDraft("");
      dispatch({ type: "prompt", text });
      pendingUnsentDraft.current = submittedDraft;
      await handle.send(text);
      pendingUnsentDraft.current = null;
    } catch (error) {
      if (!handle || isAgentSubmissionNotAdmitted(error)) {
        setDraft(submittedDraft);
        dispatch({ type: "unsubmit", text });
      }
      dispatch({ type: "event", event: { type: "error", message: error instanceof Error ? error.message : String(error) }, keepBusy: true });
    } finally {
      submitting.current = false;
      dispatch({ type: "settled", snapshot: handle ? readSnapshot(handle) : undefined });
    }
  }

  async function recover() {
    if (running || submitting.current || recoveryAttempted) return;
    setRecoveryAttempted(true);
    const handle = await ensureAgent();
    if (!handle) {
      setRecoveryAttempted(false);
      dispatch({ type: "event", event: { type: "error", message: "The saved proposal could not be recovered." } });
      return;
    }
    attach(handle);
    dispatch({ type: "settled", snapshot: readSnapshot(handle) });
  }

  async function apply() {
    if (submitting.current || running || !footer.canApply || !state.proposal?.validation?.ok) return;
    const files = state.proposal.changedFiles.join(", ");
    let result: Awaited<ReturnType<typeof onApply>>;
    try { result = await onApply(state.proposal); }
    catch (error) {
      dispatch({ type: "note", text: error instanceof Error ? error.message : "Apply could not be confirmed. The proposal remains available." });
      return;
    }
    const applied = typeof result === "boolean" ? result : result.applied;
    if (!applied) {
      if (typeof result !== "boolean" && result.preserveProposal) {
        if (result.message) dispatch({ type: "note", text: result.message });
        return;
      }
      dispatch({ type: "discard" });
      dispatch({ type: "note", text: "The Capsule changed after this was validated, so nothing was applied. Ask again to redo it against the current files." });
      return;
    }
    dispatch({ type: "discard" });
    dispatch({ type: "note", text: `Applied ${files} to the project.` });
  }

  async function discard() {
    if (submitting.current || running) return;
    try { await onDiscard(); }
    catch (error) {
      dispatch({ type: "event", event: { type: "error", message: error instanceof Error ? error.message : "Discard could not be saved." } });
      return;
    }
    dispatch({ type: "discard" });
    dispatch({ type: "note", text: "Discarded the pending changes." });
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
    recover,
    recoveryAttempted,
    apply,
    discard,
    dismissError: () => dispatch({ type: "dismiss-error" }),
    interrupt: () => subscription.current?.agent.interrupt(),
  };
}

function readSnapshot(agent: KonaeAgent): AgentThreadSnapshot {
  return { proposal: agent.proposal(), gate: agent.applyGateStatus() };
}
