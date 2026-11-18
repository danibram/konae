import { useEffect, useMemo, useRef, useState } from "react";
import { defaultSessionConfig } from "../agent/catalog";
import { createAgentKeyStore } from "../agent/key-store";
import type { KonaeAgentHandle } from "../agent/harness";
import type { AgentSessionConfig } from "../agent/contracts";
import { createDurableKonaeAgent } from "../agent/harness";
import { useAgentThread } from "./useAgentThread";
import { AgentPanel } from "./AgentPanel";
import { AgentSettingsModal } from "./AgentSettingsModal";
import { Button } from "./ui";
import { cloneStarter } from "../lib/starter";
import { applyProposal } from "../agent/apply";
import { proposalPreviewProject } from "./proposal-preview";
import { ProposalHostLifecycle } from "../runtime/proposal-host";
import { compileProjectInBrowser } from "../runtime/browser-build";
import { createPreviewDocument } from "../runtime/preview";
import { useRuntimeBridge } from "../runtime/useRuntimeBridge";
import type { MiniAppProject } from "../lib/types";
import type { AgentProposal } from "../agent/contracts";
import "./local-first-run.css";

/** Credential-free browser entry. Deliberately owns no platform/session/source API. */
export function LocalFirstRun() {
  const [project, setProject] = useState(() => ({ ...cloneStarter(), id: `local-${crypto.randomUUID()}` }));
  const [previewProject, setPreviewProject] = useState(project);
  const [proposalPreview, setProposalPreview] = useState<{ id: string; proposal: AgentProposal; project: MiniAppProject } | null>(null);
  const [proposalReady, setProposalReady] = useState(false);
  const [proposalError, setProposalError] = useState("");
  const [agent, setAgent] = useState<KonaeAgentHandle | null>(null);
  const [threadToken, setThreadToken] = useState(0);
  const [sessionConfig, setSessionConfig] = useState<AgentSessionConfig>(defaultSessionConfig);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [keyVersion, setKeyVersion] = useState(0);
  const [buildStatus, setBuildStatus] = useState("Build runs in this browser when a proposal is ready.");
  const [frame, setFrame] = useState<HTMLIFrameElement | null>(null);
  const keyStore = useMemo(() => createAgentKeyStore(localStorage), []);
  const hostRef = useRef<ProposalHostLifecycle | null>(null);
  if (!hostRef.current || hostRef.current.appliedHost && project.id !== previewProject.id) hostRef.current = new ProposalHostLifecycle(project.id, { schema: project.files["schema.sql"], actions: project.files["actions.json"], fileAccess: project.modules?.files?.fileAccess as "app-users" | undefined });
  const lifecycle = hostRef.current;
  const agentRef = useRef<KonaeAgentHandle | null>(null);
  const loadRef = useRef<Promise<KonaeAgentHandle | null> | null>(null);
  const projectRef = useRef(project); projectRef.current = project;
  const iframeHost = proposalPreview && lifecycle.isProposalReady(proposalPreview.id) ? lifecycle.proposalHost ?? lifecycle.appliedHost : lifecycle.appliedHost;
  useRuntimeBridge(frame, iframeHost);

  const ensureAgent = () => {
    if (agentRef.current) return Promise.resolve(agentRef.current);
    if (!loadRef.current) loadRef.current = Promise.resolve(createDurableKonaeAgent(projectRef.current, sessionConfig, keyStore, {
      isCurrent: () => projectRef.current.id === project.id,
      onValidation: () => undefined,
    })).then(async (handle) => { await handle.initialize?.(); agentRef.current = handle; setAgent(handle); return handle; }).catch(() => null).finally(() => { loadRef.current = null; });
    return loadRef.current;
  };
  const thread = useAgentThread({ ensureAgent, agent, sessionConfig, keyStore, threadResetToken: threadToken, keyVersion,
    onApply: async (proposal) => {
      const applied = applyProposal(projectRef.current, proposal);
      const preview = proposalPreview;
      if (!applied || !preview || preview.proposal !== proposal || !lifecycle.isProposalReady(preview.id)) return false;
      if (!await lifecycle.applyProposal(preview.id, { schema: applied.project.files["schema.sql"], actions: applied.project.files["actions.json"], fileAccess: applied.project.modules?.files?.fileAccess as "app-users" | undefined })) return false;
      setProject(applied.project); setPreviewProject(preview.project); setProposalPreview(null); setProposalReady(false); setBuildStatus("Applied locally in this browser. Nothing was published.");
      agentRef.current?.reseed(applied.project.files, applied.project.encodings);
      return true;
    },
    onDiscard: async () => { await lifecycle.discardProposal(proposalPreview?.id); setProposalPreview(null); setProposalReady(false); },
  });

  useEffect(() => lifecycle.retain(), [lifecycle]);
  useEffect(() => { void lifecycle.updateAppliedSource({ schema: previewProject.files["schema.sql"], actions: previewProject.files["actions.json"], fileAccess: previewProject.modules?.files?.fileAccess as "app-users" | undefined }); }, [lifecycle, previewProject]);
  useEffect(() => {
    const proposal = thread.state.proposal;
    if (!proposal || thread.footer.kind !== "validated") { void lifecycle.discardProposal(proposalPreview?.id); setProposalPreview(null); setProposalReady(false); return; }
    const candidate = proposalPreviewProject(project, proposal);
    if (!candidate) return;
    let active = true;
    setProposalReady(false); setProposalError(""); setBuildStatus("Building the proposal in this browser…");
    const id = `local-proposal:${crypto.randomUUID()}`;
    void (async () => {
      try {
        const compiled = await compileProjectInBrowser(candidate);
        const host = await lifecycle.prepareProposal(id, { schema: candidate.files["schema.sql"], actions: candidate.files["actions.json"], fileAccess: candidate.modules?.files?.fileAccess as "app-users" | undefined });
        if (!active || !host) return;
        const built = { ...candidate, files: { ...candidate.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script } };
        setProposalPreview({ id, proposal, project: built }); setProposalReady(true); setBuildStatus("Build passed. Preview is ready; Apply changes only this browser's local app.");
      } catch (cause) { if (active) { setProposalError(cause instanceof Error ? cause.message : String(cause)); setBuildStatus("Build or preview setup failed."); } }
    })();
    return () => { active = false; void lifecycle.discardProposal(id); };
  }, [thread.footer.kind, thread.state.proposal, project]);
  useEffect(() => () => { agentRef.current?.dispose(); }, []);

  const displayedProject = proposalPreview && proposalPreview.proposal === thread.state.proposal && proposalReady ? proposalPreview.project : previewProject;
  const previewDoc = useMemo(() => createPreviewDocument(displayedProject), [displayedProject]);
  const reset = () => { agentRef.current?.dispose(); agentRef.current = null; loadRef.current = null; setAgent(null); const next = { ...cloneStarter(), id: `local-${crypto.randomUUID()}` }; setProject(next); setPreviewProject(next); setProposalPreview(null); setProposalReady(false); setThreadToken((value) => value + 1); setBuildStatus("Local app restarted."); };
  const prepareAttachment = async () => {
    const handle = await ensureAgent() as (KonaeAgentHandle & { exportLocalHistory?: () => Promise<unknown> }) | null;
    if (!handle?.exportLocalHistory) { setProposalError("This local conversation could not be prepared for attachment."); return; }
    try {
      const exported = await handle.exportLocalHistory() as Record<string, unknown>;
      const records = Array.isArray(exported.records) ? exported.records as Array<Record<string, unknown>> : [];
      if (records.some((entry) => entry.id === undefined) || new Set(records.map((entry) => String(entry.id))).size !== records.length) throw new Error("This local transcript has missing or repeated native entry IDs and cannot be attached safely.");
      const recordIdMap = Object.fromEntries(records.map((entry, index) => [String(entry.id), index + 2]));
      const candidate = thread.state.proposal ? proposalPreviewProject(projectRef.current, thread.state.proposal) : null;
      const attachment = { ...exported, attachmentId: crypto.randomUUID(), recordIdMap, files: { ...(exported.files as object ?? {}), current: { ...projectRef.current.files }, ...(candidate ? { candidate: { ...candidate.files } } : {}) } };
      const encoded = JSON.stringify(attachment);
      if (new TextEncoder().encode(encoded).byteLength > 4 * 1024 * 1024) throw new Error("This local conversation and its files are too large for the browser handoff. Download or reduce the app before attaching.");
      sessionStorage.setItem("konae:local-history-attachment:v1", encoded);
      window.location.assign("/_konae/owner/login?returnTo=%2F");
    } catch (error) { setProposalError(error instanceof Error ? error.message : String(error)); }
  };

  return <main className="local-first-run">
    <header><a href="/local" className="local-mark">Konae</a><span>Local only · this tab</span><div><Button variant="secondary" onClick={() => setSettingsOpen(true)}>Model key</Button><Button variant="secondary" onClick={() => void prepareAttachment()}>Sign in to attach a copy</Button><Button variant="secondary" onClick={() => { if (confirm("Local app files stay here and will not be uploaded. Open owner sign-in to choose a project to publish?")) window.location.assign("/_konae/owner/login"); }}>Sign in to publish</Button></div></header>
    <section className="local-intro"><h1>Make a small app here first.</h1><p>This local run does not read owner history, projects, session data or MCP tools. Files and conversation stay temporary in this tab until you explicitly attach a copy after sign-in. A connected model key stays in this browser and is sent only to its provider.</p><Button variant="secondary" onClick={reset}>Start a new local app</Button></section>
    <div className="local-workspace"><section className="local-agent"><AgentPanel thread={thread} sessionConfig={sessionConfig} onOpenSettings={() => setSettingsOpen(true)} proposalReady={proposalReady} proposalError={proposalError} applyBlocked={!proposalReady} /></section>
      <section className="local-preview"><header><strong>Preview</strong><span role="status">{buildStatus}</span></header><iframe ref={setFrame} title="Local-only app preview" sandbox="allow-scripts allow-forms" srcDoc={previewDoc} /></section>
    </div>
    <AgentSettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} sessionConfig={sessionConfig} onSessionConfigChange={setSessionConfig} keyStore={keyStore} onKeyStoreChange={() => setKeyVersion((value) => value + 1)} />
  </main>;
}
