import {
  ArrowUp,
  BookOpen,
  Check,
  CircleAlert,
  FilePlus2,
  Loader2,
  PenLine,
  Send,
  Settings2,
  ShieldCheck,
  Sparkles,
  Square,
  Wrench,
  X,
} from "lucide-react";
import { useEffect, useRef } from "react";
import type { AgentSessionConfig } from "../agent/contracts";
import { validationSummary, type ToolChip } from "./agent-thread-state";
import type { AgentThread } from "./useAgentThread";
import "./agent-panel.css";
import { Button, Textarea } from "./ui";

type Props = {
  thread: AgentThread;
  sessionConfig: AgentSessionConfig;
  onOpenSettings: () => void;
  /** Changes this app does not make yet, offered before anyone types. */
  suggestions?: string[];
};

/**
 * The agent conversation, in the editor next to the code it edits. Provider,
 * model and key live in the settings dialog; this panel only reports which of
 * them is answering. All state belongs to `useAgentThread`, so the panel can be
 * moved between the rail and the stage without losing the conversation.
 *
 * Top to bottom: the thread, the Gate's verdict with Apply, and the composer,
 * which is always there. With no model connected a picked suggestion still
 * fills the composer, and the send button says what it needs.
 */
export function AgentPanel({ thread, sessionConfig, onOpenSettings, suggestions = [] }: Props) {
  const { state, provider, live, blocker, footer, running } = thread;
  const threadRef = useRef<HTMLDivElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const element = threadRef.current;
    if (element) element.scrollTop = state.messages.length > 0 || state.tools.length > 0 ? element.scrollHeight : 0;
  }, [state.messages, state.tools, footer.kind]);

  const model = sessionConfig.modelId || "no model";
  const descriptionReady = state.messages.length === 0 && !live && Boolean(thread.draft.trim());
  const pick = (sentence: string) => {
    thread.setDraft(sentence);
    requestAnimationFrame(() => {
      const element = composerRef.current;
      if (!element) return;
      element.focus();
      element.setSelectionRange(sentence.length, sentence.length);
    });
  };
  const send = () => {
    if (!thread.draft.trim()) return;
    if (!live) { onOpenSettings(); return; }
    void thread.submit();
  };

  return (
    <div className="agent-panel">
      <div className="messages agent-thread" ref={threadRef} aria-live="polite" aria-busy={running}>
        {state.messages.length === 0 && (
          <div className="agent-thread-empty">
            {descriptionReady ? <>
              <h2>Your description is ready</h2>
              <p>Connect a model and Konae builds it from these words.</p>
              <Button variant="primary" className="agent-ready-connect" onClick={onOpenSettings}><Settings2 size={13} aria-hidden="true" /> Connect a model</Button>
            </> : <>
            <h2>What should change?</h2>
            <p>{live
              ? "Describe it in one sentence. You see the change in the preview and apply it when it looks right."
              : "Describe it in one sentence. A model you connect makes the change, and you apply it after you see it."}</p>
            {suggestions.length > 0 && <>
              <p className="agent-examples-label">Try one</p>
              <ExamplePrompts sentences={suggestions} onPick={pick} />
            </>}
            </>}
            <p className="agent-empty-note">{live ? `${provider.label} uses your key, saved in this browser.` : "Your key stays in this browser."}</p>
          </div>
        )}
        {state.messages.map((message) => (
          <div key={message.id} className={`message ${message.role === "assistant" ? "agent" : "user"}`}>
            {message.role === "assistant" && <span className="message-avatar"><Sparkles size={13} /></span>}
            <p>{message.text}{message.streaming && <i className="agent-caret" />}</p>
          </div>
        ))}
        {state.tools.length > 0 && (
          <div className="agent-tool-strip">
            {state.tools.map((chip) => <ToolChipView key={chip.id} chip={chip} />)}
          </div>
        )}
        {running && <div className="thinking"><i /><i /><i /></div>}
        {state.error && (
          <div className="agent-error" role="alert">
            <CircleAlert size={14} />
            <span>{state.error}</span>
            <button aria-label="Dismiss error" onClick={thread.dismissError}><X size={12} /></button>
          </div>
        )}
      </div>

      {thread.demoPending && (
        <div className="agent-proposal validated">
          <p className="agent-proposal-state"><ShieldCheck size={13} /> Review this change, then apply it. The files stay as they are until you do.</p>
          <div className="agent-proposal-actions">
            <button className="agent-discard" onClick={thread.discardDemo}>Undo</button>
            <button className="agent-apply" onClick={thread.applyDemo}><Send size={12} /> Apply</button>
          </div>
        </div>
      )}

      {footer.kind !== "empty" && (
        <div className={`agent-proposal ${footer.kind}`}>
          <div className="agent-proposal-files">
            {footer.changedFiles.map((file) => <span key={file} className="agent-file-chip">{file}</span>)}
          </div>
          <p className="agent-proposal-state">
            {footer.kind === "validated" && <><ShieldCheck size={13} /> {validationSummary(footer.checked)}</>}
            {footer.kind === "unvalidated" && <><Loader2 size={13} className="spin" /> Waiting for the Validation Gate.</>}
            {footer.kind === "blocked" && <><CircleAlert size={13} /> <em>{footer.stage}</em> {footer.message}</>}
          </p>
          {footer.kind === "blocked" && footer.detail && <p className="agent-proposal-detail">{footer.detail}</p>}
          {footer.kind === "validated" && footer.owner && (
            <div className="agent-proposal-owner" role="status">
              <CircleAlert size={13} />
              <p><strong>{footer.owner.message}</strong>{footer.owner.detail && <span>{footer.owner.detail}</span>}</p>
            </div>
          )}
          <div className="agent-proposal-actions">
            <button className="agent-discard" onClick={thread.discard}>Discard</button>
            <button className="agent-apply" disabled={!footer.canApply || running} onClick={thread.apply}>
              <Send size={12} /> Apply
            </button>
          </div>
        </div>
      )}

      <div className="prompt-box">
        <Textarea id="agent-composer" ref={composerRef} aria-label="Message to agent"
          value={thread.draft}
          rows={2}
          placeholder={blocker || "Describe the change."}
          onChange={(event) => thread.setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.shiftKey) return;
            event.preventDefault();
            send();
          }}
        />
        <div>
          {live && (
            <button className="agent-model-chip" onClick={onOpenSettings} title="Model settings">
              <Settings2 size={11} />
              <span>{blocker || `${provider.label} · ${model}`}</span>
            </button>
          )}
          {running
            ? <button className="agent-stop" onClick={thread.interrupt}><Square size={11} /> Stop</button>
            : live
              ? (
                <button
                  className="agent-send"
                  aria-label="Send"
                  title={blocker}
                  disabled={!thread.draft.trim() || blocker.length > 0}
                  onClick={send}
                >
                  <ArrowUp size={15} />
                </button>
              )
              : !descriptionReady && <Button variant="primary" size="sm" className="agent-connect" onClick={onOpenSettings}><Settings2 size={13} aria-hidden="true" /> Connect a model to send</Button>}
        </div>
      </div>
    </div>
  );
}

function ExamplePrompts({ sentences, onPick }: { sentences: string[]; onPick: (sentence: string) => void }) {
  return (
    <div className="agent-examples">
      {sentences.map((sentence) => (
        <button key={sentence} type="button" onClick={() => onPick(sentence)}>{sentence}</button>
      ))}
    </div>
  );
}

function ToolChipView({ chip }: { chip: ToolChip }) {
  const Icon = toolIcon(chip.name);
  return (
    <span className={`agent-tool-chip ${chip.state}`} title={chip.detail}>
      {chip.state === "running" ? <Loader2 size={10} className="spin" /> : <Icon size={10} />}
      <strong>{chip.name}</strong>
      {chip.detail && <small>{chip.detail}</small>}
      {chip.state === "ok" && <Check size={10} />}
      {chip.state === "fail" && <CircleAlert size={10} />}
    </span>
  );
}

function toolIcon(name: string) {
  if (name.startsWith("read")) return BookOpen;
  if (name.startsWith("edit")) return PenLine;
  if (name.startsWith("write")) return FilePlus2;
  if (name.startsWith("validate")) return ShieldCheck;
  if (name.startsWith("preview")) return Wrench;
  return Wrench;
}
