import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUpRight, BarChart3, RefreshCw } from "lucide-react";
import { WorkspaceNav } from "./WorkspaceNav";
import { Alert, Badge, BarList, Breadcrumb, Button, Disclosure, EmptyState, LineChart, LoadingBlock, Segmented, Select } from "./ui";
import { UsageLoadingShapes } from "./BootShell";
import { useLoadingHold } from "./loading-hold";
import type { OwnerSession } from "./owner-session";
import { readOwnerProjectCount } from "../lib/project-sources";
import { dailyActiveAccounts, dailyExecutions, dailyUsage, executionTotals, featureRows, formatBytes, kindTotal, noticeOutcomes, outcomeLabel, pageViews, readUsageOverview, readUsageSummary, storageParts, storageRows, usageRange, type UsageApp, type UsageSummary } from "../lib/usage";
import { DailyChart } from "./UsageChart";
import "./usage.css";

const number = new Intl.NumberFormat("en");
const dateTime = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const formatTime = (at: number) => dateTime.format(at);
const dayLabel = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
const yearDayLabel = new Intl.DateTimeFormat("en", { year: "numeric", month: "short", day: "numeric", timeZone: "UTC" });
const clock = new Intl.DateTimeFormat("en", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
/** The period in one format: "Oct 16 to Nov 14", with the year only when the period crosses one. */
export function rangeLabel(from: string, to: string): string {
  const format = from.slice(0, 4) === to.slice(0, 4) ? dayLabel : yearDayLabel;
  return `${format.format(Date.parse(`${from}T00:00:00Z`))} to ${format.format(Date.parse(`${to}T00:00:00Z`))}`;
}
const deploymentId = () => new URLSearchParams(window.location.search).get("deployment");
const projectHref = (projectId: string) => `/app/${encodeURIComponent(projectId)}`;
/** An app's address as the project cards write it: host and port, no scheme. */
const addressOf = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/$/, "");
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
  const title = selected ? summary?.projectName : undefined;
  useEffect(() => { document.title = title ? `${title} usage · Konae` : "Usage · Konae"; }, [title]);
  return <div className="account-home">
    <WorkspaceNav active="usage" projectCount={projectCount} ownerSession={ownerSession} />
    <main className="account-main"><section className="account-content usage-main">
      {selected && <Breadcrumb items={[{ label: "Usage", href: "/usage" }, { label: summary?.projectName ?? "App" }]} />}
      <header className="usage-heading">
        <h1>{selected ? summary?.projectName ?? "App usage" : "Usage"}</h1>
        <div className="usage-controls">
          <label className="usage-period">Period<Select value={period} onChange={(event) => setPeriod(Number(event.target.value))}><option value={7}>Last 7 days</option><option value={30}>Last 30 days</option><option value={90}>Last 90 days</option></Select></label>
          <Button disabled={pending} onClick={() => void load()}><RefreshCw size={14} aria-hidden="true" /> Refresh</Button>
        </div>
      </header>
      <p className="usage-range">{selected ? "What this published app did" : "What your published apps did"} from {rangeLabel(range.from, range.to)}, counted in UTC days. {updatedAt ? `Updated at ${clock.format(updatedAt)}, your time.` : "Reading usage…"}</p>
      {error && <Alert tone="danger" className="usage-error" title="Usage could not be refreshed" action={<Button onClick={() => void load()}>Try again</Button>}>{updatedAt ? "The figures below are from the last successful read. " : ""}{error}</Alert>}
      {reading && <LoadingBlock label="Loading Konae" sinceNavigation><UsageLoadingShapes /></LoadingBlock>}
      {!selected && updatedAt && !reading && <AccountUsage apps={apps} />}
      {!selected && nextCursor && <Button disabled={pending} onClick={() => void load(nextCursor)}>Load more apps</Button>}
      {selected && summary && !reading && <AppUsage summary={summary} />}
      <p className="usage-footnote">Counts are approximate. People are counted only when they are signed in to the app.</p>
      <Disclosure summary="How these are counted">{selected && summary && <AppCounting summary={summary} />}<p>Editor previews are excluded. Page views are browser loads of the app's pages, not people. A visitor without an account is never counted as a person. Each app keeps its own counts, and the page refreshes every minute while it is open.</p>{!selected && <p>Active accounts count signed-in app accounts, each app on its own, so one person with accounts in two apps counts in both. Storage is what each app takes now, whatever the period. Actions, results, sign-ins and file activity appear on each app's own page.</p>}</Disclosure>
    </section></main>
  </div>;
}

function AccountUsage({ apps }: { apps: UsageApp[] }) {
  if (!apps.length) return <EmptyState className="usage-empty" icon={<BarChart3 size={22} />} title="No apps yet" description="Create a project and deploy it. Its usage appears here." action={<a className="ui-button ui-button-primary" href="/">Go to projects</a>} />;
  // Published apps fill the table; drafts have nothing recorded, so they fold into one line under it.
  const drafts = apps.filter((app) => !app.summary && !app.error);
  const listed = [...apps.filter((app) => app.summary), ...apps.filter((app) => !app.summary && app.error)];
  return <><OverviewCharts apps={apps} /><section aria-labelledby="usage-apps-heading"><h2 id="usage-apps-heading">Apps</h2>
    <div className="ui-table-scroll"><table className="ui-table usage-apps"><caption>Each published app in the period. Its own page has actions, results and features.</caption><thead><tr><th scope="col">App</th><th scope="col">Page views</th><th scope="col">Active accounts</th><th scope="col">Storage</th><th scope="col"><span className="sr-only">Detail</span></th></tr></thead>
      <tbody>{listed.map((app) => {
        const value = app.summary;
        if (!value) return <tr key={app.projectId}><th scope="row">{app.name}<small>Read failed</small></th>
          <td colSpan={3} data-label="Usage">{app.error}</td>
          <td data-label="Project"><a href={projectHref(app.projectId)}>Open project</a></td></tr>;
        // Only trouble earns a line in the overview; the counts of what went well wait on the app's page.
        const failed = executionTotals(value.metrics).failed ?? 0;
        const notSent = noticeOutcomes(value).notSent;
        const trouble = [failed > 0 ? `${number.format(failed)} failed` : "", notSent > 0 ? `${number.format(notSent)} ${notSent === 1 ? "email" : "emails"} not sent` : ""].filter(Boolean).join(", ");
        return <tr key={app.projectId}><th scope="row">{app.name}<small><a className="usage-address" href={value.url} target="_blank" rel="noopener">{addressOf(value.url)}</a></small>{trouble && <small className="usage-failure">{trouble}</small>}</th>
          <td data-label="Page views">{number.format(kindTotal(value, "page_view"))}</td>
          <td data-label="Active accounts">{number.format(value.accounts.active)}</td>
          <td data-label="Storage">{formatBytes(value.storage.total)}</td>
          <td><a href={`/usage?deployment=${encodeURIComponent(value.deploymentId)}`} aria-label={`View usage for ${app.name}`}>View usage</a></td></tr>;
      })}</tbody></table></div>
    {drafts.length > 0 && <p className="usage-drafts">{drafts.length === 1 ? `${drafts[0].name} is a draft and has` : `${drafts.length} drafts have`} no usage until deployed: {drafts.map((app, index) => <span key={app.projectId}>{index > 0 && ", "}<a href={projectHref(app.projectId)}>{app.name}</a></span>)}.</p>}
  </section></>;
}

/** The published apps added up; the table under it splits them by app. */
function OverviewCharts({ apps }: { apps: UsageApp[] }) {
  const published = apps.flatMap((app) => app.summary ? [{ app, summary: app.summary }] : []);
  if (!published.length) return null;
  const tiles = [
    ["Published apps", number.format(published.length)],
    ["Page views", number.format(published.reduce((sum, { summary }) => sum + kindTotal(summary, "page_view"), 0))],
    ["Active accounts", number.format(published.reduce((sum, { summary }) => sum + summary.accounts.active, 0))],
    ["Storage now", formatBytes(published.reduce((sum, { summary }) => sum + summary.storage.total, 0))],
  ] as const;
  return <section aria-labelledby="usage-overview-heading"><h2 id="usage-overview-heading">All apps</h2>
    <dl className="ui-stats">{tiles.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
  </section>;
}

function AppUsage({ summary }: { summary: UsageSummary }) {
  const rows = featureRows(summary);
  const coverage = summary.coverage;
  const pruned = coverage.prunedThrough !== null && coverage.prunedThrough !== undefined && coverage.prunedThrough >= Date.parse(`${summary.usage.from}T00:00:00Z`);
  const incomplete = pruned || coverage.queueFlushed === false || coverage.installationDroppedSinceRestart > 0 || coverage.installationWriteErrorsSinceRestart > 0;
  const outcomes = executionTotals(summary.metrics);
  const notices = noticeOutcomes(summary);
  const scheduled = summary.actions.some((a) => a.schedule) || (outcomes.unchanged ?? 0) > 0;
  const strip = [["ok", "Correct"], ["refused", "Refused by app rules"], ["failed", "Failed"], ...(scheduled ? [["unchanged", "Scheduled runs with no changes"]] : [])];
  const signalNames = [...new Set([...summary.actions.flatMap((a) => a.signal ? [a.signal.name] : []), ...summary.metrics.filter((m) => m.feature === "signal").map((m) => m.name)])].sort();
  const actionNames = [...new Set([...summary.actions.filter((a) => a.kind !== "call").map((a) => a.name), ...summary.metrics.filter((m) => m.feature === "execution").map((m) => m.name)])].sort();
  return <>
    <div className="usage-app-link"><a className="usage-address" href={summary.url} target="_blank" rel="noopener">{addressOf(summary.url)} <ArrowUpRight size={14} aria-hidden="true" /></a><a href={projectHref(summary.projectId)}>Open project</a></div>
    {notices.notSent > 0 && <Alert tone="warning" className="usage-error" title={`${number.format(notices.notSent)} email ${notices.notSent === 1 ? "notice was" : "notices were"} not sent`}>{[...new Set(notices.reasons.map((r) => r.label))].map((label) => label[0].toUpperCase() + label.slice(1)).join(". ")}. The actions themselves completed; only the email is missing.{notices.reasons.some((r) => r.reason === "no-transport") && " Whoever runs this installation can connect an email service."}</Alert>}
    {/* Only what makes this period's numbers incomplete stays in view; how they are counted waits in the disclosure. */}
    {incomplete && <section className="usage-coverage" aria-label="Data coverage">
      {pruned && <p>Retention has removed some records through {new Date(coverage.prunedThrough ?? 0).toISOString().slice(0, 10)}. This period is incomplete.</p>}
      {coverage.queueFlushed === false && <p role="status">The capture queue did not finish writing before this read. Recent activity may be missing. Refresh to try again.</p>}
      {(coverage.installationDroppedSinceRestart > 0 || coverage.installationWriteErrorsSinceRestart > 0) && <p role="status">This installation has dropped {number.format(coverage.installationDroppedSinceRestart)} entries and encountered {number.format(coverage.installationWriteErrorsSinceRestart)} write errors since restart. These totals cover the whole installation and cannot be attributed to this app.</p>}
    </section>}
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
    <section aria-labelledby="usage-events-heading"><h2 id="usage-events-heading">Business events</h2><p className="usage-note">Events the app declares, as it recorded them. Completed mutations emit an event only when they change a row. Daily totals outlive the retained event detail.</p>
      {signalNames.length ? <ul className="usage-signals">{signalNames.map((name) => <li key={name}><span>{name}</span><strong>{number.format(summary.metrics.filter((m) => m.feature === "signal" && m.name === name).reduce((n, m) => n + m.count, 0))}</strong><span>recorded in instrumented history</span></li>)}</ul> : <p>No business events declared or recorded.</p>}
    </section>
    <section aria-labelledby="usage-delivery-heading"><h2 id="usage-delivery-heading">Delivery status</h2><p className="usage-note">Current state of retained jobs created in the selected period. Older job records may have been removed. Queued calls and accepted email are not confirmed deliveries.</p>
      <dl className="usage-delivery"><div><dt>Service calls</dt><dd>{summary.calls.states.map((s) => `${number.format(s.count)} ${s.state}`).join(" · ") || "No retained calls in this period"}</dd></div><div><dt>Email notices</dt><dd>{summary.mail.states.map((s) => `${number.format(s.count)} ${s.state}`).join(" · ") || (notices.notSent ? `No email jobs. ${number.format(notices.notSent)} ${notices.notSent === 1 ? "notice was" : "notices were"} not sent, so nothing was queued.` : "No retained notices in this period")}</dd></div></dl>
    </section>
  </>;
}

/** The rules behind one app's numbers, read on demand under "How these are counted". */
function AppCounting({ summary }: { summary: UsageSummary }) {
  const coverage = summary.coverage;
  return <>
    <p>{coverage.startedAt ? `Execution outcomes and business event totals have been collected since ${formatTime(coverage.startedAt)}.` : "No execution outcome history yet. It starts with the next operation."} Earlier operation counts may still appear. Up to {coverage.retentionDays} days are kept, subject to storage limits, so a blank history does not prove a feature was never used.</p>
    <p>A page view counts when a browser opens one of the app's pages, by the file it served. Scripts, crawlers and prefetches are not counted. A single-page app counts its first load, not the screens it shows after that.</p>
    <p>App opens count session checks made while signed in. Actions count successful calls, including service calls accepted into the queue. An owner who entered as an account counts nothing.{summary.accounts.startedAt ? ` Accounts are counted since ${formatTime(summary.accounts.startedAt)}.` : ""}</p>
    <p>Storage is what the app takes on this installation now, whatever the period. An export carries all of it except backups. Uploaded files count what their index records, wherever the bytes are kept.</p>
  </>;
}

function Traffic({ summary }: { summary: UsageSummary }) {
  const pages = pageViews(summary);
  const tiles = [["Page views", kindTotal(summary, "page_view")], ["Sessions opened", kindTotal(summary, "session")], ["Signups", kindTotal(summary, "signup")], ["Sign-ins", kindTotal(summary, "login")]] as const;
  return <section aria-labelledby="usage-traffic-heading"><h2 id="usage-traffic-heading">Traffic</h2>
    <dl className="ui-stats">{tiles.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{number.format(value)}</dd></div>)}</dl>
    <DailyChart title="Page views per day" unit="page views" points={dailyUsage(summary, "page_view")} empty="No page views recorded in this period. Views count from the first browser visit after this installation was updated." />
    {pages.length > 1 && <div className="ui-table-scroll"><table className="ui-table"><caption>Page views by page in this period.</caption><thead><tr><th scope="col">Page</th><th scope="col">Views</th></tr></thead>
      <tbody>{pages.map((row) => <tr key={row.page}><th scope="row">{row.page}</th><td data-label="Views">{number.format(row.count)}</td></tr>)}</tbody></table></div>}
  </section>;
}

function People({ summary }: { summary: UsageSummary }) {
  const [sort, setSort] = useState<"actions" | "opens">("actions");
  const accounts = summary.accounts;
  const heading = <h2 id="usage-people-heading">People</h2>;
  const days = dailyActiveAccounts(summary);
  const name = (row: { email: string | null; account: string }) => row.email ?? `Account removed (${row.account.slice(0, 8)})`;
  const tiles = [["Active accounts", accounts.active], ["App opens", accounts.opens], ["Actions", accounts.actions]] as const;
  return <section aria-labelledby="usage-people-heading">{heading}
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
  const rows = storageRows(storage);
  const backups = storage.parts.find((part) => part.part === "backups");
  return <section aria-labelledby="usage-storage-heading">{heading}
    <div className="usage-chart-card"><BarList title="Storage by part" unit="bytes" labelHeading="Part" format={formatBytes} total={storage.total}
      rows={rows.map((row) => ({ key: row.part, label: row.name, value: row.bytes, tone: row.tone,
        detail: [row.part === "files" ? `${number.format(storage.files)} ${storage.files === 1 ? "file" : "files"}` : storageParts[row.part]?.description, row.limit ? `${row.bytes > 0 && (row.share ?? 0) < 0.01 ? "Under 1%" : `${Math.round((row.share ?? 0) * 100)}%`} of the ${formatBytes(row.limit)} limit` : null].filter(Boolean).join(" · ") }))}
      empty="The app stores nothing yet." /></div>
    {backups && backups.bytes > 0 && <p className="usage-note">Backups on this installation take {formatBytes(backups.bytes)} more. They stay here.</p>}
  </section>;
}
