import { readFile } from "node:fs/promises";
import type { McpClient } from "../src/mcp/client";
import { usageRange, type UsageSummary } from "../src/lib/usage";
import { whatHappened, type Call, type Notice, type Signal, type Sources } from "../src/lib/what-happened";

type FunctionTrace = { at: number; traceId: string; spanId: string; runnerKind: "deno" | "wasm"; status: number; outcome: string; truncated: boolean; spans: { parent: number | null; name: string; startUs: number; durationUs: number; action?: string | null; origin?: string | null; sequence?: number | null; rowsRead?: number | null; rowsWritten?: number | null; replyBytes?: number | null; sqliteUs?: number | null; errorCode?: string | null }[] };

export type AppEvent = {
  id: string;
  at: number;
  kind: "mail" | "notice" | "call" | "scheduled" | "route" | "signal" | "function";
  title: string;
  detail?: string;
  outcome: string;
  fix?: string;
  mail?: { to: string; purpose: string; action?: string; subject?: string; text?: string; link?: string };
  waterfall?: FunctionTrace["spans"];
};

type MailboxLine = { to?: string; purpose?: string; scope?: string; action?: string; subject?: string; text?: string; link?: string; expiresAt?: number };

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

export type ReadRecord = (range: { from: string; to: string }) => Promise<Sources & { functionTraces?: FunctionTrace[] }>;
export type LocalTestControls = {
  status(): Promise<unknown>;
  calls(): Promise<unknown>;
  reply(callId: string, body: string): Promise<unknown>;
  schedule(action: string): Promise<unknown>;
};

async function boundedBody(request: Request, maxBytes: number) {
  const reader = request.body?.getReader();
  if (!reader) return "";
  const chunks: Uint8Array[] = []; let length = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > maxBytes) { await reader.cancel(); throw Error(`Request body exceeds ${maxBytes} bytes.`); }
    chunks.push(value);
  }
  const bytes = new Uint8Array(length); let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
}

export function deploymentRecord(client: Pick<McpClient, "call">, deploymentId: string): ReadRecord {
  return async (range) => {
    const [summary, calls, notices, signals] = await Promise.all([
      client.call<UsageSummary>("usage_summary", { projectId: deploymentId, ...range }),
      client.call<{ calls: Call[] }>("calls_read", { projectId: deploymentId }),
      client.call<{ notices: Notice[] }>("notices_read", { projectId: deploymentId }),
      client.call<{ signals: Signal[] }>("signals_read", { projectId: deploymentId }),
    ]);
    return { summary, calls: calls.calls, notices: notices.notices, signals: signals.signals };
  };
}

export async function kernelEvents(read: ReadRecord, days = 7, mailboxShowsDelivered = false): Promise<AppEvent[]> {
  const record = await read(usageRange(days));
  const notices = mailboxShowsDelivered ? record.notices.filter((notice) => notice.state !== "delivered") : record.notices;
  const events = whatHappened({ ...record, notices }).map((entry) => ({
    id: entry.id, at: entry.at, kind: entry.kind, title: entry.title, detail: entry.detail, outcome: entry.outcome.words,
    ...(entry.outcome.fix ? { fix: entry.outcome.fix.what } : {}),
  }));
  for (const trace of record.functionTraces ?? []) {
    const actions = trace.spans.filter((span) => span.name === "konae.action").length;
    events.push({ id: `function:${trace.traceId}:${trace.spanId}`, at: trace.at, kind: "function",
      title: `Function ${trace.runnerKind} · HTTP ${trace.status} · ${actions} actions`, detail: `Trace ${trace.traceId}`,
      outcome: `${trace.outcome}${trace.truncated ? " · waterfall truncated" : ""}`, waterfall: trace.spans });
  }
  return events;
}

export async function readEvents(read: ReadRecord, deploymentId: string, options: { mailbox?: string; days?: number; seen?: Map<number, number> } = {}): Promise<AppEvent[]> {
  const mailbox = options.mailbox ? await readFile(options.mailbox, "utf8").catch(() => "") : "";
  const events = [...await kernelEvents(read, options.days, options.mailbox !== undefined), ...mailEvents(mailbox, deploymentId, options.seen)];
  return events.sort((a, b) => a.at - b.at);
}

export function formatEvent(event: AppEvent): string {
  const lines = [`${new Date(event.at).toISOString().slice(11, 19)}  ${event.kind.padEnd(9)} ${event.title}${event.detail ? `  (${event.detail})` : ""}`, `          ${event.outcome}${event.fix ? ` ${event.fix}` : ""}`];
  if (event.waterfall) for (const span of event.waterfall) lines.push(`          ${span.startUs} µs +${span.durationUs} µs ${span.name}${span.action ? ` ${span.action}` : ""}${span.errorCode ? ` ${span.errorCode}` : ""}`);
  if (event.mail?.text) lines.push(...event.mail.text.trim().split("\n").map((line) => `          | ${line}`));
  return lines.join("\n");
}

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
  textarea { width: 100%; min-height: 7rem; font: .85rem/1.5 ui-monospace, monospace; }
  input, textarea { color: var(--ink); background: var(--card); border: 1px solid var(--line); border-radius: .3rem; padding: .45rem; }
  .controls { margin: 1.5rem 0; padding: 1rem; border: 1px solid var(--line); border-radius: .6rem; }
</style>
<main>
  <h1>What this app did</h1>
  <p class="lede">Email, calls to other services, scheduled runs, routes and Signals of the app on this machine, newest first. Email is written to a file here and never sent.</p>
  <div class="filters" id="filters" role="group" aria-label="Show"></div>
  <section class="controls" id="test-controls" hidden aria-labelledby="test-heading">
    <h2 id="test-heading">Local test controls</h2><p id="test-policy" class="detail"></p><p id="reset-state" class="detail"></p>
    <h3>Paused schedules</h3><pre id="schedules"></pre>
    <form id="schedule-form"><label>Run a schedule now <input name="action" required></label> <button>Run</button></form>
    <h3>Awaiting calls</h3><div id="calls"></div>
  </section>
  <p class="error" id="error" role="alert" hidden></p>
  <ol id="list" aria-live="polite"></ol>
  <p class="empty" id="empty" hidden>Nothing yet. Call an action that notifies, calls another service or records a Signal, and it shows here.</p>
</main>
<script>
  const kinds = ["all", "mail", "notice", "call", "scheduled", "route", "signal", "function"];
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
      if (event.mail && event.mail.link) {
        const p = el("p", "detail");
        try { const link = new URL(event.mail.link); if (link.protocol === "http:" || link.protocol === "https:") { const a = el("a", "", event.mail.link); a.href = link.href; a.rel = "noreferrer noopener"; p.append(a); } }
        catch {}
        if (p.childNodes.length) item.append(p);
      }
      if (event.mail && event.mail.text) item.append(el("pre", "", event.mail.text.trim()));
      if (event.waterfall) {
        const disclosure = el("details"); disclosure.append(el("summary", "", "Invocation waterfall"));
        const spans = el("ol");
        for (const span of event.waterfall) spans.append(el("li", "", span.startUs + " µs +" + span.durationUs + " µs " + span.name + (span.action ? " · " + span.action : "") + (span.errorCode ? " · " + span.errorCode : "")));
        disclosure.append(spans); item.append(disclosure);
      }
      return item;
    }));
    document.getElementById("empty").hidden = rows.length > 0;
  }
  const token = new URLSearchParams(location.hash.slice(1)).get("control");
  async function authorized(url, options = {}) {
    return fetch(url, { ...options, headers: { ...(options.headers || {}), "X-Konae-Control": token || "" } });
  }
  async function load() {
    const error = document.getElementById("error");
    try {
      const response = await authorized("events.json");
      if (!response.ok) throw Error((await response.json()).error || "The local Kernel did not answer.");
      events = (await response.json()).reverse(); error.hidden = true; draw();
    } catch (problem) { error.textContent = "Could not read the events: " + problem.message; error.hidden = false; }
  }
  const controls = document.getElementById("test-controls");
  async function loadTestControls() {
    if (!token) return;
    try {
      const [statusResponse, callsResponse] = await Promise.all([authorized("test/status"), authorized("test/calls")]);
      if (!statusResponse.ok || !callsResponse.ok) throw Error("The local test controls refused this request.");
      const status = await statusResponse.json(); const calls = await callsResponse.json();
      controls.hidden = false;
      document.getElementById("test-policy").textContent = "Execution mode: " + status.mode;
      const reset = status.localState;
      document.getElementById("reset-state").textContent = reset
        ? "Last reset " + new Date(reset.resetAt).toLocaleString() + ". Snapshot " + reset.snapshotId + ", execution " + reset.executionId + "; retired " + reset.callsRetired + " calls and " + reset.mailRetired + " emails."
        : "No local reset record.";
      document.getElementById("schedules").textContent = JSON.stringify(status.pausedSchedules || [], null, 2);
      const host = document.getElementById("calls"); host.replaceChildren();
      for (const call of calls.calls || []) {
        const box = el("article"); box.append(el("h4", "", call.action + " · " + call.callId));
        box.append(el("pre", "", JSON.stringify(call, null, 2)));
        const form = el("form"); const label = el("label", "", "Synthetic response JSON");
        const field = el("textarea"); field.required = true; field.value = JSON.stringify({ responseNumber: call.responseNumber, status: 200, body: {} }, null, 2);
        label.append(field); const button = el("button", "", "Reply"); form.append(label, button);
        form.onsubmit = async (event) => { event.preventDefault(); try { await mutate("test/reply/" + encodeURIComponent(call.callId), field.value); await loadTestControls(); } catch (problem) { const error = document.getElementById("error"); error.textContent = problem.message; error.hidden = false; } };
        box.append(form); host.append(box);
      }
    } catch (problem) { const error = document.getElementById("error"); error.textContent = problem.message; error.hidden = false; }
  }
  async function mutate(url, body) {
    const response = await authorized(url, { method: "POST", headers: { "content-type": "application/json" }, body });
    const result = await response.json(); if (!response.ok) throw Error(result.error || "The local test control refused this request.");
    if (url.startsWith("test/reply/") && result.accepted === false) throw Error("The Kernel did not accept this response. Refresh the call list before replying again.");
    return result;
  }
  document.getElementById("schedule-form").onsubmit = async (event) => { event.preventDefault(); const action = new FormData(event.currentTarget).get("action"); try { await mutate("test/schedule/" + encodeURIComponent(action), "{}"); await loadTestControls(); } catch (problem) { const error = document.getElementById("error"); error.textContent = problem.message; error.hidden = false; } };
  load(); loadTestControls(); setInterval(() => { load(); loadTestControls(); }, 2000);
</script>
</html>
`;

export function serveEvents(read: ReadRecord, deploymentId: string, mailbox: string, port: number, controls?: LocalTestControls) {
  const seen = new Map<number, number>();
  const stamp = async () => { mailEvents(await readFile(mailbox, "utf8").catch(() => ""), deploymentId, seen); };
  const timer = setInterval(() => void stamp(), 1000);
  const token = crypto.randomUUID();
  let origin = "";
  const server = Bun.serve({
    port, hostname: "127.0.0.1",
    async fetch(request) {
      const url = new URL(request.url);
      const headers = { "cache-control": "no-store", "referrer-policy": "no-referrer", "x-content-type-options": "nosniff", "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'" };
      const host = request.headers.get("host");
      const expectedHost = `localhost:${server.port}`;
      if (host !== expectedHost) return Response.json({ error: "Host refused." }, { status: 403, headers });
      const requestOrigin = request.headers.get("origin");
      if ((requestOrigin && requestOrigin !== origin) || (request.method !== "GET" && requestOrigin !== origin)) return Response.json({ error: "Origin refused." }, { status: 403, headers });
      if (url.pathname === "/") return new Response(EVENTS_PAGE, { headers: { ...headers, "content-type": "text/html; charset=utf-8" } });
      if (request.headers.get("x-konae-control") !== token) return Response.json({ error: "Local owner token required." }, { status: 403, headers });
      if (url.pathname === "/events.json" && request.method === "GET") {
        const days = Number(url.searchParams.get("days") ?? 7);
        try {
          return Response.json(await readEvents(read, deploymentId, { mailbox, seen, days: Number.isInteger(days) && days >= 1 && days <= 400 ? days : 7 }), { headers });
        } catch (error) { return Response.json({ error: error instanceof Error ? error.message : String(error) }, { status: 502, headers }); }
      }
      if (controls && request.method === "GET" && ["/test/status", "/test/calls"].includes(url.pathname)) {
        try { return Response.json(url.pathname.endsWith("status") ? await controls.status() : await controls.calls(), { headers }); }
        catch (error) { return Response.json({ error: error instanceof Error ? error.message : String(error) }, { status: 502, headers }); }
      }
      const match = /^\/test\/(reply|schedule)\/([^/]+)$/.exec(url.pathname);
      if (controls && match && request.method === "POST") {
        if (request.headers.get("content-type")?.split(";")[0].trim().toLowerCase() !== "application/json") return Response.json({ error: "Content-Type must be application/json." }, { status: 415, headers });
        let body: string;
        try { body = await boundedBody(request, 65536); }
        catch (error) { return Response.json({ error: error instanceof Error ? error.message : String(error) }, { status: 413, headers }); }
        try {
          if (match[1] === "schedule") {
            const value = JSON.parse(body);
            if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).length) throw Error("Schedule request must be an empty JSON object.");
          }
          const result = match[1] === "reply" ? await controls.reply(decodeURIComponent(match[2]), body) : await controls.schedule(decodeURIComponent(match[2]));
          return Response.json(result, { headers });
        } catch (error) { return Response.json({ error: error instanceof Error ? error.message : String(error) }, { status: 400, headers }); }
      }
      if (url.pathname.startsWith("/test/")) return Response.json({ error: controls ? "Method refused." : "Local test controls are unavailable." }, { status: controls ? 405 : 404, headers });
      return new Response("Not found", { status: 404, headers });
    },
  });
  origin = `http://localhost:${server.port}`;
  return { url: `${origin}/#control=${token}`, origin, token, stop: () => { clearInterval(timer); server.stop(true); } };
}
