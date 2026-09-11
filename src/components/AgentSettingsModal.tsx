import { Check, KeyRound } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { defaultModelIdFor, isCustomModelId, keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import type { AgentKeyStore, AgentSessionConfig, ProviderConfig, ProviderId } from "../agent/contracts";
import { Button, Dialog, Input, Select } from "./ui";
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
    <Dialog open={open} title="Connect a model" description="The key stays in this browser." onClose={onClose} initialFocus={firstFieldRef} className="agent-settings">
        <div className="agent-settings-body">
          <label className="agent-field">
            <span>Provider</span>
            <Select
              ref={firstFieldRef}
              value={provider.id}
              onChange={(event) => changeProvider(event.target.value as ProviderId)}
            >
              {providers.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
            </Select>
          </label>

          <label className="agent-field">
            <span>Model</span>
            <Select
              value={customModel ? customOption : sessionConfig.modelId}
              onChange={(event) => onSessionConfigChange({
                ...sessionConfig,
                modelId: event.target.value === customOption ? "" : event.target.value,
              })}
            >
              {provider.defaultModels.map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}
              {provider.supportsCustomModelId && <option value={customOption}>Custom model id…</option>}
            </Select>
          </label>

          {customModel && provider.supportsCustomModelId && (
            <label className="agent-field">
              <span>Model ID</span>
              <Input
                value={sessionConfig.modelId}
                placeholder={defaultModelIdFor(provider)}
                spellCheck={false}
                onChange={(event) => onSessionConfigChange({ ...sessionConfig, modelId: event.target.value })}
              />
            </label>
          )}

          <div className={`agent-settings-key ${key}`}>
            <KeyRound size={13} />
            {key === "not-needed" && <span>The Littlebox Kernel holds the key.</span>}
            {key === "stored" && !editingKey && (
              <>
                <span><Check size={12} /> Key stored for {provider.label}</span>
                <Button onClick={() => setEditingKey(true)}>Change</Button>
                <Button onClick={() => { keyStore.clearKey(provider.id); setKeyVersion((version) => version + 1); onKeyStoreChange?.(); }}>Forget</Button>
              </>
            )}
            {(key === "missing" || editingKey) && (
              <>
                <label className="sr-only" htmlFor="agent-key">{provider.label} API key</label>
                <Input
                  id="agent-key"
                  type="password"
                  value={keyDraft}
                  placeholder={`${provider.label} API key`}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(event) => setKeyDraft(event.target.value)}
                  onKeyDown={(event) => { if (event.key === "Enter") saveKey(); }}
                />
                <Button variant="primary" disabled={!keyDraft.trim()} onClick={saveKey}>Save</Button>
                {editingKey && <Button onClick={() => { setEditingKey(false); setKeyDraft(""); }}>Cancel</Button>}
              </>
            )}
          </div>

          <p className="agent-settings-note">
            It is sent only to {provider.label}.
          </p>
        </div>
    </Dialog>
  );
}
