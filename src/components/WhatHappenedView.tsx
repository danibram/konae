import { platformAccessHref } from "./owner-session";
import { Activity, Bell, CalendarClock, Mail, RefreshCw, Send, Webhook } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { actorWords, clock, dayOf, needsAction, readWhatHappened, whatHappened, type Entry, type Kind, type Place, type Read } from "../lib/what-happened";
import { message, signedOut } from "./useConfirm";
import { Badge, Button, EmptyState, LoadingBlock, Select } from "./ui";
import "./what-happened.css";

type Props = {
  /** The published app. The preview sends no email, calls nothing and runs no schedule. */
  deployment?: { id: string; url: string };
  onOpen?: (place: Place) => void;
};

const icons: Record<Kind, ReactNode> = {
  notice: <Mail size={15} />,
  call: <Send size={15} />,
  scheduled: <CalendarClock size={15} />,
  route: <Webhook size={15} />,
  signal: <Bell size={15} />,
};

const badges = { success: "Done", warning: "Check", danger: "Needs action", neutral: "Noted" } as const;

const places: Record<Place, string> = {
  integrations: "Open Access > Integrations",
  users: "Open Access > Users",
  capsule: "Open actions.json",
};

/** What left or ran on the published app, newest first, each with its outcome and who acts when it failed. */
export function WhatHappenedView({ deployment, onOpen }: Props) {
  const [days, setDays] = useState(7);
  const [read, setRead] = useState<Read | null>(null);
  const [error, setError] = useState("");
  const [owner, setOwner] = useState(true);
  const [loading, setLoading] = useState(false);
  const [only, setOnly] = useState<"all" | "attention">("all");
  const [reload, setReload] = useState(0);
  const refresh = useCallback(() => setReload((count) => count + 1), []);

  useEffect(() => {
    if (!deployment) return;
    const controller = new AbortController();
    setLoading(true);
    readWhatHappened(deployment.id, days, controller.signal).then((answer) => {
      setRead(answer);
      setError("");
      setOwner(true);
    }, (cause) => {
      if (controller.signal.aborted) return;
      if (signedOut(cause)) setOwner(false);
      setError(message(cause));
    }).finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [deployment, days, reload]);

  if (!deployment) {
    return (
      <section className="cap-workspace happened" aria-label="What happened">
        <EmptyState
          className="happened-empty"
          icon={<Activity size={26} />}
          title="Nothing has run yet"
          description="The preview sends no email, calls no other service and runs no schedule. Once the app is deployed, what it sends and runs shows here."
        />
      </section>
    );
  }

  const entries = read ? whatHappened(read.sources) : [];
  const shown = only === "attention" ? entries.filter(needsAction) : entries;
  const attention = entries.filter(needsAction).length;

  return (
    <section className="cap-workspace happened" aria-label="What happened">
      <div className="happened-scroll">
        <header className="happened-head">
          <div>
            <h2>What happened</h2>
            <p>Emails, calls to other services, scheduled runs, requests from other services and Signals of <a href={deployment.url} target="_blank" rel="noreferrer">{deployment.url.replace(/^https?:\/\//, "")}</a>, newest first.</p>
          </div>
          <div className="happened-controls">
            <Select aria-label="Period" value={days} onChange={(event) => setDays(Number(event.target.value))}>
              <option value={1}>Today</option>
              <option value={7}>Last 7 days</option>
              <option value={30}>Last 30 days</option>
            </Select>
            <Select aria-label="Show" value={only} onChange={(event) => setOnly(event.target.value === "attention" ? "attention" : "all")}>
              <option value="all">Everything</option>
              <option value="attention">Needs someone to act</option>
            </Select>
            <Button size="sm" loading={loading} onClick={refresh}><RefreshCw size={13} /> Refresh</Button>
          </div>
        </header>
        {!owner ? <p className="happened-note">Sign in with <a href={platformAccessHref(window.location.pathname)}>owner sign-in</a> to read what this app sent and ran.</p>
          : error ? <p className="happened-error" role="alert">{error}</p> : null}
        {!read && !error && owner && loading && <LoadingBlock label="Reading what happened" rows={4} />}
        {read && attention > 0 && <p className="happened-summary" role="status">{attention} {attention === 1 ? "thing needs" : "things need"} someone to act.</p>}
        {read && shown.length === 0 && (
          <EmptyState
            className="happened-empty"
            icon={<Activity size={26} />}
            title={only === "attention" ? "Nothing needs anyone to act" : "Nothing left or ran in this period"}
            description="Emails the app sends, calls it makes, scheduled runs and Signals show here once they happen on the live app."
          />
        )}
        {groups(shown).map(([day, rows]) => (
          <section key={day} className="happened-day" aria-label={day}>
            <h3>{dayLabel(day)}</h3>
            <ol className="happened-list">{rows.map((entry) => <Row key={entry.id} entry={entry} onOpen={onOpen} />)}</ol>
          </section>
        ))}
        {read && <details className="happened-note">
          <summary>How this list is counted</summary>
          <p>Days and times are UTC. It shows the newest 50 emails, 50 calls, 100 Signals and 500 scheduled runs. Requests from other services, and emails that were never queued, are counted per day once that day ends.</p>
        </details>}
      </div>
    </section>
  );
}

function groups(entries: Entry[]): [string, Entry[]][] {
  const byDay = new Map<string, Entry[]>();
  for (const entry of entries) byDay.set(dayOf(entry), [...(byDay.get(dayOf(entry)) ?? []), entry]);
  return [...byDay];
}

function dayLabel(day: string) {
  const today = new Date().toISOString().slice(0, 10);
  const date = new Date(`${day}T00:00:00Z`);
  const words = new Intl.DateTimeFormat("en", { weekday: "long", day: "numeric", month: "long", timeZone: "UTC" }).format(date);
  return day === today ? `Today, ${words}` : words;
}

function Row({ entry, onOpen }: { entry: Entry; onOpen?: (place: Place) => void }) {
  const { outcome } = entry;
  const fix = outcome.fix;
  return (
    <li className={`happened-row is-${outcome.tone}`}>
      <span className="happened-icon" aria-hidden="true">{icons[entry.kind]}</span>
      <div className="happened-body">
        <div className="happened-title">
          <strong>{entry.title}</strong>
          <Badge tone={outcome.tone}>{badges[outcome.tone]}</Badge>
          <time dateTime={entry.daily ? entry.day : new Date(entry.at).toISOString()}>{entry.daily ? entry.detail : clock(entry.at)}</time>
        </div>
        {entry.detail && !entry.daily && <code className="happened-detail">{entry.detail}</code>}
        <p>{outcome.words}</p>
        {fix && (
          <p className="happened-fix">
            <span><strong>{actorWords[fix.who]}</strong> {fix.what}</span>
            {fix.where && onOpen && <Button size="sm" variant="ghost" onClick={() => onOpen(fix.where!)}>{places[fix.where]}</Button>}
          </p>
        )}
      </div>
    </li>
  );
}
