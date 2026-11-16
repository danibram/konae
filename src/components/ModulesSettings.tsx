import { useEffect, useMemo, useRef, useState } from "react";
import { sdkContract, type SdkModuleOption } from "../sdk/contract";
import { isModuleChanged, readModules, setModule, type ModuleRead } from "../deployments/client";
import { isSignedOutError } from "./owner-session";
import { Badge, Button, Field, Input, LoadingBlock, Select, Switch } from "./ui";

type StateMap = Record<string, { on: boolean; [key: string]: unknown }>;

export function ModulesSettings({ projectId, initial, onSignedOut }: { projectId: string | null; initial: StateMap; onSignedOut: () => void }) {
  const [view, setView] = useState<ModuleRead | null>(null);
  const [reading, setReading] = useState(Boolean(projectId));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [drafts, setDrafts] = useState<Record<string, Record<string, unknown>>>({});
  const errorRef = useRef<HTMLParagraphElement>(null);

  useEffect(() => {
    if (error) errorRef.current?.focus();
  }, [error]);

  const load = async () => {
    if (!projectId) { setReading(false); return; }
    setReading(true);
    setError("");
    try {
      const next = await readModules(projectId);
      setView(next);
      setDrafts({});
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    } finally {
      setReading(false);
    }
  };

  useEffect(() => { void load(); }, [projectId]);

  const entries = useMemo(() => sdkContract.modules.map((contract) => ({
    contract,
    live: view?.modules[contract.name],
    initialState: initial[contract.name] ?? { on: contract.switch ? false : true, ...Object.fromEntries(contract.options.map((option) => [option.key, option.default])) },
  })), [initial, view]);

  const save = async (name: string, on: boolean | undefined, options: Record<string, unknown>) => {
    if (!view || !projectId) return;
    const contract = sdkContract.modules.find((item) => item.name === name)!;
    const current = view.modules[name];
    if (!current || busy) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      setView(await setModule(projectId, name, current.state, on, options));
      setDrafts((value) => ({ ...value, [name]: {} }));
      setNotice(`${contract.name} settings saved.`);
    } catch (cause) {
      if (isModuleChanged(cause)) {
        try { setView(await readModules(projectId)); } catch { }
      }
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    } finally {
      setBusy(false);
    }
  };

  const change = (name: string, key: string, value: unknown) => {
    setDrafts((current) => ({ ...current, [name]: { ...current[name], [key]: value } }));
  };

  return <section className="app-settings-section module-settings" aria-label="Modules" aria-busy={busy || reading}>
    {reading && <LoadingBlock label="Reading module settings" rows={3} />}
    {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
    {!reading && error && view === null && <Button onClick={() => void load()}>Retry loading</Button>}
    {!reading && entries.map(({ contract, live, initialState }) => {
      const state = live?.state ?? initialState;
      const draft = drafts[contract.name] ?? {};
      const values = { ...state, ...draft };
      const changed = Object.keys(draft).length > 0;
      const switchable = live?.switch ?? contract.switch;
      const current = live?.current;
      return <article className="module-settings-card" key={contract.name}>
        <header><div><h3>{contract.name}</h3><p>{live?.summary ?? contract.summary}</p></div><Badge tone={switchable && state.on ? "success" : "neutral"}>{switchable ? state.on ? "On" : "Off" : "On · fixed"}</Badge></header>
        {!switchable && <p>{live?.message ?? "This module is on and cannot be changed yet."}</p>}
        {(contract.options as readonly SdkModuleOption[]).map((option) => {
          const value = values[option.key] ?? null;
          if (option.type === "boolean") return <Switch key={option.key} checked={value === true} onChange={(next) => change(contract.name, option.key, next)} label={option.label} disabled={busy || reading || !switchable || !projectId} />;
          if (option.type === "enum") return <Field key={option.key} label={option.label} help={live?.options.find((item) => item.key === option.key)?.help ?? option.help}>
            <Select value={String(value ?? "")} onChange={(event) => change(contract.name, option.key, event.target.value || null)} disabled={busy || reading || !switchable || !projectId}>
              {value === null && <option value="">Use the default address</option>}
              {(option.values ?? []).map((item) => <option key={item} value={item}>{item}</option>)}
            </Select>
          </Field>;
          return <Field key={option.key} label={option.label} help={live?.options.find((item) => item.key === option.key)?.help ?? option.help}>
            <Input type={option.type === "number" ? "number" : "text"} value={String(value ?? "")} onChange={(event) => change(contract.name, option.key, event.target.value || null)} disabled={busy || reading || !switchable || !projectId} />
          </Field>;
        })}
        {current && <section className="module-current" aria-label={`${contract.name} current behavior`}>
          <h4>What it does now</h4>
          <p>Canonical address in use: <a href={current.canonical.url} target="_blank" rel="noreferrer">{current.canonical.url}</a></p>
          <p>{current.canonical.reason}</p>
          <p><a href={new URL("sitemap.xml", current.canonical.url.endsWith("/") ? current.canonical.url : `${current.canonical.url}/`).toString()} target="_blank" rel="noreferrer">Open sitemap</a></p>
          <h4>Pages left out</h4>
          {current.excluded.length ? <ul>{current.excluded.map((page) => <li key={page.page}>{page.page}: {page.reason}</li>)}</ul> : <p>No pages are left out.</p>}
        </section>}
        {switchable && live && <div className="module-settings-actions">
          <Button disabled={busy || reading || !changed} onClick={() => void save(contract.name, undefined, draftOptions(contract.options, draft))}>Save changes</Button>
          <Switch checked={state.on === true} onChange={(next) => void save(contract.name, next, draftOptions(contract.options, draft))} label={`Turn ${contract.name} ${state.on ? "off" : "on"}`} disabled={busy || reading} />
        </div>}
      </article>;
    })}
    {notice && <p role="status">{notice}</p>}
    {!projectId && <p>This project is not published yet. Module settings travel with the first release. Publish it to read and change settings here.</p>}
  </section>;
}

function draftOptions(options: readonly SdkModuleOption[], draft: Record<string, unknown>) {
  return Object.fromEntries(Object.entries(draft).filter(([key, value]) => {
    const option = options.find((item) => item.key === key);
    return value !== null || option?.type === "path";
  }));
}
