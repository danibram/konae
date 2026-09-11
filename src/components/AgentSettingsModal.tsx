import { Check, KeyRound, Settings2, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { defaultModelIdFor, isCustomModelId, keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import type { AgentKeyStore, AgentSessionConfig, ProviderConfig, ProviderId } from "../agent/contracts";
import { useModalDialog } from "./useModalDialog";
import "./agent-settings.css";

type Props = {
  open: boolean;
  onClose: () => void;
  sessionConfig: AgentSessionConfig;
  onSessionConfigChange: (config: AgentSessionConfig) => void;
  keyStore: AgentKeyStore;
  /** Stored keys are invisible to React, so the panel needs telling they moved. */
  onKeyStoreChange?: () => void;
  /** Defaults to the curated catalogue; the integrator may pass its own. */
  providers?: readonly ProviderConfig[];
};

const customOption = "__custom__";

/**
 * Agent configuration only: which provider answers, which model, and the key
 * that pays for it. The conversation itself lives in the editor's agent panel —
 * a modal is the wrong place to watch work happen.
 */
export function AgentSettingsModal({
  open,
  onClose,
  sessionConfig,
  onSessionConfigChange,
  keyStore,
  onKeyStoreChange,
  providers = PROVIDERS,
}: Props) {
  const [keyDraft, setKeyDraft] = useState("");
  const [keyVersion, setKeyVersion] = useState(0);
  const [editingKey, setEditingKey] = useState(false);
  const firstFieldRef = useRef<HTMLSelectElement>(null);
  const dialog = useModalDialog({ open, onClose, initialFocus: firstFieldRef });

  const provider = resolveProvider(providers, sessionConfig.provider);
  const hasKey = useMemo(
    () => Boolean(keyStore.getKey(provider.id)),
    [keyStore, provider.id, keyVersion],
  );
  const key = keyState(provider, hasKey);
  const customModel = isCustomModelId(provider, sessionConfig.modelId);

  useEffect(() => {
    // An unsaved key must not linger in component state after the dialog closes.
    if (open) return;
    setKeyDraft("");
    setEditingKey(false);
  }, [open]);

  function changeProvider(id: ProviderId) {
    const next = resolveProvider(providers, id);
    setEditingKey(false);
    setKeyDraft("");
    onSessionConfigChange({ ...sessionConfig, provider: next.id, modelId: defaultModelIdFor(next) });
  }

  function saveKey() {
    const value = keyDraft.trim();
    if (!value) return;
    keyStore.setKey(provider.id, value);
    setKeyDraft("");
    setEditingKey(false);
    setKeyVersion((version) => version + 1);
    onKeyStoreChange?.();
  }

  if (!open) return null;

  return (
    <div className="modal-backdrop agent-settings-backdrop" role="presentation" onClick={onClose}>
      <section
        className="agent-settings"
        role="dialog"
        aria-modal="true"
        aria-label="Agent settings"
        ref={dialog.ref}
        onKeyDown={dialog.onKeyDown}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="agent-settings-head">
          <span className="agent-settings-mark"><Settings2 size={14} /></span>
          <div>
            <p className="eyebrow">BUILDER AGENT</p>
            <strong>Model &amp; provider</strong>
          </div>
          <button className="agent-settings-close" aria-label="Close settings" onClick={onClose}><X size={15} /></button>
        </header>

        <div className="agent-settings-body">
          <label className="agent-field">
            <span>PROVIDER</span>
            <select
              ref={firstFieldRef}
              value={provider.id}
              onChange={(event) => changeProvider(event.target.value as ProviderId)}
            >
              {providers.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
            </select>
          </label>

          <label className="agent-field">
            <span>MODEL</span>
            <select
              value={customModel ? customOption : sessionConfig.modelId}
              onChange={(event) => onSessionConfigChange({
                ...sessionConfig,
                modelId: event.target.value === customOption ? "" : event.target.value,
              })}
            >
              {provider.defaultModels.map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}
              {provider.supportsCustomModelId && <option value={customOption}>Custom model id…</option>}
            </select>
          </label>

          {customModel && provider.supportsCustomModelId && (
            <label className="agent-field">
              <span>MODEL ID</span>
              <input
                value={sessionConfig.modelId}
                placeholder={defaultModelIdFor(provider)}
                spellCheck={false}
                onChange={(event) => onSessionConfigChange({ ...sessionConfig, modelId: event.target.value })}
              />
            </label>
          )}

          <div className={`agent-settings-key ${key}`}>
            <KeyRound size={13} />
            {key === "not-needed" && <span>Key held by the Littlebox Kernel — nothing to store here.</span>}
            {key === "stored" && !editingKey && (
              <>
                <span><Check size={12} /> Key stored for {provider.label}</span>
                <button onClick={() => setEditingKey(true)}>Change</button>
                <button onClick={() => { keyStore.clearKey(provider.id); setKeyVersion((version) => version + 1); onKeyStoreChange?.(); }}>Forget</button>
              </>
            )}
            {(key === "missing" || editingKey) && (
              <>
                <label className="sr-only" htmlFor="agent-key">{provider.label} API key</label>
                <input
                  id="agent-key"
                  type="password"
                  value={keyDraft}
                  placeholder={`${provider.label} API key — stays in this browser`}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => setKeyDraft(event.target.value)}
                  onKeyDown={(event) => { if (event.key === "Enter") saveKey(); }}
                />
                <button className="primary" disabled={!keyDraft.trim()} onClick={saveKey}>Save</button>
                {editingKey && <button onClick={() => { setEditingKey(false); setKeyDraft(""); }}>Cancel</button>}
              </>
            )}
          </div>

          <p className="agent-settings-note">
            The key is kept in this browser's local storage and sent only to {provider.label}. Prompts carry the
            contents of the five Capsule files so the agent can edit them.
          </p>
        </div>
      </section>
    </div>
  );
}
