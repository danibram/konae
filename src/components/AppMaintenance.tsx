import { Wrench } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { readMaintenance, setMaintenance, type MaintenanceState } from "../deployments/client";
import { isSignedOutError } from "./owner-session";
import { Badge, Button, LoadingBlock } from "./ui";

/**
 * Puts the published app in maintenance or takes it out. Each change shows
 * the state the Kernel answers with, never the one the button asked for.
 */
export function AppMaintenance({ deploymentId, onSignedOut }: { deploymentId: string; onSignedOut: () => void }) {
  const [state, setState] = useState<MaintenanceState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [reload, setReload] = useState(0);
  const errorRef = useRef<HTMLParagraphElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { if (error) errorRef.current?.focus(); }, [error]);
  useEffect(() => {
    let current = true;
    setError("");
    readMaintenance(deploymentId).then((value) => { if (current) setState(value); }, (cause) => {
      if (!current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    });
    return () => { current = false; };
  }, [deploymentId, reload, onSignedOut]);

  const change = async (on: boolean) => {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const next = await setMaintenance(deploymentId, on);
      if (alive.current) setState(next);
    } catch (cause) {
      if (!alive.current) return;
      setError(cause instanceof Error ? cause.message : String(cause));
      if (isSignedOutError(cause)) onSignedOut();
    } finally { if (alive.current) setBusy(false); }
  };

  const own = state && state.maintenance && !state.installation;
  return <section className="domain-step" aria-label="Maintenance" aria-busy={busy || !state}>
    <header className="domain-heading"><Wrench size={20} /><h3>Maintenance</h3></header>
    {error && <p className="domain-error" role="alert" tabIndex={-1} ref={errorRef}>{error}</p>}
    {!state && !error && <LoadingBlock label="Reading maintenance" rows={1} />}
    {!state && error && <Button onClick={() => setReload((value) => value + 1)}>Retry loading</Button>}
    {state && <>
      <div className="domain-status"><Badge tone={state.maintenance ? "warning" : "success"}>{state.maintenance ? "In maintenance" : "Serving"}</Badge></div>
      {state.installation
        ? <p>{state.installation}</p>
        : state.maintenance
          ? <p>{state.unreadable ? "The app's maintenance state cannot be read, so the Kernel keeps it paused." : `Paused since ${new Date(state.since ?? 0).toLocaleString()}.`} Visitors see a maintenance page. Writes, signups, sign-ins, email, calls and scheduled runs wait until you take it out.</p>
          : <p>Pause the app while you move it or replace its data. Visitors see a maintenance page and nothing writes until you take it out.</p>}
      {!state.installation && <Button variant={own ? "primary" : "secondary"} loading={busy} onClick={() => void change(!own)}>{own ? "Take out of maintenance" : "Put in maintenance"}</Button>}
    </>}
  </section>;
}
