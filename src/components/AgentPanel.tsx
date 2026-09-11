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
import { Textarea } from "./ui";

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
    if (element) element.scrollTop = state.messages.length > 0 || state.tools.length > 0 ? element.scrollHeight : 0;
  }, [state.messages, state.tools, footer.kind]);

  const model = sessionConfig.modelId || "no model";
  const needsConnect = !live && state.messages.length === 0;

  if (needsConnect) {
    return (
      <div className="messages agent-thread agent-connect-panel" ref={threadRef} aria-live="polite">
        <div className="agent-thread-empty agent-connect-step">
          <span className="agent-empty-mark"><Settings2 size={22} /></span>
          <h2>Connect a model to change this app.</h2>
          <p>Describe what should change in plain words. The model edits the files; you apply the change after you see it.</p>
          <button type="button" className="agent-connect" onClick={onOpenSettings}>
            Connect a model
          </button>
          <p className="agent-empty-note">Your key stays in this browser. Pick a change. It waits here until the model is connected.</p>
          <ExamplePrompts onPick={thread.setDraft} />
        </div>
      </div>
    );
  }

  return (
    <>
      <div className="prompt-box">
        <Textarea aria-label="Message to agent"
          value={thread.draft}
          rows={2}
          placeholder={blocker || "Describe the app."}
          onChange={(event) => thread.setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter" || event.shiftKey) return;
            event.preventDefault();
            if (!thread.draft.trim()) return;
            if (!live) { onOpenSettings(); return; }
            void thread.submit();
          }}
        />
        <div>
          <button className={`agent-model-chip ${live ? "" : "is-connect"}`} onClick={onOpenSettings} title="Agent settings">
            <Settings2 size={11} />
            <span>{blocker || (live ? `${provider.label} · ${model}` : "Connect a model")}</span>
          </button>
          {running
            ? <button className="agent-stop" onClick={thread.interrupt}><Square size={11} /> Stop</button>
            : live && (
              <button
                className="agent-send"
                aria-label="Send"
                title={blocker}
                disabled={running || !thread.draft.trim() || blocker.length > 0}
                onClick={() => { void thread.submit(); }}
              >
                <ArrowUp size={15} />
              </button>
            )}
        </div>
      </div>
      <div className="messages agent-thread" ref={threadRef} aria-live="polite" aria-busy={running}>
        {state.messages.length === 0 && (
          <div className="agent-thread-empty">
            <span className="agent-empty-mark"><Sparkles size={22} /></span>
            <h2>What should change?</h2>
            <p>Describe it in one sentence. You apply the change after you see it.</p>
            <p className="agent-examples-label">Try asking</p>
            <ExamplePrompts onPick={thread.setDraft} />
            <p className="agent-empty-note">{provider.label} uses your key, saved in this browser.</p>
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
            {footer.kind === "validated" && <><ShieldCheck size={13} /> {validationSummary(footer.checked)}{footer.changedFiles.includes("actions.json") ? " This list stops at 100 rows." : ""}</>}
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

    </>
  );
}

const examplePrompts = [
  "Let anyone with the link add a row.",
  "Stop the list at 100 rows.",
  "Ask for a name before the app opens.",
];

function ExamplePrompts({ onPick }: { onPick: (sentence: string) => void }) {
  return (
    <div className="agent-examples">
      {examplePrompts.map((sentence) => (
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
