import { useEffect, useRef, useState } from "react";
import { Dialog, Button } from "./ui";
import {
  deleteDurableConversation, downloadDurableConversation, listDurableConversations,
  readDurableCheckpointSummary, readDurableDetail, readDurableEffects, readDurableLease, readDurableRecord,
  readDurableRecords, requestDurableStop,
  type DurableConversationCursor, type DurableConversationInfo, type DurableRecordCursor,
  type DurableRecordSummaryItem,
} from "../agent/durable-storage";
import "./durable-history.css";

type Props = { projectId: string; ownerId: string; open: boolean; onClose: () => void; onContinue: (conversationId: string) => void };
type PageState = { items: DurableRecordSummaryItem[]; nextCursor: DurableRecordCursor | null };
type Expanded = { loading: boolean; value?: Record<string, unknown>; error?: string };

export function DurableHistoryDialog({ projectId, ownerId, open, onClose, onContinue }: Props) {
  const [conversations, setConversations] = useState<DurableConversationInfo[]>([]);
  const [conversationCursor, setConversationCursor] = useState<DurableConversationCursor | null>(null);
  const [selected, setSelected] = useState<DurableConversationInfo | null>(null);
  const [records, setRecords] = useState<PageState | null>(null);
  const [lease, setLease] = useState<Awaited<ReturnType<typeof readDurableLease>> | null>(null);
  const [effects, setEffects] = useState<Awaited<ReturnType<typeof readDurableEffects>>>([]);
  const [checkpoint, setCheckpoint] = useState<{ sequence: number; status: string; sourcesRevision?: number; candidateId?: string; inspectionOnly?: boolean } | null>(null);
  const [checkpointError, setCheckpointError] = useState("");
  const [expanded, setExpanded] = useState<Record<string, Expanded>>({});
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [waitingForRelease, setWaitingForRelease] = useState(false);
  const selectionToken = useRef(0);

  useEffect(() => {
    if (!open) return;
    let active = true;
    selectionToken.current += 1;
    setError(""); setSelected(null); setRecords(null); setExpanded({}); setConfirmDelete(false); setCheckpoint(null); setCheckpointError(""); setWaitingForRelease(false);
    void listDurableConversations(projectId, ownerId).then((page) => {
      if (active) { setConversations(page.items); setConversationCursor(page.nextCursor); }
    }).catch((cause) => { if (active) setError(message(cause)); });
    return () => { active = false; selectionToken.current += 1; };
  }, [open, ownerId, projectId]);

  const choose = async (item: DurableConversationInfo) => {
    const token = ++selectionToken.current;
    setSelected(item); setRecords(null); setExpanded({}); setError(""); setCheckpointError(""); setLease(null); setEffects([]); setConfirmDelete(false); setWaitingForRelease(false); setBusy(false);
    try {
      const [page, currentLease, currentEffects, checkpoints] = await Promise.all([
        readDurableRecords(projectId, item.id, ownerId), readDurableLease(projectId, item.id, ownerId), readDurableEffects(projectId, item.id, ownerId), readDurableCheckpointSummary(projectId, item.id, ownerId).then((value) => ({ value, error: "" })).catch((cause) => ({ value: null, error: message(cause) })),
      ]);
      if (token === selectionToken.current) {
        setRecords(page); setLease(currentLease); setEffects(currentEffects);
        setCheckpointError(checkpoints.error);
        const latest = checkpoints.value?.item;
        setCheckpoint(latest ? { sequence: latest.sequence, status: latest.status ?? "saved checkpoint", sourcesRevision: latest.sourcesRevision ?? undefined, candidateId: latest.candidateId ?? undefined, inspectionOnly: latest.inspectionOnly } : null);
      }
    } catch (cause) { if (token === selectionToken.current) setError(message(cause)); }
  };
  useEffect(() => {
    if (!open || !waitingForRelease || !selected) return;
    let active = true;
    const poll = async () => {
      try {
        const current = await readDurableLease(projectId, selected.id, ownerId);
        if (!active) return;
        setLease(current);
        if (!current.live) setWaitingForRelease(false);
      } catch (cause) { if (active) setError(message(cause)); }
    };
    const timer = window.setInterval(() => void poll(), 1500);
    void poll();
    return () => { active = false; window.clearInterval(timer); };
  }, [open, waitingForRelease, selected?.id, ownerId, projectId]);
  const older = async () => {
    if (!conversationCursor || busy) return;
    const token = selectionToken.current;
    const cursor = conversationCursor;
    setBusy(true);
    try { const page = await listDurableConversations(projectId, ownerId, cursor); if (token === selectionToken.current) { setConversations((items) => [...items, ...page.items]); setConversationCursor(page.nextCursor); } }
    catch (cause) { if (token === selectionToken.current) setError(message(cause)); } finally { if (token === selectionToken.current) setBusy(false); }
  };
  const olderRecords = async () => {
    if (!selected || !records?.nextCursor || busy) return;
    const token = selectionToken.current;
    const conversationId = selected.id;
    const cursor = records.nextCursor;
    setBusy(true);
    try { const page = await readDurableRecords(projectId, conversationId, ownerId, cursor); if (token === selectionToken.current && selected?.id === conversationId) setRecords((current) => current ? { items: [...page.items, ...current.items], nextCursor: page.nextCursor } : page); }
    catch (cause) { if (token === selectionToken.current && selected?.id === conversationId) setError(message(cause)); } finally { if (token === selectionToken.current) setBusy(false); }
  };
  const expand = async (item: DurableRecordSummaryItem) => {
    if (item.id === null) return;
    const key = String(item.id);
    if (expanded[key]) { setExpanded((current) => { const next = { ...current }; delete next[key]; return next; }); return; }
    setExpanded((current) => ({ ...current, [key]: { loading: true } }));
    const token = selectionToken.current;
    const conversationId = selected?.id;
    try {
      if (!conversationId) return;
      const raw = await readDurableRecord(projectId, conversationId, item.id, ownerId);
      if (token === selectionToken.current && selected?.id === conversationId) setExpanded((current) => ({ ...current, [key]: { loading: false, value: raw } }));
    } catch (cause) { if (token === selectionToken.current && selected?.id === conversationId) setExpanded((current) => ({ ...current, [key]: { loading: false, error: message(cause) } })); }
  };
  const remove = async () => {
    if (!selected) return;
    const conversationId = selected.id;
    const token = selectionToken.current;
    setBusy(true);
    try {
      await deleteDurableConversation(projectId, conversationId, ownerId);
      const page = await listDurableConversations(projectId, ownerId);
      if (token === selectionToken.current && selected?.id === conversationId) { setConversations(page.items); setConversationCursor(page.nextCursor); setSelected(null); setRecords(null); setConfirmDelete(false); setError(""); }
    } catch (cause) { if (token === selectionToken.current && selected?.id === conversationId) setError(message(cause)); } finally { if (token === selectionToken.current) setBusy(false); }
  };
  const download = async () => {
    if (!selected) return;
    const conversationId = selected.id;
    const token = selectionToken.current;
    setBusy(true);
    try {
      const blob = await downloadDurableConversation(projectId, conversationId, ownerId);
      if (token !== selectionToken.current || selected?.id !== conversationId) return;
      const href = URL.createObjectURL(blob); const anchor = document.createElement("a"); anchor.href = href; anchor.download = `konae-conversation-${selected.id}.json`; anchor.click(); URL.revokeObjectURL(href);
    } catch (cause) { if (token === selectionToken.current && selected?.id === conversationId) setError(message(cause)); } finally { if (token === selectionToken.current) setBusy(false); }
  };
  const stopWriter = async () => {
    if (!selected || !lease?.conversationId || !lease.live) return;
    const conversationId = selected.id;
    const activeConversationId = lease.conversationId;
    const generation = lease.generation;
    const token = selectionToken.current;
    setBusy(true);
    try {
      await requestDurableStop(projectId, activeConversationId, generation, crypto.randomUUID(), ownerId);
      if (token !== selectionToken.current || selected?.id !== conversationId) return;
      setError("Stop requested. Waiting for the active turn to drain and release its lease.");
      setWaitingForRelease(true);
      const refreshed = await readDurableLease(projectId, conversationId, ownerId);
      if (token === selectionToken.current && selected?.id === conversationId) setLease(refreshed);
    } catch (cause) { if (token === selectionToken.current && selected?.id === conversationId) setError(message(cause)); } finally { if (token === selectionToken.current) setBusy(false); }
  };

  return <Dialog open={open} title="Conversation history" description="Read-only history for this saved project. Opening it does not resume work or contact a model provider." onClose={onClose} className="durable-history-dialog">
    <div className="durable-history-layout">
      <aside className="durable-history-list" aria-label="Recent conversations">
        {!conversations.length && <p className="durable-muted">No saved conversations.</p>}
        {conversations.map((item) => <button type="button" key={item.id} className={`durable-conversation-choice${selected?.id === item.id ? " selected" : ""}`} onClick={() => void choose(item)}>
          <strong>{safeText(item.title) || "Untitled conversation"}</strong><span>{new Date(item.updatedAt ?? item.createdAt ?? 0).toLocaleString()}</span><small>Saved through sequence {item.lastSeq}</small>
        </button>)}
        {conversationCursor && <Button variant="secondary" onClick={() => void older()} disabled={busy}>Load older conversations</Button>}
      </aside>
      <section className="durable-history-reader" aria-label="Conversation timeline">
        {!selected ? <p className="durable-muted">Choose a conversation to read its saved messages and call records.</p> : <>
          <header className="durable-reader-head"><div><h3>{safeText(selected.title) || "Untitled conversation"}</h3><p>Last acknowledged save: {new Date(selected.updatedAt ?? 0).toLocaleString()} · sequence {selected.lastSeq}</p></div>
            <div className="durable-reader-actions"><Button variant="secondary" onClick={() => void download()} disabled={busy}>Download JSON</Button><Button variant="secondary" onClick={() => setConfirmDelete(true)} disabled={busy}>Delete</Button></div>
          </header>
          {lease?.live && <div className="durable-status" role="status">Another browser holds the active writer lease (generation {lease.generation}, executor {lease.executorId ?? "unknown"}). <Button variant="secondary" onClick={() => void stopWriter()} disabled={busy}>Ask it to stop and release</Button></div>}
          {lease && !lease.live && lease.unresolvedEffects && <div className="durable-status" role="alert">A write outcome is unresolved. Continue stays blocked until its documented read-back reconciles it.</div>}
          {lease && !lease.live && !lease.unresolvedEffects && <div className="durable-status">No active writer. Saved source changes are checked again when you choose Continue.</div>}
          {checkpoint && <div className="durable-status"><strong>Latest saved proposal checkpoint:</strong> {safeText(checkpoint.status)} · checkpoint sequence {checkpoint.sequence}{checkpoint.sourcesRevision !== undefined ? ` · source revision ${checkpoint.sourcesRevision}` : ""}{checkpoint.candidateId ? ` · candidate ${checkpoint.candidateId}` : ""}{checkpoint.inspectionOnly ? ". This local attachment is inspection-only because some native continuation details were not preserved." : ". Large files stay unopened until recovery."}</div>}
          {checkpointError && <div className="durable-status" role="status">Checkpoint summary is temporarily unavailable; the saved timeline is still shown. {safeText(checkpointError)}</div>}
          <ol className="durable-timeline">
            {[...(records?.items ?? [])].reverse().map((item) => <HistoryEntry key={`${item.sequence}:${item.ordinal}`} item={item} expanded={expanded[String(item.id)]} onExpand={() => void expand(item)} effects={effects} projectId={projectId} conversationId={selected.id} ownerId={ownerId} />)}
          </ol>
          {records?.nextCursor && <Button variant="secondary" onClick={() => void olderRecords()} disabled={busy}>Load older entries</Button>}
          {!confirmDelete && lease && !lease.live && !checkpoint?.inspectionOnly && (checkpoint?.status === "applied-unsaved" || checkpoint?.status === "apply-unknown") && <Button variant="primary" onClick={() => onContinue(selected.id)}>Check saved source outcome</Button>}
          {!confirmDelete && lease && !lease.live && !lease.unresolvedEffects && !checkpoint?.inspectionOnly && checkpoint?.status !== "applied-unsaved" && checkpoint?.status !== "apply-unknown" && <Button variant="primary" onClick={() => onContinue(selected.id)}>Continue from this saved conversation</Button>}
          {confirmDelete && <div className="durable-delete-confirm" role="alertdialog" aria-label="Confirm conversation deletion"><p>Delete this conversation and its saved details? This cannot be undone.</p><Button variant="secondary" onClick={() => setConfirmDelete(false)}>Cancel</Button><Button variant="danger" onClick={() => void remove()} disabled={busy}>Delete conversation</Button></div>}
        </>}
      </section>
    </div>
    {error && <p className="durable-history-error" role="alert">{error}</p>}
  </Dialog>;
}

function HistoryEntry({ item, expanded, onExpand, effects, projectId, conversationId, ownerId }: { item: DurableRecordSummaryItem; expanded?: Expanded; onExpand: () => void; effects: Awaited<ReturnType<typeof readDurableEffects>>; projectId: string; conversationId: string; ownerId: string }) {
  const summary = item.summary;
  const messages = summary.messages ?? [];
  const callIds = messages.flatMap((message) => message.toolCalls ?? []).map((call) => call.id).filter((id): id is string => Boolean(id));
  const linked = effects.filter((effect) => callIds.includes(effect.callId) || messages.some((message) => message.toolCallId === effect.callId));
  return <li className="durable-timeline-entry">
    <small>{summary.kind} · sequence {item.sequence}</small>
    {summary.intent != null && <div className="durable-intent"><strong>Declared intent</strong><pre>{inertJson(summary.intent)}</pre></div>}
    {messages.map((message, index) => <article key={index} className="durable-message"><strong>{message.role ?? "Message"}{message.toolName ? ` · ${message.toolName}` : ""}{message.isError ? " · refused or failed" : ""}</strong>
      {(message.content ?? []).map((part, partIndex) => <p key={partIndex}>{safeText(part.text)}{part.clipped && <em> · saved preview clipped; expand for the stored entry</em>}</p>)}
      {message.toolCalls?.map((call) => <p className="durable-call-link" key={call.id}>Tool call {safeText(call.name)} · id {safeText(call.id)} · arguments/details on expansion</p>)}
      {message.stopReason && <small>Turn ended: {safeText(message.stopReason)}</small>}
    </article>)}
    {linked.map((effect) => <div className="durable-effect" key={effect.callId}><strong>{safeText(effect.tool)}</strong> · {safeText(effect.state)} · generation {effect.generation} · started {whenAvailable(effect.createdAt)} · updated {whenAvailable(effect.updatedAt)}{effect.observationCallId ? ` · observation ${effect.observationCallId}` : ""}
      {effect.intent != null && <p><strong>Declared intent</strong>: {safeText(effect.intent.text)}{effect.intent.sourceEntryId ? ` · source entry ${safeText(effect.intent.sourceEntryId)}` : ""}{effect.intent.declaredAt ? ` · ${whenAvailable(effect.intent.declaredAt)}` : ""}</p>}
      {effect.refusal && <p>Refusal: {safeText(effect.refusal)}</p>}
    </div>)}
    {item.id !== null && <details onToggle={(event) => { if (event.currentTarget.open && !expanded) onExpand(); }}><summary>Expand stored arguments, result and trace</summary>
      {expanded?.loading && <p>Loading saved entry detail…</p>}{expanded?.error && <p role="alert">{expanded.error}</p>}
      {expanded?.value && <EntryDetails value={expanded.value} projectId={projectId} conversationId={conversationId} ownerId={ownerId} />}
    </details>}
  </li>;
}

function EntryDetails({ value, projectId, conversationId, ownerId }: { value: Record<string, unknown>; projectId: string; conversationId: string; ownerId: string }) {
  const entry = (value.value && typeof value.value === "object" ? value.value : value) as Record<string, unknown>;
  const messages = Array.isArray(entry.model) ? entry.model as Array<Record<string, unknown>> : [];
  const calls = messages.flatMap((message) => Array.isArray(message.content) ? message.content as Array<Record<string, unknown>> : []).filter((part) => part.type === "toolCall");
  const data = entry.data && typeof entry.data === "object" ? entry.data as Record<string, unknown> : {};
  const pointer = findDetailPointer(data) ?? findDetailPointer(entry);
  const [detail, setDetail] = useState<unknown>(null);
  const [loading, setLoading] = useState(false);
  useEffect(() => { setDetail(null); }, [pointer]);
  const loadDetail = async () => {
    if (!pointer || loading || detail !== null) return;
    setLoading(true);
    try { setDetail(await readDurableDetail({ projectId, conversationId, ownerId }, pointer)); }
    catch { setDetail({ unavailable: "Stored detail is not available from this entry." }); }
    finally { setLoading(false); }
  };
  return <div className="durable-entry-details">
    <strong>Stored model-visible message</strong><pre>{inertJson(messages)}</pre>
    {calls.length > 0 && <><strong>Model arguments</strong><pre>{inertJson(calls.map((call) => ({ name: call.name, id: call.id, arguments: call.arguments })))}</pre></>}
    {Object.keys(data).length > 0 && <><strong>Stored trace metadata</strong><pre>{inertJson(data)}</pre></>}
    {pointer && <><Button variant="secondary" onClick={() => void loadDetail()} disabled={loading}>{loading ? "Loading result…" : "Load original result and effective arguments"}</Button>{detail !== null && <><strong>Original result, effective arguments, clipping and redaction</strong><pre>{inertJson(detail)}</pre></>}</>}
    {!pointer && <p className="durable-muted">No separate result detail is stored on this entry.</p>}
  </div>;
}

function findDetailPointer(value: Record<string, unknown>): string | null {
  for (const candidate of Object.values(value)) if (candidate && typeof candidate === "object") {
    const item = candidate as Record<string, unknown>;
    if (item.kind === "immutable-detail" && typeof item.digest === "string") return item.digest;
    const nested = findDetailPointer(item); if (nested) return nested;
  }
  return null;
}
function inertJson(value: unknown) { try { return JSON.stringify(value, null, 2); } catch { return "Stored value could not be displayed."; } }
function safeText(value: unknown) { return typeof value === "string" ? value : value == null ? "" : String(value); }
function message(value: unknown) { return value instanceof Error ? value.message : String(value); }
function whenAvailable(value: number | null | undefined) { return typeof value === "number" && Number.isFinite(value) ? new Date(value).toLocaleString() : "time unavailable"; }
