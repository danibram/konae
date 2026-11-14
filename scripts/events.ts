// `konae events`: what an app did besides answer its page. Email, calls
// to other services, scheduled runs, routes and Signals, newest last, from
// the local app, through the kernel binary's commands, or, with --prod, from the published
// Deployment. The entries are the editor's What happened list
// (src/lib/what-happened.ts), so a refusal reads the same in both places.
import { readFile } from "node:fs/promises";
import type { McpClient } from "../src/mcp/client";
import { usageRange, type UsageSummary } from "../src/lib/usage";
import { whatHappened, type Call, type Notice, type Signal, type Sources } from "../src/lib/what-happened";

/** One line of the feed. `mail` exists only for the local app, whose mailbox is a file that names the recipient and holds the text. */
export type AppEvent = {
  id: string;
  at: number;
  kind: "mail" | "notice" | "call" | "scheduled" | "route" | "signal";
  title: string;
  detail?: string;
  outcome: string;
  /** Who must act and what to do, when someone must. */
  fix?: string;
  /** A mail's recipient, purpose, subject, text and link, as the development mailbox wrote them. */
  mail?: { to: string; purpose: string; action?: string; subject?: string; text?: string; link?: string };
};

type MailboxLine = { to?: string; purpose?: string; scope?: string; action?: string; subject?: string; text?: string; link?: string; expiresAt?: number };

/**
 * The development mailbox's lines for one Deployment. A line carries no
 * time, so `at` is the time of this read for a line not seen before, and
 * `seen` keeps the first time each line was read during a --follow.
 */
export function mailEvents(mailbox: string, deploymentId: string, seen = new Map<number, number>(), now = Date.now()): AppEvent[] {
  const events: AppEvent[] = [];
  mailbox.split("\n").forEach((text, index) => {
    if (!text.trim()) return;
    let line: MailboxLine;
    try { line = JSON.parse(text); } catch { return; }
    if (line.scope !== `deployment:${deploymentId}` || !line.to || !line.purpose) return;
    if (!seen.has(index)) seen.set(index, now);
    events.push({
      id: `mail:${index}`, at: seen.get(index)!, kind: "mail",
      title: line.action ? `Email from ${line.action} to ${line.to}` : `Account email (${line.purpose}) to ${line.to}`,
      detail: line.subject ?? line.link,
      outcome: "Written to the development mailbox. Nothing was sent.",
      mail: { to: line.to, purpose: line.purpose, action: line.action, subject: line.subject, text: line.text, link: line.link },
    });
  });
  return events;
}

/** The Kernel's own record of a range of days: Usage, calls, notices and Signals. */
export type ReadRecord = (range: { from: string; to: string }) => Promise<Sources>;

/** The published Deployment's record, read with the four owner tools at once. */
export function deploymentRecord(client: Pick<McpClient, "call">, deploymentId: string): ReadRecord {
  return async (range) => {
    const [summary, calls, notices, signals] = await Promise.all([
      client.call<UsageSummary>("usage_summary", { deploymentId, ...range }),
      client.call<{ calls: Call[] }>("calls_read", { deploymentId }),
      client.call<{ notices: Notice[] }>("notices_read", { deploymentId }),
      client.call<{ signals: Signal[] }>("signals_read", { deploymentId }),
    ]);
    return { summary, calls: calls.calls, notices: notices.notices, signals: signals.signals };
  };
}

/** The Kernel's own record: notices, calls, scheduled runs, routes and Signals of the last `days` days. */
export async function kernelEvents(read: ReadRecord, days = 7, mailboxShowsDelivered = false): Promise<AppEvent[]> {
  const record = await read(usageRange(days));
  // Locally a delivered notice is a line of the mailbox, which names its recipient and text, so it is listed once, as mail.
  const notices = mailboxShowsDelivered ? record.notices.filter((notice) => notice.state !== "delivered") : record.notices;
  return whatHappened({ ...record, notices }).map((entry) => ({
    id: entry.id, at: entry.at, kind: entry.kind, title: entry.title, detail: entry.detail, outcome: entry.outcome.words,
    ...(entry.outcome.fix ? { fix: entry.outcome.fix.what } : {}),
  }));
}

/** Everything, oldest first, so a terminal and a --follow read downwards. */
export async function readEvents(read: ReadRecord, deploymentId: string, options: { mailbox?: string; days?: number; seen?: Map<number, number> } = {}): Promise<AppEvent[]> {
  const mailbox = options.mailbox ? await readFile(options.mailbox, "utf8").catch(() => "") : "";
  const events = [...await kernelEvents(read, options.days, options.mailbox !== undefined), ...mailEvents(mailbox, deploymentId, options.seen)];
  return events.sort((a, b) => a.at - b.at);
}

/** One event as terminal lines: time, kind and title, then what happened, then a mail's text. */
export function formatEvent(event: AppEvent): string {
  const lines = [`${new Date(event.at).toISOString().slice(11, 19)}  ${event.kind.padEnd(9)} ${event.title}${event.detail ? `  (${event.detail})` : ""}`, `          ${event.outcome}${event.fix ? ` ${event.fix}` : ""}`];
  if (event.mail?.text) lines.push(...event.mail.text.trim().split("\n").map((line) => `          | ${line}`));
  return lines.join("\n");
}

/**
 * The page `dev` serves beside the app: the same events, newest first, read
 * again every two seconds. It is one file with no dependency, and it writes
 * every value as text, never as markup, because a mail's text comes from the app's input.
 */
export const EVENTS_PAGE = `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>What this app did</title>
<style>
  :root { color-scheme: light dark; --ink: #1c1b19; --soft: #6b6862; --line: #e4e1da; --paper: #fbfaf7; --card: #fff; --accent: #2f5d50; --warn: #9a5b00; }
  @media (prefers-color-scheme: dark) { :root { --ink: #ecebe6; --soft: #a09c93; --line: #34322e; --paper: #171614; --card: #1f1e1b; --accent: #8fc7b4; --warn: #e2a84a; } }
  * { box-sizing: border-box; }
  body { margin: 0; font: 15px/1.5 system-ui, sans-serif; color: var(--ink); background: var(--paper); }
  main { max-width: 52rem; margin: 0 auto; padding: 2rem 1.25rem 4rem; }
  h1 { font-size: 1.4rem; margin: 0 0 .25rem; }
  .lede { color: var(--soft); margin: 0 0 1.25rem; }
  .filters { display: flex; flex-wrap: wrap; gap: .4rem; margin-bottom: 1rem; }
  .filters button { font: inherit; padding: .2rem .7rem; border: 1px solid var(--line); border-radius: 999px; background: var(--card); color: var(--ink); cursor: pointer; }
  .filters button[aria-pressed="true"] { background: var(--accent); border-color: var(--accent); color: var(--paper); }
  .filters button:focus-visible, a:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
  ol { list-style: none; margin: 0; padding: 0; display: grid; gap: .6rem; }
  li { background: var(--card); border: 1px solid var(--line); border-radius: .6rem; padding: .7rem .9rem; }
  .head { display: flex; gap: .6rem; align-items: baseline; flex-wrap: wrap; }
  .kind { font: 600 .72rem/1.6 ui-monospace, monospace; text-transform: uppercase; letter-spacing: .04em; color: var(--accent); min-width: 5.2rem; }
  .title { font-weight: 600; overflow-wrap: anywhere; }
  time { margin-left: auto; color: var(--soft); font: .8rem ui-monospace, monospace; }
  .detail, .outcome { margin: .15rem 0 0; color: var(--soft); overflow-wrap: anywhere; }
  .fix { color: var(--warn); }
  pre { margin: .5rem 0 0; padding: .6rem .75rem; background: var(--paper); border: 1px solid var(--line); border-radius: .4rem; white-space: pre-wrap; overflow-wrap: anywhere; font: .85rem/1.5 ui-monospace, monospace; }
  .empty, .error { color: var(--soft); padding: 2rem 0; }
  .error { color: var(--warn); }
</style>
<main>
  <h1>What this app did</h1>
  <p class="lede">Email, calls to other services, scheduled runs, routes and Signals of the app on this machine, newest first. Email is written to a file here and never sent.</p>
  <div class="filters" id="filters" role="group" aria-label="Show"></div>
  <p class="error" id="error" role="alert" hidden></p>
  <ol id="list" aria-live="polite"></ol>
  <p class="empty" id="empty" hidden>Nothing yet. Call an action that notifies, calls another service or records a Signal, and it shows here.</p>
</main>
<script>
  const kinds = ["all", "mail", "notice", "call", "scheduled", "route", "signal"];
  let shown = "all"; let events = [];
  const el = (tag, cls, text) => { const node = document.createElement(tag); if (cls) node.className = cls; if (text !== undefined) node.textContent = text; return node; };
  const filters = document.getElementById("filters");
  for (const kind of kinds) {
    const button = el("button", "", kind === "all" ? "Everything" : kind);
    button.type = "button"; button.setAttribute("aria-pressed", String(kind === shown));
    button.onclick = () => { shown = kind; for (const other of filters.children) other.setAttribute("aria-pressed", String(other === button)); draw(); };
    filters.append(button);
  }
  function draw() {
    const list = document.getElementById("list");
    const rows = events.filter((event) => shown === "all" || event.kind === shown);
    list.replaceChildren(...rows.map((event) => {
      const item = el("li"); const head = el("div", "head");
      const when = el("time", "", new Date(event.at).toLocaleTimeString()); when.dateTime = new Date(event.at).toISOString();
      head.append(el("span", "kind", event.kind), el("span", "title", event.title), when);
      item.append(head);
      if (event.detail && !(event.mail && event.mail.link)) item.append(el("p", "detail", event.detail));
      item.append(el("p", "outcome", event.outcome));
      if (event.fix) item.append(el("p", "outcome fix", event.fix));
      if (event.mail && event.mail.link) { const p = el("p", "detail"); const a = el("a", "", event.mail.link); a.href = event.mail.link; p.append(a); item.append(p); }
      if (event.mail && event.mail.text) item.append(el("pre", "", event.mail.text.trim()));
      return item;
    }));
    document.getElementById("empty").hidden = rows.length > 0;
  }
  async function load() {
    const error = document.getElementById("error");
    try {
      const response = await fetch("events.json");
      if (!response.ok) throw Error((await response.json()).error || "The local Kernel did not answer.");
      events = (await response.json()).reverse(); error.hidden = true; draw();
    } catch (problem) { error.textContent = "Could not read the events: " + problem.message; error.hidden = false; }
  }
  load(); setInterval(load, 2000);
</script>
</html>
`;

/**
 * Serves the page and `events.json` on this machine only. It stamps each
 * mailbox line once a second, so a mail's time is when it was written to
 * within a second, also for a later `konae events`, which reads from here.
 */
export function serveEvents(read: ReadRecord, deploymentId: string, mailbox: string, port: number) {
  const seen = new Map<number, number>();
  const stamp = async () => { mailEvents(await readFile(mailbox, "utf8").catch(() => ""), deploymentId, seen); };
  const timer = setInterval(() => void stamp(), 1000);
  const server = Bun.serve({
    port, hostname: "127.0.0.1",
    async fetch(request) {
      const url = new URL(request.url);
      if (url.pathname === "/") return new Response(EVENTS_PAGE, { headers: { "content-type": "text/html; charset=utf-8", "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'" } });
      if (url.pathname !== "/events.json") return new Response("Not found", { status: 404 });
      const days = Number(url.searchParams.get("days") ?? 7);
      try {
        return Response.json(await readEvents(read, deploymentId, { mailbox, seen, days: Number.isInteger(days) && days >= 1 && days <= 400 ? days : 7 }));
      } catch (error) {
        return Response.json({ error: error instanceof Error ? error.message : String(error) }, { status: 502 });
      }
    },
  });
  return { url: `http://localhost:${server.port}`, stop: () => { clearInterval(timer); server.stop(true); } };
}
