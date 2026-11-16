import { Check, ExternalLink } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { defaultModelIdFor, isCustomModelId, keyState, PROVIDERS, resolveProvider } from "../agent/catalog";
import type { AgentKeyStore, AgentSessionConfig, ProviderConfig, ProviderId } from "../agent/contracts";
import { Button, Dialog, Field, Input, Select } from "./ui";
import "./agent-settings.css";

type Props = {
  open: boolean;
  onClose: () => void;
  sessionConfig: AgentSessionConfig;
  onSessionConfigChange: (config: AgentSessionConfig) => void;
  keyStore: AgentKeyStore;
  onKeyStoreChange?: () => void;
  providers?: readonly ProviderConfig[];
};

const customOption = "__custom__";

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
  const [failure, setFailure] = useState("");
  const firstFieldRef = useRef<HTMLSelectElement>(null);
  const keyFieldRef = useRef<HTMLInputElement>(null);

  const provider = resolveProvider(providers, sessionConfig.provider);
  const hasKey = useMemo(
    () => Boolean(keyStore.getKey(provider.id)),
    [keyStore, provider.id, keyVersion],
  );
  const key = keyState(hasKey);
  const customModel = isCustomModelId(provider, sessionConfig.modelId);
  const askingForKey = key === "missing" || editingKey;

  useEffect(() => {
    if (open) return;
    setKeyDraft("");
    setEditingKey(false);
    setFailure("");
  }, [open]);

  function changeProvider(id: ProviderId) {
    const next = resolveProvider(providers, id);
    setEditingKey(false);
    setKeyDraft("");
    setFailure("");
    onSessionConfigChange({ ...sessionConfig, provider: next.id, modelId: defaultModelIdFor(next) });
  }

  function saveKey() {
    if (!askingForKey) return;
    const value = keyDraft.trim();
    if (!value) {
      setFailure(`Paste the key from ${provider.label} first.`);
      keyFieldRef.current?.focus();
      return;
    }
    try {
      keyStore.setKey(provider.id, value);
    } catch (cause) {
      setFailure(`This browser did not keep the key. ${cause instanceof Error ? cause.message : String(cause)}`);
      return;
    }
    setKeyDraft("");
    setEditingKey(false);
    setFailure("");
    setKeyVersion((version) => version + 1);
    onKeyStoreChange?.();
    onClose();
  }

  if (!open) return null;

  return (
    <Dialog
      open={open}
      title="Connect a model"
      description="A model turns what you write in Describe into changes to your app. Your app already runs and deploys without one."
      onClose={onClose}
      initialFocus={firstFieldRef}
      className="agent-settings"
      footer={askingForKey
        ? <>
          <Button onClick={() => { if (editingKey) { setEditingKey(false); setKeyDraft(""); setFailure(""); } else onClose(); }}>Cancel</Button>
          <Button variant="primary" type="submit" form="agent-settings-form">Save</Button>
        </>
        : <Button onClick={onClose}>Done</Button>}
    >
      <form id="agent-settings-form" className="ui-dialog-body agent-settings-body" onSubmit={(event) => { event.preventDefault(); saveKey(); }}>
        <Field label="Provider">
          <Select
            ref={firstFieldRef}
            value={provider.id}
            onChange={(event) => changeProvider(event.target.value as ProviderId)}
          >
            {providers.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
          </Select>
        </Field>

        <Field label="Model">
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
        </Field>

        {customModel && provider.supportsCustomModelId && (
          <Field label="Model ID" help={`As ${provider.label} writes it, for example ${defaultModelIdFor(provider)}.`}>
            <Input
              value={sessionConfig.modelId}
              spellCheck={false}
              onChange={(event) => onSessionConfigChange({ ...sessionConfig, modelId: event.target.value })}
            />
          </Field>
        )}

        {key === "stored" && !editingKey && (
          <div className="agent-settings-stored">
            <span><Check size={14} aria-hidden="true" /> Your {provider.label} key is saved in this browser.</span>
            <Button size="sm" onClick={() => { setFailure(""); setEditingKey(true); }}>Change</Button>
            <Button size="sm" onClick={() => { keyStore.clearKey(provider.id); setKeyVersion((version) => version + 1); onKeyStoreChange?.(); }}>Forget</Button>
          </div>
        )}

        {askingForKey && (
          <div className={failure ? "agent-settings-key is-failed" : "agent-settings-key"}>
            <Field label={`${provider.label} API key`} help={failure || undefined}>
              <Input
                ref={keyFieldRef}
                type="password"
                value={keyDraft}
                autoComplete="off"
                spellCheck={false}
                aria-invalid={failure ? true : undefined}
                onChange={(event) => { setKeyDraft(event.target.value); if (failure) setFailure(""); }}
              />
            </Field>
            <p className="agent-settings-help">
              An API key is a password that lets Konae use {provider.label}'s models for you.{" "}
              {provider.keyPage
                ? <>Create one on <a href={provider.keyPage} target="_blank" rel="noopener noreferrer">{provider.label}'s API keys page<ExternalLink size={12} aria-hidden="true" /><span className="sr-only"> (opens in a new tab)</span></a>.</>
                : <>Create one in your {provider.label} account.</>}{" "}
              {provider.label} charges the account that owns this key for model use.
            </p>
          </div>
        )}

        <p className="agent-settings-note">
          Konae stores this key in this browser's local storage. Requests send it only to {provider.label}.
          {askingForKey && " After Save, press Send in Describe, and Konae builds the app from what you wrote."}
        </p>
      </form>
    </Dialog>
  );
}
