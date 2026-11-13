import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUpRight, BarChart3, RefreshCw } from "lucide-react";
import { WorkspaceNav } from "./WorkspaceNav";
import { Alert, Badge, BarList, Breadcrumb, Button, EmptyState, LineChart, LoadingBlock, Segmented, Select } from "./ui";
import { UsageLoadingShapes } from "./BootShell";
import { useLoadingHold } from "./loading-hold";
import type { OwnerSession } from "./owner-session";
import { readOwnerProjectCount } from "../lib/project-sources";
import { dailyActiveAccounts, dailyExecutions, dailyUsage, executionTotals, featureRows, formatBytes, kindTotal, noticeOutcomes, outcomeLabel, pageViews, readUsageOverview, readUsageSummary, storageParts, storageRows, usageActivity, usageRange, type UsageApp, type UsageSummary } from "../lib/usage";
import { DailyChart } from "./UsageChart";
import "./usage.css";

const number = new Intl.NumberFormat("en");
const dateTime = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const formatTime = (at: number) => dateTime.format(at);
const dayLabel = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
const deploymentId = () => new URLSearchParams(window.location.search).get("deployment");
const projectHref = (projectId: string) => `/app/${encodeURIComponent(projectId)}`;
const outcomeColor = (outcome: string) => outcome === "ok" ? "var(--ds-s1)" : outcome === "failed" ? "var(--ds-danger)" : outcome === "refused" ? "var(--ds-warning-fill)" : undefined;
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
  // The first read keeps its skeleton long enough that it never flashes.
  const reading = useLoadingHold(!updatedAt && !error, { sinceNavigation: true });
  const range = usageRange(period);
  return <div className="account-home">
    <WorkspaceNav active="usage" projectCount={projectCount} ownerSession={ownerSession} />
    <main className="account-main"><section className="account-content usage-main">
      {selected && <Breadcrumb items={[{ label: "Usage", href: "/usage" }, { label: summary?.projectName ?? "App" }]} />}
      <header className="account-hero usage-heading">
        <div><h1>{selected ? summary?.projectName ?? "App usage" : "Usage"}</h1>
          <p>{selected ? "What this published app did in the period, and what it could not do." : "What your published apps did in the period. Drafts are listed without figures until you publish them."}</p></div>
        <div className="usage-controls"><label>Period<Select value={period} onChange={(event) => setPeriod(Number(event.target.value))}><option value={7}>Last 7 days</option><option value={30}>Last 30 days</option><option value={90}>Last 90 days</option></Select></label>
          <Button disabled={pending} onClick={() => void load()}><RefreshCw size={14} /> Refresh</Button></div>
      </header>
      <div className="usage-range"><span>{range.from} to {range.to} · UTC</span><span>{updatedAt ? `Updated ${formatTime(updatedAt)}` : "Waiting for Kernel data"}</span></div>
      {error && <Alert tone="danger" className="usage-error" title="Usage could not be refreshed" action={<Button onClick={() => void load()}>Try again</Button>}>{updatedAt ? "The figures below are from the last successful read. " : ""}{error}</Alert>}
      {reading && <LoadingBlock label="Loading Konae" sinceNavigation><UsageLoadingShapes /></LoadingBlock>}
      {!selected && updatedAt && !reading && <AccountUsage apps={apps} />}
      {!selected && nextCursor && <Button disabled={pending} onClick={() => void load(nextCursor)}>Load more apps</Button>}
      {selected && summary && !reading && <AppUsage summary={summary} />}
      <p className="usage-footnote">Counts are approximate and belong to each app's Kernel. Editor previews are excluded. Page views are browser loads of the app's pages, not people. People are counted only when they are signed in to an app account; a visitor without an account is never counted as a person. The page refreshes every minute while it is open.</p>
    </section></main>
  </div>;
}

function AccountUsage({ apps }: { apps: UsageApp[] }) {
  if (!apps.length) return <EmptyState className="usage-empty" icon={<BarChart3 size={22} />} title="No apps yet" description="Create a project and publish it. Its usage appears here." action={<a className="ui-button ui-button-primary" href="/">Go to projects</a>} />;
  return <><OverviewCharts apps={apps} /><section aria-labelledby="usage-apps-heading"><h2 id="usage-apps-heading">Apps</h2>
    <div className="ui-table-scroll"><table className="ui-table"><caption>Activity in the selected period. Open an app for feature details.</caption><thead><tr><th scope="col">App</th><th scope="col">Page views</th><th scope="col">Active accounts</th><th scope="col">Recorded operations</th><th scope="col">Execution results</th><th scope="col">Features with activity</th><th scope="col">Latest recorded day</th><th scope="col">Storage</th><th scope="col">Usage</th></tr></thead>
      <tbody>{apps.map((app) => {
        const value = app.summary;
        if (!value) return <tr key={app.projectId}><th scope="row">{app.name}<small>{app.error ? "Read failed" : "Draft"}</small></th>
          <td colSpan={7} data-label="Usage">{app.error ?? "Not published, so nothing is recorded. Publish it to collect usage."}</td>
          <td data-label="Project"><a href={projectHref(app.projectId)}>Open project</a></td></tr>;
        const features = featureRows(value).filter((f) => f.count > 0 || f.failures > 0 || f.refused > 0)
          .map((f) => f.id === "notify" && f.notSent ? <span key={f.id} className="usage-failure">{f.notSent === f.count ? `${f.name} (none sent)` : `${f.name} (${number.format(f.notSent)} not sent)`}</span> : <span key={f.id}>{f.name}</span>);
        const latest = [...value.usage.days.map((d) => d.day), ...value.metrics.map((m) => m.day)].sort().at(-1);
        const outcomes = executionTotals(value.metrics);
        const hasOutcomes = value.metrics.some((m) => m.feature === "execution");
        return <tr key={app.projectId}><th scope="row">{app.name}<small><a href={value.url} target="_blank" rel="noopener">{new URL(value.url).hostname}</a></small></th>
          <td data-label="Page views">{number.format(kindTotal(value, "page_view"))}</td>
          <td data-label="Active accounts">{value.accounts ? number.format(value.accounts.active) : "Not reported"}</td>
          <td data-label="Operations">{value.usage.days.length || hasOutcomes ? number.format(usageActivity(value)) : "No data yet"}</td>
          <td data-label="Results">{hasOutcomes ? <span className="usage-overview-results"><span>{number.format(outcomes.ok ?? 0)} correct</span><span>{number.format(outcomes.refused ?? 0)} refused</span><span className={(outcomes.failed ?? 0) > 0 ? "usage-failure" : undefined}>{number.format(outcomes.failed ?? 0)} failed</span>{(outcomes.unchanged ?? 0) > 0 && <span>{number.format(outcomes.unchanged)} no changes</span>}</span> : "No outcomes yet"}</td>
          <td data-label="Active features"><span>{features.length ? features.flatMap((f, i) => i ? [", ", f] : [f]) : "No recorded activity"}</span></td>
          <td data-label="Latest day">{latest ?? "No data"}</td><td data-label="Storage">{value.storage ? formatBytes(value.storage.total) : "Not reported"}</td><td data-label="Detail"><a href={`/usage?deployment=${encodeURIComponent(value.deploymentId)}`} aria-label={`View usage for ${app.name}`}>View usage <ArrowUpRight size={13} /></a></td></tr>;
      })}</tbody></table></div>
    <p className="usage-note">Recorded operations count successful action calls, HTTP requests, scheduled attempts and queued service calls. Results cover instrumented executions only and may cover less history than operations. Authentication and file activity appear in the feature detail. Active accounts count signed-in app accounts, each app on its own, so one person with accounts in two apps counts in both. Storage is what each app takes now, whatever the period.</p>
  </section></>;
}

/** The apps side by side: who is used, and who takes the room. */
function OverviewCharts({ apps }: { apps: UsageApp[] }) {
  const published = apps.flatMap((app) => app.summary ? [{ app, summary: app.summary }] : []);
  if (!published.length) return null;
  const stored = published.filter(({ summary }) => summary.storage);
  const tiles = [
    ["Published apps", number.format(published.length)],
    ["Page views", number.format(published.reduce((sum, { summary }) => sum + kindTotal(summary, "page_view"), 0))],
    ["Active accounts", number.format(published.reduce((sum, { summary }) => sum + (summary.accounts?.active ?? 0), 0))],
    ["Storage now", stored.length ? formatBytes(stored.reduce((sum, { summary }) => sum + summary.storage!.total, 0)) : "Not reported"],
  ] as const;
  return <section aria-labelledby="usage-overview-heading"><h2 id="usage-overview-heading">All apps</h2>
    <dl className="ui-stats">{tiles.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
    <div className="usage-charts">
      <div className="usage-chart-card"><BarList title="Recorded operations by app" unit="operations" labelHeading="App"
        rows={published.map(({ app, summary }) => ({ key: app.projectId, label: <a href={`/usage?deployment=${encodeURIComponent(summary.deploymentId)}`}>{app.name}</a>, value: usageActivity(summary), detail: `${number.format(kindTotal(summary, "page_view"))} page views` }))}
        empty="No app recorded an operation in this period." /></div>
      <div className="usage-chart-card"><BarList title="Storage by app" unit="bytes" labelHeading="App" format={formatBytes}
        rows={stored.map(({ app, summary }) => ({ key: app.projectId, label: <a href={`/usage?deployment=${encodeURIComponent(summary.deploymentId)}`}>{app.name}</a>, value: summary.storage!.total, detail: `${number.format(summary.storage!.files)} uploaded ${summary.storage!.files === 1 ? "file" : "files"}` }))}
        empty={stored.length ? "No app stores anything yet." : "These apps run on a Kernel that does not report storage."} /></div>
    </div>
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
    <div className="usage-app-link"><a href={summary.url} target="_blank" rel="noopener">{new URL(summary.url).hostname} <ArrowUpRight size={14} /></a><a href={projectHref(summary.projectId)}>Open project</a></div>
    {notices.notSent > 0 && <Alert tone="warning" className="usage-error" title={`${number.format(notices.notSent)} email ${notices.notSent === 1 ? "notice was" : "notices were"} not sent`}>{[...new Set(notices.reasons.map((r) => r.label))].map((label) => label[0].toUpperCase() + label.slice(1)).join(". ")}. The actions themselves completed; only the email is missing.{notices.reasons.some((r) => r.reason === "no-transport") && " Whoever runs this installation can connect an email service."}</Alert>}
    <section className="usage-coverage" aria-label="Data coverage"><p>{coverage.startedAt ? `Execution outcomes and business event totals have been collected since ${formatTime(coverage.startedAt)}.` : "No execution outcome history yet. It starts when this Kernel records the next operation."} Earlier operation counts may still appear below.</p>
      <p>Up to {coverage.retentionDays} days are retained, subject to storage limits. Blank history does not prove a feature was never used.</p>
      {coverage.prunedThrough !== null && coverage.prunedThrough !== undefined && coverage.prunedThrough >= Date.parse(`${summary.usage.from}T00:00:00Z`) && <p>Retention has removed some records through {new Date(coverage.prunedThrough).toISOString().slice(0, 10)}. This period is incomplete.</p>}
      {coverage.queueFlushed === false && <p role="status">The capture queue did not finish writing before this read. Recent activity may be missing. Refresh to try again.</p>}
      {(coverage.installationDroppedSinceRestart > 0 || coverage.installationWriteErrorsSinceRestart > 0) && <p role="status">This Kernel has dropped {number.format(coverage.installationDroppedSinceRestart)} entries and encountered {number.format(coverage.installationWriteErrorsSinceRestart)} write errors since restart. These totals cover the whole installation and cannot be attributed to this app.</p>}
    </section>
    <Traffic summary={summary} />
    <People summary={summary} />
    <Storage summary={summary} />
    <section aria-labelledby="usage-results-heading"><h2 id="usage-results-heading">Execution summary</h2>
      {summary.metrics.some((m) => m.feature === "execution") ? <dl className="ui-stats">{strip.map(([key, label]) => <div key={key} className={key === "failed" && (outcomes.failed ?? 0) > 0 ? "usage-failure" : undefined}><dt>{label}</dt><dd>{number.format(outcomes[key] ?? 0)}</dd></div>)}</dl> : <p>No execution outcomes recorded in this period.</p>}
      <DailyChart title="Executions per day" unit="executions" points={dailyExecutions(summary)} partLabel={outcomeLabel} partColor={outcomeColor} stacked empty="No execution outcomes recorded in this period." />
      <a className="usage-jump" href="#execution-detail">See results by action</a>
    </section>
    <section aria-labelledby="usage-features-heading"><h2 id="usage-features-heading">Features</h2><div className="ui-table-scroll"><table className="ui-table"><caption>Current published configuration and recorded activity in this period.</caption><thead><tr><th scope="col">Feature</th><th scope="col">Published state</th><th scope="col">Recorded activity</th><th scope="col">Latest recorded day</th></tr></thead><tbody>{rows.map((row) => <tr key={row.id}><th scope="row">{row.name}<small>{row.description}</small></th><td data-label="Published state">{row.configured}</td><td data-label="Activity">{row.count ? <><strong>{number.format(row.count)}</strong>{row.id === "notify"
      ? <>{notices.queued > 0 && <small>{number.format(notices.queued)} queued for sending</small>}{notices.reasons.map((r) => <small key={`${r.action}/${r.reason}`} className="usage-failure">{r.action} · {number.format(r.count)} not sent: {r.label}</small>)}</>
      : <small>{row.totals.map((t) => `${t.name || t.kind} · ${number.format(t.count)}`).join("; ")}</small>}</> : "No recorded operations"}{row.failures > 0 && <small className="usage-failure">{number.format(row.failures)} execution failures</small>}{row.refused > 0 && <small>{number.format(row.refused)} refused by app rules</small>}</td><td data-label="Latest day">{row.lastDay ?? "No data"}</td></tr>)}</tbody></table></div></section>
    <section id="execution-detail" aria-labelledby="usage-actions-heading"><h2 id="usage-actions-heading">Results by action</h2><p className="usage-note">These counts cover the instrumented history only.</p>
      <div className="ui-table-scroll"><table className="ui-table"><caption>One execution per origin. Platform access refusals and invalid inputs are excluded.</caption><thead><tr><th scope="col">Action</th><th scope="col">Origin</th><th scope="col">Result</th><th scope="col">Count</th><th scope="col">Last activity</th></tr></thead><tbody>{actionNames.flatMap((name) => {
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

function Traffic({ summary }: { summary: UsageSummary }) {
  const pages = pageViews(summary);
  const tiles = [["Page views", kindTotal(summary, "page_view")], ["Sessions opened", kindTotal(summary, "session")], ["Signups", kindTotal(summary, "signup")], ["Sign-ins", kindTotal(summary, "login")]] as const;
  return <section aria-labelledby="usage-traffic-heading"><h2 id="usage-traffic-heading">Traffic</h2>
    <p className="usage-note">The Kernel counts a page view when a browser opens one of the app's pages, by the file it served. Scripts, crawlers and prefetches are not counted. A single-page app counts its first load, not the screens it shows after that.</p>
    <dl className="ui-stats">{tiles.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{number.format(value)}</dd></div>)}</dl>
    <DailyChart title="Page views per day" unit="page views" points={dailyUsage(summary, "page_view")} empty="No page views recorded in this period. Views count from the first browser visit after this Kernel was updated." />
    {pages.length > 1 && <div className="ui-table-scroll"><table className="ui-table"><caption>Page views by page in this period.</caption><thead><tr><th scope="col">Page</th><th scope="col">Views</th></tr></thead>
      <tbody>{pages.map((row) => <tr key={row.page}><th scope="row">{row.page}</th><td data-label="Views">{number.format(row.count)}</td></tr>)}</tbody></table></div>}
  </section>;
}

function People({ summary }: { summary: UsageSummary }) {
  const [sort, setSort] = useState<"actions" | "opens">("actions");
  const accounts = summary.accounts;
  const heading = <h2 id="usage-people-heading">People</h2>;
  if (!accounts) return <section aria-labelledby="usage-people-heading">{heading}<p>This app's Kernel does not count activity per account. Whoever runs this installation can update its Kernel to collect it.</p></section>;
  const days = dailyActiveAccounts(summary);
  const name = (row: { email: string | null; account: string }) => row.email ?? `Account removed (${row.account.slice(0, 8)})`;
  const tiles = [["Active accounts", accounts.active], ["App opens", accounts.opens], ["Actions", accounts.actions]] as const;
  return <section aria-labelledby="usage-people-heading">{heading}
    <p className="usage-note">App opens count session checks made while signed in. Actions count successful calls, including service calls accepted into the queue. A visitor without an account is never counted as a person, and an owner who entered as an account counts nothing.{accounts.startedAt ? ` Counted since ${formatTime(accounts.startedAt)}.` : ""}</p>
    <dl className="ui-stats">{tiles.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{number.format(value)}</dd></div>)}</dl>
    <div className="usage-charts">
      <div className="usage-chart-card"><LineChart title="Active accounts per day" unit="accounts" labels={days.map((d) => dayLabel.format(Date.parse(`${d.day}T00:00:00Z`)))}
        series={[{ id: "active", label: "Active accounts", values: days.map((d) => d.value) }]} area empty="No signed-in account used the app in this period." /></div>
      <div className="usage-chart-card">
        <div className="usage-chart-tools"><Segmented label="Rank accounts by" value={sort} onChange={setSort} options={[{ value: "actions", label: "Actions" }, { value: "opens", label: "App opens" }]} /></div>
        <BarList title={sort === "actions" ? "Actions by account" : "App opens by account"} unit={sort === "actions" ? "actions" : "app opens"} labelHeading="Account" total={sort === "actions" ? accounts.actions : accounts.opens}
          rows={accounts.top.map((row) => ({ key: row.account, label: name(row), value: row[sort], detail: `${number.format(sort === "actions" ? row.opens : row.actions)} ${sort === "actions" ? "app opens" : "actions"} · ${row.activeDays} ${row.activeDays === 1 ? "day" : "days"} active` }))}
          empty={sort === "actions" ? "No successful actions recorded for these accounts in this period." : "No app opens recorded for these accounts in this period."} />
      </div>
    </div>
    {accounts.active > accounts.top.length && <p className="usage-note">The bars show the {accounts.limit} most active accounts by combined opens and actions. Totals include all {number.format(accounts.active)} active accounts.</p>}
    {accounts.top.length > 0 && <details className="ui-disclosure"><summary>Show accounts as a table</summary>
      <div className="ui-table-scroll"><table className="ui-table"><caption>The {accounts.limit} most active accounts in this period{accounts.active > accounts.top.length ? `, of ${number.format(accounts.active)}` : ""}.</caption>
        <thead><tr><th scope="col">Account</th><th scope="col">App opens</th><th scope="col">Actions</th><th scope="col">Days active</th><th scope="col">Last activity</th></tr></thead>
        <tbody>{accounts.top.map((row) => <tr key={row.account}><th scope="row">{name(row)}</th><td data-label="App opens">{number.format(row.opens)}</td><td data-label="Actions">{number.format(row.actions)}</td><td data-label="Days active">{row.activeDays}</td><td data-label="Last activity">{formatTime(row.lastAt)}</td></tr>)}</tbody></table></div>
    </details>}
  </section>;
}

function Storage({ summary }: { summary: UsageSummary }) {
  const storage = summary.storage;
  const heading = <h2 id="usage-storage-heading">Storage</h2>;
  if (!storage) return <section aria-labelledby="usage-storage-heading">{heading}<p>This app's Kernel does not report storage. Whoever runs this installation can update its Kernel to report it.</p></section>;
  const rows = storageRows(storage);
  const backups = storage.parts.find((part) => part.part === "backups");
  return <section aria-labelledby="usage-storage-heading">{heading}
    <p className="usage-note">What the app takes on this installation now, whatever the period. An export carries all of it except backups. Uploaded files count what their index records, wherever the bytes are kept.</p>
    <div className="usage-chart-card"><BarList title="Storage by part" unit="bytes" labelHeading="Part" format={formatBytes} total={storage.total}
      rows={rows.map((row) => ({ key: row.part, label: row.name, value: row.bytes, tone: row.tone,
        detail: [row.part === "files" ? `${number.format(storage.files)} ${storage.files === 1 ? "file" : "files"}` : storageParts[row.part]?.description, row.limit ? `${row.bytes > 0 && (row.share ?? 0) < 0.01 ? "Under 1%" : `${Math.round((row.share ?? 0) * 100)}%`} of the ${formatBytes(row.limit)} limit` : null].filter(Boolean).join(" · ") }))}
      empty="The app stores nothing yet." /></div>
    {backups && backups.bytes > 0 && <p className="usage-note">Backups on this installation take {formatBytes(backups.bytes)} more. They stay here.</p>}
  </section>;
}
