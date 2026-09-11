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

type Props = {
  thread: AgentThread;
  sessionConfig: AgentSessionConfig;
  onOpenSettings: () => void;
};

/**
 * The agent conversation, in the editor next to the code it edits. Provider,
 * model and key live in the settings dialog; this panel only reports which of
 * them is answering. All state belongs to `useAgentThread`, so the panel can be
 * dragged between groups without losing the conversation.
 */
export function AgentPanel({ thread, sessionConfig, onOpenSettings }: Props) {
  const { state, provider, live, blocker, footer, running } = thread;
  const threadRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const element = threadRef.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [state.messages, state.tools, footer.kind]);

  const model = sessionConfig.modelId || "no model";

  return (
    <>
      <div className="messages agent-thread" ref={threadRef} aria-live="polite" aria-busy={running}>
        {state.messages.length === 0 && (
          <div className="agent-thread-empty">
            <p>
              Describe a change and the agent edits a disposable copy of your project files. Nothing reaches the
              project until the Validation Gate passes.
            </p>
            <p>
              {live
                ? `${provider.label} answers with your key, stored in this browser.`
                : "No provider key yet — the offline demo answers a few scripted changes. Open settings to connect a model."}
            </p>
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
          <div className="agent-proposal-actions">
            <button className="agent-discard" onClick={thread.discard}>Discard</button>
            <button className="agent-apply" disabled={!footer.canApply || running} onClick={thread.apply}>
              <Send size={12} /> Apply
            </button>
          </div>
        </div>
      )}

      <div className="prompt-box">
        <textarea aria-label="Message to agent"
          value={thread.draft}
          rows={2}
          placeholder={blocker || "Describe a change…"}
          onChange={(event) => thread.setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.shiftKey) return;
            event.preventDefault();
            void thread.submit();
          }}
        />
        <div>
          <button className="agent-model-chip" onClick={onOpenSettings} title="Agent settings">
            <Settings2 size={11} />
            <span>{blocker || (live ? `${provider.label} · ${model}` : "Offline demo")}</span>
          </button>
          {running
            ? <button className="agent-stop" onClick={thread.interrupt}><Square size={11} /> Stop</button>
            : (
              <button
                className="agent-send"
                aria-label="Send"
                title={blocker}
                disabled={!thread.canSend}
                onClick={() => void thread.submit()}
              >
                <ArrowUp size={15} />
              </button>
            )}
        </div>
      </div>
    </>
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
