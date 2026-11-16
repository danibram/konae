import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { sdkContract, type SdkModuleOption } from "../sdk/contract";
import { isModuleChanged, readModules, setModule, type ModuleRead } from "../deployments/client";
import { isSignedOutError } from "./owner-session";
import { Badge, Button, Field, Input, LoadingBlock, Select, Switch } from "./ui";

type StateMap = Record<string, { on: boolean; [key: string]: unknown }>;

export function ModulesSettings({ projectId, initial, files = [], onSignedOut }: { projectId: string | null; initial: StateMap; files?: string[]; onSignedOut: () => void }) {
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
  const switchable = entries.filter(({ contract, live }) => live?.switch ?? contract.switch);
  const fixed = entries.filter(({ contract, live }) => !(live?.switch ?? contract.switch));
  const imageFiles = useMemo(() => files.filter((file) => /\.(?:avif|gif|jpe?g|png|svg|webp)$/i.test(file)), [files]);

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
      setNotice(`${contract.title} settings saved.`);
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
    {!reading && switchable.map(({ contract, live, initialState }) => {
      const state = live?.state ?? initialState;
      const draft = drafts[contract.name] ?? {};
      const values = { ...state, ...draft };
      const changed = Object.keys(draft).length > 0;
      const switchable = live?.switch ?? contract.switch;
      return <article className="module-settings-card" key={contract.name}>
        <header><div><h3>{contract.title}</h3><small className="module-settings-key">{contract.name}</small><p>{live?.summary ?? contract.summary}</p></div><Badge tone={state.on ? "success" : "neutral"}>{state.on ? "On" : "Off"}</Badge></header>
        {(contract.options as readonly SdkModuleOption[]).map((option) => {
          const value = values[option.key] ?? null;
          if (option.type === "boolean") return <Switch key={option.key} checked={value === true} onChange={(next) => change(contract.name, option.key, next)} label={option.label} disabled={busy || reading || !switchable || !projectId} />;
          if (option.type === "enum") return <Field key={option.key} label={option.label} help={live?.options.find((item) => item.key === option.key)?.help ?? option.help}>
            <Select value={String(value ?? "")} onChange={(event) => change(contract.name, option.key, event.target.value || null)} disabled={busy || reading || !switchable || !projectId}>
              {value === null && <option value="">{option.defaultLabel ?? "Use the default"}</option>}
              {(option.values ?? []).map((item) => <option key={item} value={item}>{option.valueLabels?.[item] ?? item}</option>)}
            </Select>
          </Field>;
          if (option.valuesFrom === "app-images") {
            const choices = value && !imageFiles.includes(String(value)) ? [String(value), ...imageFiles] : imageFiles;
            return <Field key={option.key} label={option.label} help={live?.options.find((item) => item.key === option.key)?.help ?? option.help}>
              <Select value={String(value ?? "")} onChange={(event) => change(contract.name, option.key, event.target.value || null)} disabled={busy || reading || !switchable || !projectId}>
                <option value="">No image</option>
                {choices.map((item) => <option key={item} value={item}>{item}</option>)}
              </Select>
            </Field>;
          }
          return <Field key={option.key} label={option.label} help={live?.options.find((item) => item.key === option.key)?.help ?? option.help}>
            <Input type={option.type === "number" ? "number" : "text"} value={String(value ?? "")} onChange={(event) => change(contract.name, option.key, event.target.value || null)} disabled={busy || reading || !switchable || !projectId} />
          </Field>;
        })}
        {statusByModule[contract.name]?.(live?.current)}
        {live && <div className="module-settings-actions">
          <Button disabled={busy || reading || !changed} onClick={() => void save(contract.name, undefined, draftOptions(contract.options, draft))}>Save changes</Button>
          <Switch checked={state.on === true} onChange={(next) => void save(contract.name, next, draftOptions(contract.options, draft))} label={`Turn ${contract.title} ${state.on ? "off" : "on"}`} disabled={busy || reading} />
        </div>}
      </article>;
    })}
    {!reading && fixed.length > 0 && <section className="module-settings-fixed" aria-label="Modules that are always on">
      <h3>Always on</h3>
      {fixed.map(({ contract, live }) => <article className="module-settings-fixed-item" key={contract.name}>
        <div><h4>{contract.title}</h4><small className="module-settings-key">{contract.name}</small></div>
        <p>{live?.summary ?? contract.summary}</p>
      </article>)}
    </section>}
    {notice && <p role="status">{notice}</p>}
    {!projectId && <p>This project is not published yet. Module settings travel with the first release. Publish it to read and change settings here.</p>}
  </section>;
}

function SeoStatus({ current }: { current: unknown }): ReactNode {
  if (!isRecord(current) || !isRecord(current.canonical) || typeof current.canonical.url !== "string" || typeof current.canonical.reason !== "string" || !Array.isArray(current.excluded)) return null;
  const canonical = current.canonical;
  const excluded = current.excluded.filter((page): page is { page: string; reason: string } => isRecord(page) && typeof page.page === "string" && typeof page.reason === "string");
  return <section className="module-current" aria-label="Search listing current behavior">
    <h4>What it does now</h4>
    <p>Canonical address in use: <a href={canonical.url} target="_blank" rel="noreferrer">{canonical.url}</a></p>
    <p>{canonical.reason}</p>
    <p><a href={new URL("sitemap.xml", canonical.url.endsWith("/") ? canonical.url : `${canonical.url}/`).toString()} target="_blank" rel="noreferrer">Open sitemap</a></p>
    <h4>Pages left out</h4>
    {excluded.length ? <ul>{excluded.map((page) => <li key={page.page}>{page.page}: {page.reason}</li>)}</ul> : <p>No pages are left out.</p>}
  </section>;
}

function isRecord(value: unknown): value is Record<string, any> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

const statusByModule: Record<string, (current: unknown) => ReactNode> = { seo: (current) => <SeoStatus current={current} /> };

function draftOptions(options: readonly SdkModuleOption[], draft: Record<string, unknown>) {
  return Object.fromEntries(Object.entries(draft).filter(([key, value]) => {
    const option = options.find((item) => item.key === key);
    return value !== null || option?.type === "path";
  }));
}
