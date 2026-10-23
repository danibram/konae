import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUpRight, BarChart3, RefreshCw } from "lucide-react";
import { WorkspaceNav } from "./WorkspaceNav";
import { Badge, Button, EmptyState, Select } from "./ui";
import type { OwnerSession } from "./owner-session";
import { readOwnerProjectCount } from "../lib/managed-sources";
import { executionTotals, featureRows, noticeOutcomes, outcomeLabel, readUsageOverview, readUsageSummary, usageActivity, usageRange, type Metric, type UsageApp, type UsageSummary } from "../lib/usage";
import "./usage.css";

const number = new Intl.NumberFormat("en");
const dateTime = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const formatTime = (at: number) => dateTime.format(at);
const deploymentId = () => new URLSearchParams(window.location.search).get("deployment");
const projectHref = (projectId: string) => `/app/${encodeURIComponent(projectId)}`;
const outcomeTone = (outcome: string) => outcome === "ok" ? "success" : outcome === "failed" ? "danger" : outcome === "refused" ? "warning" : "neutral";

export function UsagePage({ ownerSession }: { ownerSession: OwnerSession }) {
  const [period, setPeriod] = useState(30);
  const [apps, setApps] = useState<UsageApp[]>([]);
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [pending, setPending] = useState(true);
  const [error, setError] = useState("");
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [projectCount, setProjectCount] = useState<number | undefined>();
  const controller = useRef<AbortController | null>(null);
  const selected = deploymentId();
  useEffect(() => {
    if (ownerSession.state !== "signed-in") {
      setProjectCount(undefined);
      return;
    }
    let active = true;
    void readOwnerProjectCount(ownerSession.ownerId)
      .then((count) => { if (active) setProjectCount(count); })
      .catch(() => { if (active) setProjectCount(undefined); });
    return () => { active = false; };
  }, [ownerSession]);
  const load = useCallback(async (cursor?: string) => {
    controller.current?.abort();
    const request = new AbortController();
    controller.current = request;
    setPending(true);
    setError("");
    try {
      if (selected) {
        const value = await readUsageSummary(selected, period, request.signal);
        if (request.signal.aborted) return;
        setSummary(value);
        setUpdatedAt(value.updatedAt);
      } else {
        const value = await readUsageOverview(period, cursor, request.signal);
        if (request.signal.aborted) return;
        setApps((current) => cursor ? [...current, ...value.apps] : value.apps);
        setNextCursor(value.nextCursor);
        setUpdatedAt(Date.now());
      }
    } catch (cause) {
      if (!request.signal.aborted) setError(cause instanceof Error ? cause.message : "Usage could not be read.");
    } finally {
      if (!request.signal.aborted) setPending(false);
    }
  }, [period, selected]);
  useEffect(() => {
    setApps([]); setSummary(null); setUpdatedAt(null); setNextCursor(null);
    void load();
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") void load(); }, 60_000);
    return () => { window.clearInterval(timer); controller.current?.abort(); };
  }, [load]);
  const range = usageRange(period);
  return <div className="account-home">
    <WorkspaceNav active="usage" projectCount={projectCount} ownerSession={ownerSession} />
    <main className="account-main"><section className="account-content usage-main">
      {selected && <nav className="usage-breadcrumb" aria-label="Breadcrumb"><a href="/usage">Usage</a><span aria-hidden="true">/</span><span aria-current="page">{summary?.projectName ?? "App"}</span></nav>}
      <header className="account-hero usage-heading">
        <div><h1>{selected ? summary?.projectName ?? "App usage" : "Usage"}</h1>
          <p>{selected ? "What this published app did in the period, and what it could not do." : "What your published apps did in the period. Drafts are listed without figures until you publish them."}</p></div>
        <div className="usage-controls"><label>Period<Select value={period} onChange={(event) => setPeriod(Number(event.target.value))}><option value={7}>Last 7 days</option><option value={30}>Last 30 days</option><option value={90}>Last 90 days</option></Select></label>
          <Button disabled={pending} onClick={() => void load()}><RefreshCw size={14} /> Refresh</Button></div>
      </header>
      <div className="usage-range"><span>{range.from} to {range.to} · UTC</span><span>{updatedAt ? `Updated ${formatTime(updatedAt)}` : "Waiting for Kernel data"}</span></div>
      {error && <div className="usage-error" role="alert"><p>Usage could not be refreshed. {updatedAt ? "The figures below are from the last successful read. " : ""}{error}</p><Button onClick={() => void load()}>Try again</Button></div>}
      {pending && !updatedAt && <p role="status">Reading usage from the Kernel…</p>}
      {!selected && updatedAt && <AccountUsage apps={apps} />}
      {!selected && nextCursor && <Button disabled={pending} onClick={() => void load(nextCursor)}>Load more apps</Button>}
      {selected && summary && <AppUsage summary={summary} />}
      <p className="usage-footnote">Counts are approximate and belong to each app's Kernel. Editor previews are excluded. Session checks are not visits or unique users. The page refreshes every minute while it is open.</p>
    </section></main>
  </div>;
}

function AccountUsage({ apps }: { apps: UsageApp[] }) {
  if (!apps.length) return <EmptyState className="usage-empty" icon={<BarChart3 size={22} />} title="No apps yet" description="Create a project and publish it. Its usage appears here." action={<a className="ui-button ui-button-primary" href="/">Go to projects</a>} />;
  return <section aria-labelledby="usage-apps-heading"><h2 id="usage-apps-heading">Apps</h2>
    <div className="usage-table-scroll"><table className="usage-table"><caption>Activity in the selected period. Open an app for feature details.</caption><thead><tr><th scope="col">App</th><th scope="col">Recorded operations</th><th scope="col">Execution results</th><th scope="col">Features with activity</th><th scope="col">Latest recorded day</th><th scope="col">Usage</th></tr></thead>
      <tbody>{apps.map((app) => {
        const value = app.summary;
        if (!value) return <tr key={app.projectId}><th scope="row">{app.name}<small>{app.error ? "Read failed" : "Draft"}</small></th>
          <td colSpan={4} data-label="Usage">{app.error ?? "Not published, so nothing is recorded. Publish it to collect usage."}</td>
          <td data-label="Project"><a href={projectHref(app.projectId)}>Open project</a></td></tr>;
        const features = featureRows(value).filter((f) => f.count > 0 || f.failures > 0 || f.refused > 0)
          .map((f) => f.id === "notify" && f.notSent ? <span key={f.id} className="usage-failure">{f.notSent === f.count ? `${f.name} (none sent)` : `${f.name} (${number.format(f.notSent)} not sent)`}</span> : <span key={f.id}>{f.name}</span>);
        const latest = [...value.usage.days.map((d) => d.day), ...value.metrics.map((m) => m.day)].sort().at(-1);
        const outcomes = executionTotals(value.metrics);
        const hasOutcomes = value.metrics.some((m) => m.feature === "execution");
        return <tr key={app.projectId}><th scope="row">{app.name}<small><a href={value.url} target="_blank" rel="noopener">{new URL(value.url).hostname}</a></small></th>
          <td data-label="Operations">{value.usage.days.length || hasOutcomes ? number.format(usageActivity(value)) : "No data yet"}</td>
          <td data-label="Results">{hasOutcomes ? <span className="usage-overview-results"><span>{number.format(outcomes.ok ?? 0)} correct</span><span>{number.format(outcomes.refused ?? 0)} refused</span><span className={(outcomes.failed ?? 0) > 0 ? "usage-failure" : undefined}>{number.format(outcomes.failed ?? 0)} failed</span>{(outcomes.unchanged ?? 0) > 0 && <span>{number.format(outcomes.unchanged)} no changes</span>}</span> : "No outcomes yet"}</td>
          <td data-label="Active features"><span>{features.length ? features.flatMap((f, i) => i ? [", ", f] : [f]) : "No recorded activity"}</span></td>
          <td data-label="Latest day">{latest ?? "No data"}</td><td data-label="Detail"><a href={`/usage?deployment=${encodeURIComponent(value.deploymentId)}`} aria-label={`View usage for ${app.name}`}>View usage <ArrowUpRight size={13} /></a></td></tr>;
      })}</tbody></table></div>
    <p className="usage-note">Recorded operations count successful action calls, HTTP requests, scheduled attempts and queued service calls. Results cover instrumented executions only and may cover less history than operations. Authentication and file activity appear in the feature detail.</p>
  </section>;
}

function AppUsage({ summary }: { summary: UsageSummary }) {
  const rows = featureRows(summary);
  const coverage = summary.coverage;
  const outcomes = executionTotals(summary.metrics);
  const notices = noticeOutcomes(summary);
  const scheduled = summary.actions.some((a) => a.schedule) || (outcomes.unchanged ?? 0) > 0;
  const strip = [["ok", "Correct"], ["refused", "Refused by app rules"], ["failed", "Failed"], ...(scheduled ? [["unchanged", "Scheduled runs with no changes"]] : [])];
  const signalNames = [...new Set([...summary.actions.flatMap((a) => a.signal ? [a.signal.name] : []), ...summary.metrics.filter((m) => m.feature === "signal").map((m) => m.name)])].sort();
  const actionNames = [...new Set([...summary.actions.filter((a) => a.kind !== "call").map((a) => a.name), ...summary.metrics.filter((m) => m.feature === "execution").map((m) => m.name)])].sort();
  return <>
    <div className="usage-app-link"><a href={summary.url} target="_blank" rel="noopener">{new URL(summary.url).hostname} <ArrowUpRight size={14} /></a>{summary.sourceAuthority === "managed" ? <a href={projectHref(summary.projectId)}>Open project</a> : <span className="usage-note">External project · Files are managed outside Littlebox</span>}</div>
    {notices.notSent > 0 && <div className="usage-error" role="status"><p><strong>{number.format(notices.notSent)} email {notices.notSent === 1 ? "notice was" : "notices were"} not sent.</strong> {[...new Set(notices.reasons.map((r) => r.label))].map((label) => label[0].toUpperCase() + label.slice(1)).join(". ")}. The actions themselves completed; only the email is missing.{notices.reasons.some((r) => r.reason === "no-transport") && " Whoever runs this installation can connect an email service."}</p></div>}
    <section className="usage-coverage" aria-label="Data coverage"><p>{coverage.startedAt ? `Execution outcomes and business event totals have been collected since ${formatTime(coverage.startedAt)}.` : "No execution outcome history yet. It starts when this Kernel records the next operation."} Earlier operation counts may still appear below.</p>
      <p>Up to {coverage.retentionDays} days are retained, subject to storage limits. Blank history does not prove a feature was never used.</p>
      {coverage.prunedThrough !== null && coverage.prunedThrough !== undefined && coverage.prunedThrough >= Date.parse(`${summary.usage.from}T00:00:00Z`) && <p>Retention has removed some records through {new Date(coverage.prunedThrough).toISOString().slice(0, 10)}. This period is incomplete.</p>}
      {coverage.queueFlushed === false && <p role="status">The capture queue did not finish writing before this read. Recent activity may be missing. Refresh to try again.</p>}
      {(coverage.installationDroppedSinceRestart > 0 || coverage.installationWriteErrorsSinceRestart > 0) && <p role="status">This Kernel has dropped {number.format(coverage.installationDroppedSinceRestart)} entries and encountered {number.format(coverage.installationWriteErrorsSinceRestart)} write errors since restart. These totals cover the whole installation and cannot be attributed to this app.</p>}
    </section>
    <section aria-labelledby="usage-results-heading"><h2 id="usage-results-heading">Execution summary</h2>
      {summary.metrics.some((m) => m.feature === "execution") ? <dl className="usage-outcome-strip">{strip.map(([key, label]) => <div key={key} className={key === "failed" && (outcomes.failed ?? 0) > 0 ? "usage-failure" : undefined}><dt>{label}</dt><dd>{number.format(outcomes[key] ?? 0)}</dd></div>)}</dl> : <p>No execution outcomes recorded in this period.</p>}
      <a className="usage-jump" href="#execution-detail">See results by action</a>
    </section>
    <section aria-labelledby="usage-features-heading"><h2 id="usage-features-heading">Features</h2><div className="usage-table-scroll"><table className="usage-table"><caption>Current published configuration and recorded activity in this period.</caption><thead><tr><th scope="col">Feature</th><th scope="col">Published state</th><th scope="col">Recorded activity</th><th scope="col">Latest recorded day</th></tr></thead><tbody>{rows.map((row) => <tr key={row.id}><th scope="row">{row.name}<small>{row.description}</small></th><td data-label="Published state">{row.configured}</td><td data-label="Activity">{row.count ? <><strong>{number.format(row.count)}</strong>{row.id === "notify"
      ? <>{notices.queued > 0 && <small>{number.format(notices.queued)} queued for sending</small>}{notices.reasons.map((r) => <small key={`${r.action}/${r.reason}`} className="usage-failure">{r.action} · {number.format(r.count)} not sent: {r.label}</small>)}</>
      : <small>{row.totals.map((t) => `${t.name || t.kind} · ${number.format(t.count)}`).join("; ")}</small>}</> : "No recorded operations"}{row.failures > 0 && <small className="usage-failure">{number.format(row.failures)} execution failures</small>}{row.refused > 0 && <small>{number.format(row.refused)} refused by app rules</small>}</td><td data-label="Latest day">{row.lastDay ?? "No data"}</td></tr>)}</tbody></table></div></section>
    <section id="execution-detail" aria-labelledby="usage-actions-heading"><h2 id="usage-actions-heading">Results by action</h2><p className="usage-note">These counts cover the instrumented history only.</p>
      <DailyActivity metrics={summary.metrics} />
      <div className="usage-table-scroll"><table className="usage-table"><caption>One execution per origin. Platform access refusals and invalid inputs are excluded.</caption><thead><tr><th scope="col">Action</th><th scope="col">Origin</th><th scope="col">Result</th><th scope="col">Count</th><th scope="col">Last activity</th></tr></thead><tbody>{actionNames.flatMap((name) => {
        const groups = new Map<string, { origin: string; outcome: string; count: number; lastAt: number }>();
        summary.metrics.filter((m) => m.feature === "execution" && m.name === name).forEach((m) => { const key = `${m.origin}/${m.outcome}`; const current = groups.get(key); groups.set(key, { ...m, count: (current?.count ?? 0) + m.count, lastAt: Math.max(current?.lastAt ?? 0, m.lastAt) }); });
        if (!groups.size) return [<tr key={name}><th scope="row">{name}</th><td colSpan={4} data-label="Result">No recorded executions in this period</td></tr>];
        return [...groups.values()].map((g) => <tr key={`${name}/${g.origin}/${g.outcome}`}><th scope="row">{name}</th><td data-label="Origin">{g.origin}</td><td data-label="Result"><Badge tone={outcomeTone(g.outcome)}>{outcomeLabel(g.outcome)}</Badge></td><td data-label="Count">{number.format(g.count)}</td><td data-label="Last activity">{formatTime(g.lastAt)}</td></tr>);
      })}{!actionNames.length && <tr><td colSpan={5}>No actions declared or recorded.</td></tr>}</tbody></table></div>
    </section>
    <section aria-labelledby="usage-events-heading"><h2 id="usage-events-heading">Business events</h2><p className="usage-note">Events declared by the app, recorded by the Kernel. Completed mutations emit an event only when they change a row. Daily totals outlive the retained event detail.</p>
      {signalNames.length ? <ul className="usage-signals">{signalNames.map((name) => <li key={name}><span>{name}</span><strong>{number.format(summary.metrics.filter((m) => m.feature === "signal" && m.name === name).reduce((n, m) => n + m.count, 0))}</strong><span>recorded in instrumented history</span></li>)}</ul> : <p>No business events declared or recorded.</p>}
    </section>
    <section aria-labelledby="usage-delivery-heading"><h2 id="usage-delivery-heading">Delivery status</h2><p className="usage-note">Current state of retained jobs created in the selected period. Older job records may have been removed. Queued calls and accepted email are not confirmed deliveries.</p>
      <dl className="usage-delivery"><div><dt>Service calls</dt><dd>{summary.calls.states.map((s) => `${number.format(s.count)} ${s.state}`).join(" · ") || "No retained calls in this period"}</dd></div><div><dt>Email notices</dt><dd>{summary.mail.states.map((s) => `${number.format(s.count)} ${s.state}`).join(" · ") || (notices.notSent ? `No email jobs. ${number.format(notices.notSent)} ${notices.notSent === 1 ? "notice was" : "notices were"} not sent, so nothing was queued.` : "No retained notices in this period")}</dd></div></dl>
    </section>
  </>;
}

function DailyActivity({ metrics }: { metrics: Metric[] }) {
  const byDay = new Map<string, number>();
  for (const metric of metrics) if (metric.feature === "execution") byDay.set(metric.day, (byDay.get(metric.day) ?? 0) + metric.count);
  const values = [...byDay].sort(([a], [b]) => a.localeCompare(b));
  const max = Math.max(1, ...values.map(([, count]) => count));
  if (!values.length) return <p className="usage-note">No execution outcomes recorded in this period.</p>;
  return <details className="usage-daily"><summary>Daily execution activity</summary><ul>{values.map(([day, count]) => <li key={day}><span>{day}</span><span className="usage-volume-track" role="img" aria-label={`${day}, ${number.format(count)} total executions across all results`}><span style={{ width: `${count / max * 100}%` }} /></span><strong>{number.format(count)}</strong></li>)}</ul></details>;
}
