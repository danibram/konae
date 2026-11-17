import {
  Activity,
  BarChart3,
  Box,
  ChevronDown,
  CircleAlert,
  Code2,
  Database,
  Download,
  ExternalLink,
  FileCode2,
  FilePlus2,
  FileJson2,
  FileText,
  FolderPlus,
  FolderOpen,
  FolderTree,
  Laptop,
  MoreHorizontal,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Play,
  RefreshCw,
  Rocket,
  RotateCcw,
  Settings2,
  ShieldCheck,
  Smartphone,
  Sparkles,
  Trash2,
  UserRound,
  Wifi,
} from "lucide-react";
import { lazy, Suspense, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { PROVIDERS, defaultSessionConfig } from "./agent/catalog";
import { applyProposal } from "./agent/apply";
import type { AgentProposal, AgentSessionConfig } from "./agent/contracts";
import type { KonaeAgentHandle, PlatformConfirmation, PublishOutcome } from "./agent/harness";
import { createAgentKeyStore } from "./agent/key-store";
import { Alert, Button, Dialog, IconButton, LoadingBlock, Menu, TabBar, ToastRegion, type MenuItem, type TabBarItem, type ToastItem } from "./components/ui";
import { AppStatus, UndeployedMarker } from "./components/AppStatus";
import { accessNotDeployed, ownerStorage, releasedAt, sameAsDeployed } from "./deployments/deployed-changes";
import { cachedSourceDigest, useSourceDigest } from "./deployments/source-digest";
import { ProjectCrumb } from "./components/ProjectCrumb";
import { switcherProjects } from "./components/project-list";
import { suggestedChanges } from "./components/agent-suggestions";
import "./components/workbench.css";
import { AgentPanel } from "./components/AgentPanel";
import { proposalPreviewProject, type ProposalPreview } from "./components/proposal-preview";
import { AgentSettingsModal } from "./components/AgentSettingsModal";
import { describeDraftKey, useAgentThread } from "./components/useAgentThread";
import { focusPanelTab, WorkbenchLayout, type PanelRender } from "./components/WorkbenchLayout";
import { panelIds, stack, stageLayout, type LayoutState, type PanelId } from "./ui/layout";
import { inTextEntry, shortcutFor } from "./ui/shortcuts";
import { DeployDialog } from "./components/DeployDialog";
import { SchemaChangeDialog } from "./components/SchemaChangeDialog";
import { SchemaChangeDeclined, type SchemaChange } from "./deployments/release";
import { ExplorerTree } from "./components/ExplorerTree";
import { ActionContractView } from "./components/ActionContractView";
import { useActionContract } from "./components/useActionContract";
import { ACTION_CONTRACT_MARKDOWN } from "./lib/action-contract";
import { isSignedOutError, ownerInitials, platformAccessHref, useOwnerSession, type OwnerSession } from "./components/owner-session";
import { PlatformAccess } from "./components/PlatformAccess";
import { BootShell } from "./components/BootShell";
import { useLoadingHold } from "./components/loading-hold";
import { heldLazy } from "./components/held-lazy";
import { filesUnder, renamePath } from "./components/project-tree";
import { usePreviewProblems } from "./components/usePreviewProblems";
import { ProjectDashboard } from "./components/ProjectDashboard";
import { NotFound } from "./components/NotFound";
import { deployProject, HostedDatabaseClient, type DeploymentReceipt, type SignupMode } from "./deployments/client";
import { McpError, platformMcp } from "./mcp/client";
import { readReleaseFacts } from "./deployments/release-facts";
import type { DeploymentVisibility, RuntimeActionAccess } from "./deployments/manifest";
import { parseServerActionSource } from "./kernel/actions";
import { parseRoute, pushRoute } from "./lib/routing";
import { applyTemplate, defaultTemplate, type TemplateId } from "./lib/templates";
import { cloneStarter, plainListProject } from "./lib/starter";
import type { MiniAppProject, ProjectFile } from "./lib/types";
import { isEncoded, sourceBytes } from "./lib/source-files";
import { formatBytes } from "./lib/usage";
import { createDirectory, createFile, normalizeProjectPath, protectedProjectPaths, removePath } from "./lib/project-files";
import { actionReadout, emptyActionProgram, refusedSpot } from "./components/action-readiness";
import { createWorkspaceProject, loadWorkspaceState, persistWorkspace, resetWorkspaceProject } from "./lib/workspace";
import { ProjectSources, SaveSupersededError, type ProjectMetadata } from "./lib/project-sources";
import { createPreviewDocument } from "./runtime/preview";
import { buildProjectInBrowser, compileProjectInBrowser } from "./runtime/browser-build";
import { useRuntimeBridge } from "./runtime/useRuntimeBridge";
import { ProposalHostLifecycle } from "./runtime/proposal-host";

const WhatHappenedView = heldLazy(() => import("./components/WhatHappenedView").then((module) => module.WhatHappenedView));
const CapabilityWorkspace = heldLazy(() => import("./components/CapabilityWorkspace").then((module) => module.CapabilityWorkspace));
const AppSettingsDialog = heldLazy(() => import("./components/AppSettingsDialog").then((module) => module.AppSettingsDialog));

type EditorFile = string;

const knownFileMeta: Record<string, { label: string; icon: typeof FileText; language: string }> = {
  "index.html": { label: "index.html", icon: FileCode2, language: "HTML" },
  "styles.css": { label: "styles.css", icon: FileText, language: "CSS" },
  "app.js": { label: "app.js", icon: FileJson2, language: "JAVASCRIPT" },
  "schema.sql": { label: "SQL", icon: Database, language: "SQL" },
  "actions.json": { label: "actions.json", icon: ShieldCheck, language: "SERVER ACTIONS" },
};

const UsagePage = lazy(() => import("./components/UsagePage").then((module) => ({ default: module.UsagePage })));

const DesignSystem = lazy(() => import("./components/DesignSystem").then((module) => ({ default: module.DesignSystem })));

const modKey = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? "⌘" : "Ctrl+";

export function App() {
  if (window.location.pathname === "/platform-access") return <PlatformAccess />;
  if (window.location.pathname === "/design-system") return <PublicDesignSystem />;
  return <PlatformGate />;
}

function PublicDesignSystem() {
  const [session] = useOwnerSession();
  return <Suspense fallback={<BootShell ownerSession={session} />}><DesignSystem ownerSession={session} /></Suspense>;
}

function PlatformGate() {
  const [session, refresh] = useOwnerSession();

  useEffect(() => {
    if (session.state === "signed-out") {
      const returnTo = `${window.location.pathname}${window.location.search}${window.location.hash}`;
      window.location.replace(platformAccessHref(returnTo));
    }
  }, [session.state]);

  if (session.state === "unavailable") {
    return <main className="platform-page"><div className="platform-main"><CardForPlatformError message={session.message} onRetry={refresh} /></div></main>;
  }
  if (session.state !== "signed-in") return <BootShell ownerSession={session} />;
  if (window.location.pathname === "/usage") return <Suspense fallback={<BootShell ownerSession={session} />}><UsagePage key={session.ownerId} ownerSession={session} /></Suspense>;
  return <WorkspaceGate key={session.ownerId} ownerSession={session} refreshOwnerSession={refresh} />;
}

function WorkspaceGate({ ownerSession, refreshOwnerSession }: { ownerSession: Extract<OwnerSession, { state: "signed-in" }>; refreshOwnerSession: () => void }) {
  const [sources] = useState(() => new ProjectSources(ownerSession.ownerId));
  const [loaded, setLoaded] = useState<Awaited<ReturnType<ProjectSources["load"]>> | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    void sources.load().then((value) => { if (active) setLoaded(value); }).catch((cause) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, [sources]);
  const booting = useLoadingHold(!loaded, { sinceNavigation: true });
  if (error) return <main className="platform-page"><div className="platform-main"><CardForPlatformError message={error} onRetry={() => window.location.reload()} /></div></main>;
  if (!loaded || booting) return <BootShell ownerSession={ownerSession} />;
  return <WorkspaceApp ownerSession={ownerSession} refreshOwnerSession={refreshOwnerSession} sources={sources} loaded={loaded} />;
}

function CardForPlatformError({ message, onRetry }: { message: string; onRetry: () => void }) {
  return <div className="platform-card"><h1>Platform unavailable</h1><p role="alert">Could not check your session. {message}</p><Button onClick={onRetry}>Try again</Button></div>;
}

function WorkspaceApp({ ownerSession, refreshOwnerSession, sources, loaded }: { ownerSession: Extract<OwnerSession, { state: "signed-in" }>; refreshOwnerSession: () => void; sources: ProjectSources; loaded: { projects: MiniAppProject[]; missing: ProjectMetadata[]; deployments: Record<string, DeploymentReceipt> } }) {
  const workspaceStorage = useMemo(() => ownerStorage(ownerSession.ownerId), [ownerSession.ownerId]);
  const [initialWorkspace] = useState(() => {
    const cached = loadWorkspaceState(workspaceStorage);
    return { projects: loaded.projects, deployments: loaded.deployments, activeProjectId: loaded.projects.some((item) => item.id === cached.activeProjectId) ? cached.activeProjectId : loaded.projects[0]?.id ?? "" };
  });
  const [missing, setMissing] = useState(loaded.missing);
  const [projects, setProjects] = useState(initialWorkspace.projects);
  const [deployments, setDeployments] = useState(initialWorkspace.deployments);
  const [project, setProject] = useState(() => initialWorkspace.projects.find((item) => item.id === initialWorkspace.activeProjectId) ?? initialWorkspace.projects[0] ?? cloneStarter());
  const [previewProject, setPreviewProject] = useState(project);
  const [loadingProjectId, setLoadingProjectId] = useState("");
  const [projectLoadError, setProjectLoadError] = useState("");
  const [previewEpoch, setPreviewEpoch] = useState(0);
  const [route, setRoute] = useState(() => {
    const requested = parseRoute(window.location.pathname);
    if (requested.screen !== "editor" || loaded.projects.some((item) => item.id === requested.projectId)) return requested;
    if (loaded.missing.some((item) => item.id === requested.projectId)) return requested;
    return { screen: "not-found" as const, what: "project" as const, path: window.location.pathname };
  });
  const screen = route.screen;
  const [activeFile, setActiveFile] = useState<EditorFile>("app.js");
  const [selectedPath, setSelectedPath] = useState("app.js");
  const [newEntry, setNewEntry] = useState<"file" | "folder" | null>(null);
  const [newEntryPath, setNewEntryPath] = useState("");
  const [treeError, setTreeError] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [resetOpen, setResetOpen] = useState(false);
  const [viewport, setViewport] = useState<"desktop" | "mobile">("desktop");
  const [deployOpen, setDeployOpen] = useState(false);
  const [deployment, setDeployment] = useState<DeploymentReceipt | null>(null);
  const [databaseChoice, setDatabaseTarget] = useState<"local" | "deployed" | null>(null);
  const [authView, setAuthView] = useState<"rules" | "users" | "integrations">("rules");
  const [inviteFocus, setInviteFocus] = useState(0);
  const [exporting, setExporting] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [deployError, setDeployError] = useState("");
  const [deployErrorCode, setDeployErrorCode] = useState<string | undefined>();
  const [schemaChange, setSchemaChange] = useState<{ change: SchemaChange; resolve: (confirmed: boolean) => void } | null>(null);
  const [saved, setSaved] = useState(true);
  const [saveError, setSaveError] = useState("");
  const [sourceConflict, setSourceConflict] = useState(false);
  const [operationError, setOperationError] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [appSettingsOpen, setAppSettingsOpen] = useState(false);
  const [domainVersion, setDomainVersion] = useState(0);
  const [platformConfirmation, setPlatformConfirmation] = useState<PlatformConfirmation | null>(null);
  const resolvePlatformConfirmation = useRef<((approved: boolean) => void) | null>(null);
  const resolveAgentPublish = useRef<((outcome: PublishOutcome) => void) | null>(null);
  const deploymentsRef = useRef(deployments);
  deploymentsRef.current = deployments;
  const [agent, setAgent] = useState<KonaeAgentHandle | null>(null);
  const [agentSession, setAgentSession] = useState<AgentSessionConfig>(defaultSessionConfig);
  const [threadResetToken, setThreadResetToken] = useState(0);
  const [keyVersion, setKeyVersion] = useState(0);
  const codeTabEscape = useRef(false);
  const agentRef = useRef<KonaeAgentHandle | null>(null);
  const agentLoad = useRef<Promise<KonaeAgentHandle | null> | null>(null);
  const projectRef = useRef(project);
  const hostLifecycleRef = useRef<{ projectId: string; lifecycle: ProposalHostLifecycle } | null>(null);
  if (hostLifecycleRef.current?.projectId !== project.id) {
    hostLifecycleRef.current = {
      projectId: project.id,
      lifecycle: new ProposalHostLifecycle(project.id, {
        schema: project.files["schema.sql"],
        actions: project.files["actions.json"],
        fileAccess: project.modules?.files?.fileAccess as "app-users" | undefined,
      }),
    };
  }
  const hostLifecycle = hostLifecycleRef.current.lifecycle;
  useEffect(() => hostLifecycle.retain(), [hostLifecycle]);
  useEffect(() => {
    void hostLifecycle.updateAppliedSource({
      schema: previewProject.files["schema.sql"],
      actions: previewProject.files["actions.json"],
      fileAccess: previewProject.modules?.files?.fileAccess as "app-users" | undefined,
    });
  }, [hostLifecycle, previewProject.files["schema.sql"], previewProject.files["actions.json"], previewProject.modules?.files?.fileAccess]);
  const workspaceRef = useRef({ projects, project, deployments });
  const synchronized = useRef(new Map(loaded.projects.map((item) => [item.id, JSON.stringify(item)])));
  const serverSnapshots = useRef(new Map(loaded.projects.map((item) => [item.id, JSON.stringify(item)])));
  const conflictedRevision = useRef<number | null>(null);
  const previewBuildVersion = useRef(0);
  const workspaceGeneration = useRef(0);
  const projectSelectionVersion = useRef(0);
  const activeProjectSelection = useRef<string | null>(null);
  const retryLoadController = useRef<AbortController | null>(null);
  const routeRef = useRef(route);
  const pendingSaves = useRef(0);
  const failedSave = useRef(false);
  const failedProjects = useRef(new Set<string>());
  const [saveAttempt, setSaveAttempt] = useState(0);
  workspaceRef.current = { projects, project, deployments };

  useEffect(() => {
    const generation = ++workspaceGeneration.current;
    return () => { if (workspaceGeneration.current === generation) workspaceGeneration.current++; };
  }, []);

  useEffect(() => {
    const flush = () => {
      const current = workspaceRef.current;
      const items = current.projects.map((item) => item.id === current.project.id ? current.project : item);
      try { persistWorkspace(items, current.project.id, current.deployments, workspaceStorage); }
      catch { /* The regular save reports storage failures in the editor. */ }
      sources.cancelAll();
    };
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", flush);
    };
  }, [workspaceStorage, sources]);

  const gutterRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const keyStore = useMemo(() => createAgentKeyStore(localStorage, ownerSession.ownerId), [ownerSession.ownerId]);
  const [explorerOpen, setExplorerOpen] = useState(false);
  const compact = useMediaQuery("(max-width: 900px)");
  const wide = useMediaQuery("(min-width: 1200px)");
  const [railOpen, setRailOpen] = useState(() => localStorage.getItem("konae:describe-rail") !== "closed");
  const rail = wide && !compact && railOpen;
  const railFolded = wide && !compact && !railOpen;
  const foldRail = (open: boolean) => {
    setRailOpen(open);
    try { localStorage.setItem("konae:describe-rail", open ? "open" : "closed"); } catch { /* The choice lasts this visit. */ }
    requestAnimationFrame(() => document.getElementById(open ? "describe-rail-fold" : "describe-rail-open")?.focus());
  };
  const [compactPanel, setCompactPanel] = useState<PanelId>("preview");
  const [stage, setStage] = useState<PanelId>("preview");
  const [codeShare, setCodeShare] = useState(() => Number(localStorage.getItem("konae:code-share")) || 50);
  const [visited, setVisited] = useState<ReadonlySet<PanelId>>(() => new Set<PanelId>(["preview", "agent"]));
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const [exportAhead, setExportAhead] = useState(false);
  const toast = (message: string, tone: ToastItem["tone"] = "success") => {
    const id = Date.now() + Math.random();
    setToasts((current) => [...current, { id, tone, message }]);
    window.setTimeout(() => setToasts((current) => current.filter((item) => item.id !== id)), 7000);
  };

  useEffect(() => {
    setAppSettingsOpen(false);
    setPlatformConfirmation(null);
    setExportAhead(false);
    return () => {
      resolvePlatformConfirmation.current?.(false);
      resolvePlatformConfirmation.current = null;
      resolveAgentPublish.current?.({ status: "declined" });
      resolveAgentPublish.current = null;
    };
  }, [project.id]);

  const shownSection: PanelId = compact ? compactPanel : stage === "agent" && wide ? "preview" : stage;
  const displayedLayout: LayoutState = compact
    ? { root: stack(workbenchPanels, workbenchPanels.indexOf(compactPanel)), closed: [], focused: compactPanel }
    : stageLayout(shownSection, { rail, codeShare });
  const updateLayout = (next: LayoutState) => {
    if (compact) {
      if (next.focused) setCompactPanel(next.focused);
      return;
    }
    if (next.root?.type === "split" && next.root.sizes[0] !== codeShare) {
      setCodeShare(next.root.sizes[0]);
      localStorage.setItem("konae:code-share", String(next.root.sizes[0]));
    }
  };
  const show = (panel: PanelId) => {
    if (panel === "explorer") return;
    setVisited((current) => current.has(panel) ? current : new Set(current).add(panel));
    if (compact) {
      setCompactPanel(panel);
      return;
    }
    if (panel === "agent" && wide) {
      if (!rail) foldRail(true);
      requestAnimationFrame(() => requestAnimationFrame(() => document.getElementById("agent-composer")?.focus()));
      return;
    }
    setStage(panel);
  };
  const [proposalPreview, setProposalPreview] = useState<ProposalPreview | null>(null);
  const [proposalPreparation, setProposalPreparation] = useState<{ proposal: AgentProposal; error?: string } | null>(null);
  const [previewFrame, setPreviewFrame] = useState<HTMLIFrameElement | null>(null);
  useEffect(() => {
    const preload = () => { for (const panel of [CapabilityWorkspace, WhatHappenedView, AppSettingsDialog]) panel.preload().catch(() => {}); };
    if (typeof window.requestIdleCallback === "function") {
      const id = window.requestIdleCallback(preload, { timeout: 3000 });
      return () => window.cancelIdleCallback(id);
    }
    const timer = window.setTimeout(preload, 1500);
    return () => window.clearTimeout(timer);
  }, []);
  const activeDeployment = deployments[project.id] ?? null;
  const sourceDigest = useSourceDigest(activeDeployment ? project : null);
  const createdDigest = activeDeployment ? null : readCreatedDigest(workspaceStorage, project.id);
  const currentDigest = useSourceDigest(createdDigest ? project : null);
  const asCreated = createdDigest !== null && (createdDigest === "pending" || currentDigest === null || currentDigest === createdDigest);
  useEffect(() => {
    if (createdDigest && createdDigest !== "pending" && (activeDeployment || (currentDigest !== null && currentDigest !== createdDigest))) forgetCreatedDigest(workspaceStorage, project.id);
  }, [createdDigest, currentDigest, activeDeployment, workspaceStorage, project.id]);
  const sameAsLive = sameAsDeployed(project, activeDeployment, sourceDigest);
  const undeployed = sameAsLive === false;
  const deployedDatabase = useMemo(() => activeDeployment ? new HostedDatabaseClient(activeDeployment.id) : undefined, [activeDeployment]);
  const databaseTarget = deployedDatabase ? databaseChoice ?? "deployed" : "local";
  const liveRows = useLiveRowCount(deployedDatabase, shownSection === "preview" || shownSection === "code");
  const actionContract = useActionContract(project.files["actions.json"], project.files["schema.sql"], project);
  const contractOpen = activeFile === ACTION_CONTRACT_MARKDOWN;
  const activeEncoded = !contractOpen && isEncoded(project, activeFile);
  const activeSource = contractOpen ? (actionContract.ok ? actionContract.markdown : actionContract.reason) : activeEncoded ? "" : project.files[activeFile] ?? "";
  const activeMeta = fileMetaFor(activeFile);
  const lines = activeSource.split("\n").length;

  useEffect(() => {
    setProjects((current) => {
      const index = current.findIndex((item) => item.id === project.id);
      if (index < 0) return current;
      if (current[index] === project) return current;
      return current.map((item) => item.id === project.id ? project : item);
    });
  }, [project]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      try {
        persistWorkspace(projects, project.id, deployments, workspaceStorage);
      } catch (error) {
        setSaveError(error instanceof Error ? error.message : String(error));
      }
    }, 350);
    return () => window.clearTimeout(timeout);
  }, [deployments, project, projects, workspaceStorage]);

  useEffect(() => {
    if (publishing) return;
    const timeout = window.setTimeout(() => {
      for (const item of projects) {
        if (sources.hasSources(item.id) && !sources.isLoaded(item.id)) continue;
        const snapshot = JSON.stringify(item);
        if (synchronized.current.get(item.id) === snapshot) continue;
        synchronized.current.set(item.id, snapshot);
        pendingSaves.current++;
        setSaved(false);
        void sources.save(item).then(() => { serverSnapshots.current.set(item.id, snapshot); }).catch((error) => {
          if (error instanceof SaveSupersededError) return;
          failedSave.current = true;
          failedProjects.current.add(item.id);
          setSaveError(error instanceof Error ? error.message : String(error));
        }).finally(() => {
          pendingSaves.current--;
          if (pendingSaves.current === 0 && !failedSave.current) setSaved(true);
        });
      }
    }, 500);
    return () => window.clearTimeout(timeout);
  }, [projects, publishing, sources, saveAttempt]);

  useEffect(() => {
    if (!activeDeployment || route.screen !== "editor" || !sources.isLoaded(project.id)) return;
    let active = true;
    const check = async () => {
      let reloadPending = false;
      try {
        const revision = await sources.inspectRevision(project.id);
        if (!active || revision === sources.revision(project.id) || conflictedRevision.current === revision) return;
        const clean = JSON.stringify(projectRef.current) === serverSnapshots.current.get(project.id);
        if (!clean) {
          conflictedRevision.current = revision;
          setSourceConflict(true);
          return;
        }
        const baseline = JSON.stringify(projectRef.current);
        const latest = await sources.reload(project.id);
        reloadPending = true;
        if (!active || !latest.project || projectRef.current.id !== project.id || JSON.stringify(projectRef.current) !== baseline) {
          sources.cancelReload(project.id);
          if (active && projectRef.current.id === project.id) {
            conflictedRevision.current = latest.revision;
            setSourceConflict(true);
          }
          return;
        }
        const compiled = await compilePreviewSnapshot(latest.project);
        if (!active || projectRef.current.id !== project.id || JSON.stringify(projectRef.current) !== baseline) {
          sources.cancelReload(project.id);
          if (active && projectRef.current.id === project.id) {
            conflictedRevision.current = latest.revision;
            setSourceConflict(true);
          }
          return;
        }
        const currentRevision = await sources.inspectRevision(project.id);
        if (!active || projectRef.current.id !== project.id || JSON.stringify(projectRef.current) !== baseline) {
          sources.cancelReload(project.id);
          if (active && projectRef.current.id === project.id) {
            conflictedRevision.current = latest.revision;
            setSourceConflict(true);
            setSaveError("This draft changed while saved sources were loading. Keep this draft or reload saved sources again.");
          }
          return;
        }
        if (currentRevision !== latest.revision) {
          sources.cancelReload(project.id);
          conflictedRevision.current = null;
          return;
        }
        sources.acceptReload(project.id, latest);
        reloadPending = false;
        serverSnapshots.current.set(project.id, JSON.stringify(latest.project));
        synchronized.current.set(project.id, JSON.stringify(latest.project));
        failedProjects.current.delete(project.id);
        failedSave.current = failedProjects.current.size > 0;
        setSaveError("");
        setProject(latest.project);
        setProjects((current) => current.map((item) => item.id === project.id ? latest.project! : item));
        if (compiled) {
          setPreviewProject(compiled);
          setPreviewEpoch((value) => value + 1);
        }
        setSourceConflict(false);
        conflictedRevision.current = null;
      } catch {
        if (reloadPending) sources.cancelReload(project.id);
        /* The normal save or next poll reports a stale owner session. */
      }
    };
    const timer = window.setInterval(() => { void check(); }, 2500);
    void check();
    return () => { active = false; window.clearInterval(timer); };
  }, [activeDeployment?.id, project.id, route.screen, sources]);

  const reloadConflictingSources = async () => {
    const id = project.id;
    const generation = workspaceGeneration.current;
    const baseline = JSON.stringify(projectRef.current);
    try {
      const latest = await sources.reload(id);
      if (!latest.project || projectRef.current.id !== id || JSON.stringify(projectRef.current) !== baseline) {
        sources.cancelReload(id);
        if (projectRef.current.id === id) {
          conflictedRevision.current = latest.revision;
          setSourceConflict(true);
          setSaveError("This draft changed while saved sources were loading. Keep this draft or reload saved sources again.");
        }
        return;
      }
      const compiled = await compilePreviewSnapshot(latest.project);
      if (projectRef.current.id !== id || JSON.stringify(projectRef.current) !== baseline) {
        sources.cancelReload(id);
        conflictedRevision.current = latest.revision;
        setSourceConflict(true);
        setSaveError("This draft changed while saved sources were loading. Keep this draft or reload saved sources again.");
        return;
      }
      const currentRevision = await sources.inspectRevision(id);
      if (generation !== workspaceGeneration.current || projectRef.current.id !== id || JSON.stringify(projectRef.current) !== baseline) {
        sources.cancelReload(id);
        if (generation === workspaceGeneration.current && projectRef.current.id === id) {
          conflictedRevision.current = latest.revision;
          setSourceConflict(true);
          setSaveError("This draft changed while saved sources were loading. Keep this draft or reload saved sources again.");
        }
        return;
      }
      if (currentRevision !== latest.revision) {
        sources.cancelReload(id);
        conflictedRevision.current = null;
        setSourceConflict(true);
        return;
      }
      sources.acceptReload(id, latest);
      serverSnapshots.current.set(id, JSON.stringify(latest.project));
      synchronized.current.set(id, JSON.stringify(latest.project));
      failedProjects.current.delete(id);
      failedSave.current = failedProjects.current.size > 0;
      setSaveError("");
      setProject(latest.project);
      setProjects((current) => current.map((item) => item.id === id ? latest.project! : item));
      if (compiled) {
        setPreviewProject(compiled);
        setPreviewEpoch((value) => value + 1);
      }
      setSourceConflict(false);
      conflictedRevision.current = null;
    } catch (error) {
      sources.cancelReload(id);
      setSaveError(`Could not reload saved sources: ${error instanceof Error ? error.message : String(error)}`);
    }
  };

  const retrySave = () => {
    if (pendingSaves.current) return;
    for (const id of failedProjects.current) { sources.retry(id); synchronized.current.delete(id); }
    failedProjects.current.clear();
    failedSave.current = false;
    setSaveError("");
    setSaveAttempt((value) => value + 1);
  };

  useEffect(() => {
    if (gutterRef.current) gutterRef.current.scrollTop = editorRef.current?.scrollTop ?? 0;
  }, [activeFile]);

  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      const action = shortcutFor({
        key: event.key,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        inTextEntry: inTextEntry(event.target),
      });
      if (!action) return;
      event.preventDefault();
      if (action.kind === "run") {
        runPreview({ show: true });
        return;
      }
      if (action.panel === "explorer") {
        toggleExplorer({ focus: true });
        return;
      }
      const target = action.kind === "fold" && shownSection === action.panel && action.panel !== "preview" ? "preview" : action.panel;
      show(target);
      if (!(target === "agent" && rail)) requestAnimationFrame(() => focusPanelTab(target));
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  });

  useEffect(() => {
    if (screen !== "editor") return;
    const timer = window.setTimeout(() => {
      void import("./components/WhatHappenedView");
      void import("./components/CapabilityWorkspace");
      void import("./components/AppSettingsDialog");
    }, 600);
    return () => window.clearTimeout(timer);
  }, [screen]);

  useEffect(() => {
    if (!compact || shownSection !== "code") return;
    document.querySelector(".compact-file-list [aria-current='page']")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [compact, shownSection, activeFile]);

  const updateFile = (value: string) => {
    if (contractOpen) return;
    setProject((current) => isEncoded(current, activeFile) ? current : {
      ...current,
      updatedAt: Date.now(),
      files: { ...current.files, [activeFile]: value },
    });
  };

  const addTreeEntry = () => {
    if (!newEntry) return;
    if (!newEntryPath.trim()) {
      setTreeError(newEntry === "file" ? "Enter a file path, for example src/tasks.js." : "Enter a folder path, for example src/features.");
      return;
    }
    try {
      const current = projectRef.current;
      const tree = { files: current.files, directories: current.directories, encodings: current.encodings };
      const next = newEntry === "file" ? createFile(tree, newEntryPath) : createDirectory(tree, newEntryPath);
      const path = normalizeProjectPath(newEntryPath) ?? "";
      setProject({ ...current, ...next, updatedAt: Date.now() });
      if (newEntry === "file") setActiveFile(path);
      setSelectedPath(path);
      setNewEntry(null);
      setNewEntryPath("");
      setTreeError("");
    } catch (error) {
      setTreeError(error instanceof Error ? error.message : String(error));
    }
  };

  const startNewEntry = (kind: "file" | "folder") => {
    const folder = project.directories.includes(selectedPath) ? `${selectedPath}/` : "";
    setRenaming(false);
    setNewEntry(kind);
    setNewEntryPath(folder);
    setTreeError("");
  };

  const startRename = () => {
    if (!selectedPath || protectedProjectPaths.has(selectedPath)) return;
    setNewEntry(null);
    setRenaming(true);
    setRenameValue(selectedPath);
    setTreeError("");
  };

  const renameSelectedPath = () => {
    try {
      const current = projectRef.current;
      const from = selectedPath;
      const next = renamePath({ files: current.files, directories: current.directories, encodings: current.encodings }, from, renameValue);
      const to = normalizeProjectPath(renameValue) ?? from;
      const moved = (path: string) => path === from ? to : path.startsWith(`${from}/`) ? `${to}${path.slice(from.length)}` : path;
      setProject({ ...current, ...next, updatedAt: Date.now() });
      setActiveFile((file) => moved(file));
      setSelectedPath(to);
      setRenaming(false);
      setTreeError("");
    } catch (error) {
      setTreeError(error instanceof Error ? error.message : String(error));
    }
  };

  const deletePath = (path: string) => {
    if (!path || protectedProjectPaths.has(path)) return;
    const current = projectRef.current;
    setProject({
      ...current,
      ...removePath({ files: current.files, directories: current.directories, encodings: current.encodings }, path),
      updatedAt: Date.now(),
    });
    if (activeFile === path || activeFile.startsWith(`${path}/`)) setActiveFile("app.js");
    setSelectedPath("app.js");
    setPendingDelete(null);
  };

  const compilePreviewSnapshot = async (snapshot: MiniAppProject): Promise<MiniAppProject | null> => {
    const version = ++previewBuildVersion.current;
    setOperationError("");
    try {
      const compiled = await compileProjectInBrowser(snapshot);
      if (version !== previewBuildVersion.current) return null;
      return {
        ...snapshot,
        files: { ...snapshot.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script },
      };
    } catch (error) {
      if (version === previewBuildVersion.current) setOperationError(`Build failed: ${error instanceof Error ? error.message : String(error)}`);
      return null;
    }
  };

  const isCurrentProjectSelection = (projectId: string, version: number) =>
    version === projectSelectionVersion.current
    && routeRef.current.screen === "editor"
    && routeRef.current.projectId === projectId;

  const invalidateProjectSelection = () => {
    projectSelectionVersion.current++;
    activeProjectSelection.current = null;
    retryLoadController.current?.abort();
    retryLoadController.current = null;
  };

  const acceptProjectSelection = (loadedProject: MiniAppProject, selectionVersion: number, fromSourceLoad = false) => {
    if (!isCurrentProjectSelection(loadedProject.id, selectionVersion)) return;
    activeProjectSelection.current = loadedProject.id;
    if (fromSourceLoad) {
      setProjects((current) => current.map((item) => item.id === loadedProject.id ? loadedProject : item));
      synchronized.current.set(loadedProject.id, JSON.stringify(loadedProject));
      serverSnapshots.current.set(loadedProject.id, JSON.stringify(loadedProject));
    }
    setProject(loadedProject);
    void compilePreviewSnapshot(loadedProject).then((compiled) => {
      if (compiled && activeProjectSelection.current === loadedProject.id
        && isCurrentProjectSelection(loadedProject.id, selectionVersion)
        && projectRef.current.id === loadedProject.id
        && JSON.stringify(projectRef.current) === JSON.stringify(loadedProject)) {
        setPreviewProject(compiled);
        setPreviewEpoch((value) => value + 1);
      }
    });
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setSelectedPath("app.js");
    setDatabaseTarget(null);
    setLoadingProjectId("");
    setProjectLoadError("");
  };

  const runPreview = ({ show: reveal = false }: { show?: boolean } = {}) => {
    setOperationError("");
    const snapshot = project;
    void compilePreviewSnapshot(snapshot)
      .then((compiled) => {
        if (!compiled || JSON.stringify(projectRef.current) !== JSON.stringify(snapshot)) return;
        setPreviewProject(compiled);
        setPreviewEpoch((value) => value + 1);
        if (reveal && shownSection !== "code") show("preview");
      })
      .catch((error) => setOperationError(`Build failed: ${error instanceof Error ? error.message : String(error)}`));
  };

  useEffect(() => {
    if (sources.hasSources(project.id)) return;
    let cancelled = false;
    setOperationError("");
    const snapshot = project;
    void compilePreviewSnapshot(snapshot).then((compiled) => {
      if (!cancelled && compiled && projectRef.current.id === snapshot.id && JSON.stringify(projectRef.current) === JSON.stringify(snapshot)) setPreviewProject(compiled);
    });
    return () => { cancelled = true; previewBuildVersion.current++; };
  }, [project.id, sources]);

  projectRef.current = project;
  routeRef.current = route;

  useEffect(() => {
    if (route.screen === "editor" && projects.some((item) => item.id === route.projectId)) document.title = `${project.name} · Konae`;
  }, [route, project.name, projects]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void import("./components/CapabilityWorkspace");
      void import("./components/WhatHappenedView");
      void import("./components/AppSettingsDialog");
    }, 800);
    return () => window.clearTimeout(timer);
  }, []);

  const ensureAgent = () => {
    if (agentRef.current) return Promise.resolve<KonaeAgentHandle | null>(agentRef.current);
    if (!agentLoad.current) {
      agentLoad.current = import("./agent/harness")
        .then(({ createKonaeAgent }) => {
          const handle = createKonaeAgent(projectRef.current, agentSession, keyStore, {
            liveDatabase: () => hostLifecycleRef.current!.lifecycle.appliedHost.exportDatabase(),
            sourceFiles: () => projectRef.current,
            previewErrors: () => hostLifecycleRef.current!.lifecycle.appliedHost.previewErrors(),
            releaseFacts: async () => {
              const current = projectRef.current;
              const deploymentId = deploymentsRef.current[current.id]?.id;
              const facts = await readReleaseFacts(current.id, deploymentId);
              if (!deploymentId) return { ...facts, visibility: current.manifest.visibility === "private" ? "private" : "public", modules: current.modules };
              const read = await platformMcp.call<{ modules: Record<string, { state: { on: boolean; [option: string]: unknown } }> }>("modules_read", { projectId: deploymentId });
              return { ...facts, visibility: current.manifest.visibility === "private" ? "private" : "public", modules: Object.fromEntries(Object.entries(read.modules).map(([name, module]) => [name, module.state])) };
            },
            platform: {
              listTools: (signal) => platformMcp.listTools(signal),
              call: (name, args, signal) => platformMcp.call(name, args, signal),
              deploymentForProject: (projectId) => deploymentsRef.current[projectId]?.id ?? null,
              confirm: (request) => new Promise<boolean>((resolve) => {
                if (request.projectId !== projectRef.current.id) { resolve(false); return; }
                resolvePlatformConfirmation.current?.(false);
                resolvePlatformConfirmation.current = resolve;
                setDeployOpen(false);
                setSettingsOpen(false);
                setAppSettingsOpen(false);
                setPlatformConfirmation(request);
              }),
              publish: (projectId) => new Promise<PublishOutcome>((resolve) => {
                if (projectId !== projectRef.current.id) { resolve({ status: "declined" }); return; }
                resolveAgentPublish.current?.({ status: "declined" });
                resolveAgentPublish.current = resolve;
                resolvePlatformConfirmation.current?.(false);
                resolvePlatformConfirmation.current = null;
                setPlatformConfirmation(null);
                setDeployment(null);
                setDeployError("");
                setDeployOpen(true);
                refreshOwnerSession();
              }),
              onChanged: () => setDomainVersion((version) => version + 1),
            },
          });
          agentRef.current = handle;
          setAgent(handle);
          return handle;
        })
        .catch(() => {
          agentLoad.current = null;
          return null;
        });
    }
    return agentLoad.current;
  };

  useEffect(() => {
    agentRef.current?.retarget(project);
  }, [project.id, project.manifest, project.files]);

  useEffect(() => {
    agentRef.current?.configure(agentSession);
  }, [agent, agentSession]);

  useEffect(() => () => agentRef.current?.dispose(), []);

  useEffect(() => {
    const follow = () => { invalidateProjectSelection(); setRoute(parseRoute(window.location.pathname)); };
    window.addEventListener("popstate", follow);
    return () => window.removeEventListener("popstate", follow);
  }, []);

  useEffect(() => () => {
    retryLoadController.current?.abort();
    projectSelectionVersion.current++;
  }, []);

  useEffect(() => {
    if (route.screen !== "editor") { invalidateProjectSelection(); return; }
    if (route.projectId === project.id && activeProjectSelection.current === route.projectId
      && (!sources.hasSources(project.id) || sources.isLoaded(project.id))) return;
    const selectionVersion = ++projectSelectionVersion.current;
    const target = projects.find((item) => item.id === route.projectId);
    if (!target) {
      if (!missing.some((item) => item.id === route.projectId)) {
        setRoute({ screen: "not-found", what: "project", path: window.location.pathname });
      }
      return;
    }
    let active = true;
    if (sources.hasSources(target.id) && !sources.isLoaded(target.id)) {
      setLoadingProjectId(target.id);
      setProjectLoadError("");
      const controller = new AbortController();
      void sources.loadProject(target.id, controller.signal).then((loadedProject) => {
        if (active && loadedProject) acceptProjectSelection(loadedProject, selectionVersion, true);
        else if (active && isCurrentProjectSelection(target.id, selectionVersion)) setProjectLoadError("This project no longer has saved sources.");
      }).catch((error) => { if (active && isCurrentProjectSelection(target.id, selectionVersion)) setProjectLoadError(error instanceof Error ? error.message : String(error)); });
      return () => { active = false; controller.abort(); };
    }
    acceptProjectSelection(target, selectionVersion);
    return () => { active = false; };
  }, [route, projects, project.id, sources]);

  const arrival = useRef<PanelId>("preview");
  const seenProject = useRef<string | null>(null);
  useEffect(() => {
    if (route.screen !== "editor" || !("projectId" in route) || !projects.some((item) => item.id === route.projectId)) {
      seenProject.current = null;
      return;
    }
    if (seenProject.current === route.projectId) return;
    seenProject.current = route.projectId;
    const target = arrival.current;
    arrival.current = "preview";
    setStage("preview");
    setCompactPanel("preview");
    show(target);
    if (compact && target === "agent") requestAnimationFrame(() => requestAnimationFrame(() => document.getElementById("agent-start")?.focus()));
  }, [route]);

  const applyAgentProposal = async (proposal: AgentProposal): Promise<boolean> => {
    const applied = applyProposal(project, proposal);
    if (!applied) return false;
    const approvedPreview = proposalPreview?.proposal === proposal ? proposalPreview : null;
    if (!approvedPreview || !hostLifecycle.isProposalReady(approvedPreview.id)) return false;
    const next = applied.project;
    const hostApplied = await hostLifecycle.applyProposal(approvedPreview.id, {
      schema: next.files["schema.sql"],
      actions: next.files["actions.json"],
      fileAccess: next.modules?.files?.fileAccess as "app-users" | undefined,
    });
    if (!hostApplied) return false;
    setProposalPreview(null);
    setProposalPreparation(null);
    setProject(next);
    setPreviewProject(approvedPreview.project);
    agentRef.current?.reseed(next.files, next.encodings);
    if (applied.changedFiles[0] && isEditorFile(applied.changedFiles[0])) {
      setActiveFile(applied.changedFiles[0]);
    }
    return true;
  };

  const agentThread = useAgentThread({
    ensureAgent,
    agent,
    sessionConfig: agentSession,
    keyStore,
    threadResetToken,
    keyVersion,
    onApply: applyAgentProposal,
    onDiscard: () => {
      void hostLifecycle.discardProposal(proposalPreview?.id);
      setProposalPreview(null);
      setProposalPreparation(null);
      agentRef.current?.discard();
    },
    providers: PROVIDERS,
    draftKey: project.id,
    draftStorage: workspaceStorage,
  });

  useEffect(() => {
    const proposal = agentThread.state.proposal;
    if (agentThread.footer.kind !== "validated" || !proposal) {
      void hostLifecycle.discardProposal(proposalPreview?.id);
      setProposalPreview(null);
      setProposalPreparation(null);
      return;
    }
    if (proposalPreview?.proposal === proposal && hostLifecycle.isProposalReady(proposalPreview.id)) return;
    const candidate = proposalPreviewProject(project, proposal);
    if (!candidate) {
      void hostLifecycle.discardProposal(proposalPreview?.id);
      setProposalPreview(null);
      setProposalPreparation(null);
      return;
    }
    let current = true;
    setProposalPreview(null);
    setProposalPreparation({ proposal });
    setOperationError("");
    const id = `proposal:${project.id}:${crypto.randomUUID()}`;
    void (async () => {
      try {
        const compiled = await compileProjectInBrowser(candidate);
        if (!current) return;
        const host = await hostLifecycle.prepareProposal(id, {
          schema: candidate.files["schema.sql"],
          actions: candidate.files["actions.json"],
          fileAccess: candidate.modules?.files?.fileAccess as "app-users" | undefined,
        });
        if (!host || !current) {
          await hostLifecycle.discardProposal(id);
          return;
        }
        setProposalPreview({
          id,
          proposal,
          project: {
            ...candidate,
            files: { ...candidate.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script },
          },
        });
        setProposalPreparation(null);
        setStage("preview");
      } catch (error) {
        if (current) setProposalPreparation({ proposal, error: error instanceof Error ? error.message : String(error) });
        await hostLifecycle.discardProposal(id);
      }
    })();
    return () => {
      current = false;
      void hostLifecycle.discardProposal(id);
    };
  }, [agentThread.footer.kind, agentThread.state.proposal, project, hostLifecycle]);

  const activeProposalPreview = proposalPreview?.project.id === project.id
    && proposalPreview.proposal === agentThread.state.proposal
    && hostLifecycle.isProposalReady(proposalPreview.id)
    ? proposalPreview
    : null;
  const activePreviewProject = activeProposalPreview?.project ?? previewProject;
  const activeRuntimeHost = activeProposalPreview ? hostLifecycle.proposalHost ?? hostLifecycle.appliedHost : hostLifecycle.appliedHost;
  const hostRef = useRuntimeBridge(previewFrame, activeRuntimeHost);
  const activePreviewId = activeProposalPreview?.id ?? project.id;
  const previewProblems = usePreviewProblems(hostRef, activePreviewId);
  const proposalReady = Boolean(activeProposalPreview);
  const proposalError = proposalPreparation?.proposal === agentThread.state.proposal ? proposalPreparation.error : undefined;

  const previewDocument = useMemo(() => createPreviewDocument(activePreviewProject), [activePreviewProject]);
  const previewDocumentKey = useMemo(() => documentFingerprint(previewDocument), [previewDocument]);

  const resetProject = () => {
    const fresh = resetWorkspaceProject(project);
    hostLifecycle.appliedHost.reset();
    void hostLifecycle.discardProposal();
    setProject(fresh);
    setPreviewProject(fresh);
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setSelectedPath("app.js");
  };

  const exportProject = async () => {
    setExporting(true);
    setOperationError("");
    setExportAhead(false);
    try {
      const { createProjectExport, exportedMessage } = await import("./lib/export-project");
      const result = await createProjectExport(project, activeDeployment?.id);
      downloadBlob(result.blob, result.filename);
      toast(exportedMessage(result.filename, Boolean(activeDeployment), result.sourcesAhead));
      if (result.sourcesAhead) setExportAhead(true);
    } catch (error) {
      setOperationError(`Export failed: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setExporting(false);
    }
  };

  const exportDatabase = async () => {
    const bytes = databaseTarget === "deployed" && deployedDatabase
      ? await deployedDatabase.exportDatabase()
      : await hostRef.current.exportDatabase();
    const suffix = databaseTarget === "deployed" ? ".deployed" : "";
    downloadBlob(new Blob([bytes as BlobPart], { type: "application/vnd.sqlite3" }), `${project.slug}${suffix}.sqlite`);
  };

  const deploy = async (visibility: DeploymentVisibility, signupMode?: SignupMode) => {
    setPublishing(true);
    setDeployError("");
    try {
      synchronized.current.set(project.id, JSON.stringify(project));
      const observedSourcesRevision = await sources.save(project);
      serverSnapshots.current.set(project.id, JSON.stringify(project));
      const deployment = deploymentsRef.current[project.id];
      const buildProject = deployment
        ? { ...project, modules: Object.fromEntries(Object.entries((await platformMcp.call<{ modules: Record<string, { state: { on: boolean; [option: string]: unknown } }> }>("modules_read", { projectId: deployment.id })).modules).map(([name, module]) => [name, module.state])) }
        : project;
      const artifact = await buildProjectInBrowser(buildProject);
      const initialSignupPolicy = !activeDeployment && signupMode ? { mode: signupMode, allowlist: [] } : undefined;
      const confirmSchema = (change: SchemaChange) => new Promise<boolean>((resolve) => setSchemaChange({ change, resolve }));
      const published = await deployProject(project, visibility, artifact, confirmSchema, initialSignupPolicy, observedSourcesRevision, deploymentsRef.current[project.id] ? undefined : project.modules);
      const record = published.receipt;
      const releasedProject = { ...project, manifest: record.manifest };
      const releasedSnapshot = JSON.stringify(releasedProject);
      sources.markPublished(project.id, published.sourcesRevision, releasedProject);
      serverSnapshots.current.set(project.id, releasedSnapshot);
      synchronized.current.set(project.id, releasedSnapshot);
      setProject((current) => current.id === project.id && JSON.stringify(current) === JSON.stringify(project) ? releasedProject : current);
      setProjects((current) => current.map((item) => item.id === project.id && JSON.stringify(item) === JSON.stringify(project) ? releasedProject : item));
      setExportAhead(false);
      setDeployment(record);
      setDeployments((current) => ({ ...current, [project.id]: record }));
      resolveAgentPublish.current?.({ status: "published", url: record.url, visibility: record.manifest.visibility });
      resolveAgentPublish.current = null;
    } catch (error) {
      if (error instanceof SchemaChangeDeclined) return;
      if (isSignedOutError(error)) refreshOwnerSession();
      setDeployError(error instanceof Error ? error.message : String(error));
      setDeployErrorCode(error instanceof McpError ? error.refusal?.code : undefined);
    } finally {
      setPublishing(false);
    }
  };

  const openEditorRoute = (projectId: string) => {
    invalidateProjectSelection();
    const next = { screen: "editor" as const, projectId };
    setRoute(next);
    pushRoute(next);
  };

  const leftProject = useRef<string | undefined>(undefined);
  if (route.screen === "editor") leftProject.current = route.projectId;
  const goToProjects = () => {
    invalidateProjectSelection();
    const next = { screen: "projects" as const };
    setRoute(next);
    pushRoute(next);
  };

  const retryProjectLoad = () => {
    if (route.screen !== "editor") return;
    const projectId = route.projectId;
    retryLoadController.current?.abort();
    const controller = new AbortController();
    retryLoadController.current = controller;
    const selectionVersion = ++projectSelectionVersion.current;
    setProjectLoadError("");
    void sources.loadProject(projectId, controller.signal).then((loadedProject) => {
      if (!isCurrentProjectSelection(projectId, selectionVersion)) return;
      if (loadedProject) acceptProjectSelection(loadedProject, selectionVersion, true);
      else setProjectLoadError("This project no longer has saved sources.");
    }).catch((error) => {
      if (!controller.signal.aborted && isCurrentProjectSelection(projectId, selectionVersion)) {
        setProjectLoadError(error instanceof Error ? error.message : String(error));
      }
    }).finally(() => {
      if (retryLoadController.current === controller) retryLoadController.current = null;
    });
  };

  const openProject = (projectId: string, entry: "preview" | "agent" | "code" = "preview") => {
    const next = projects.find((item) => item.id === projectId);
    if (!next) { if (missing.some((item) => item.id === projectId)) openEditorRoute(projectId); return; }
    arrival.current = entry === "code" ? "code" : entry === "agent" ? "agent" : "preview";
    if (!sources.hasSources(projectId) || sources.isLoaded(projectId)) {
      setProject(next);
      setThreadResetToken((token) => token + 1);
      setActiveFile(entry === "code" ? "app.js" : "index.html");
      setSelectedPath(entry === "code" ? "app.js" : "index.html");
      setDatabaseTarget(null);
    }
    openEditorRoute(next.id);
  };

  const createProject = (name: string, template: TemplateId | "scratch" = defaultTemplate, description?: string) => {
    const described = description?.trim() ?? "";
    arrival.current = template === "scratch" || described ? "agent" : "preview";
    const base = createWorkspaceProject(name, projects);
    if (described) workspaceStorage.setItem(describeDraftKey(base.id), described);
    const next = template === "scratch" ? plainListProject(base, described) : applyTemplate(base, template);
    rememberCreatedDigest(workspaceStorage, next);
    setProjects((current) => [...current, next]);
    setProject(next);
    setPreviewProject(next);
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setSelectedPath("app.js");
    setDatabaseTarget(null);
    openEditorRoute(next.id);
  };

  const deleteMissingProject = async (item: ProjectMetadata, name: string) => {
    await sources.deleteProject(item.id, name);
    setMissing((current) => current.filter((entry) => entry.id !== item.id));
    setDeployments((current) => {
      const { [item.id]: _removed, ...rest } = current;
      return rest;
    });
  };

  const deleteEditableProject = async (item: ProjectMetadata, name: string) => {
    await sources.deleteProject(item.id, name);
    synchronized.current.delete(item.id);
    setProjects((current) => current.filter((entry) => entry.id !== item.id));
    setDeployments((current) => {
      const { [item.id]: _removed, ...rest } = current;
      return rest;
    });
  };

  const setActionAccess = (name: string, access: RuntimeActionAccess) => {
    let parsed: { actions?: Record<string, { access?: string }> };
    try {
      parsed = JSON.parse(project.files["actions.json"] ?? emptyActionProgram);
    } catch {
      setOperationError("actions.json is not valid JSON, so who can run it cannot be changed from here.");
      return;
    }
    if (!parsed.actions?.[name]) return;
    parsed.actions[name].access = access;
    const source = JSON.stringify(parsed, null, 2);
    let program;
    try {
      program = parseServerActionSource(source);
    } catch (error) {
      setOperationError(error instanceof Error ? error.message : String(error));
      return;
    }
    const next = {
      ...project,
      files: { ...project.files, "actions.json": source },
      updatedAt: Date.now(),
    };
    setProject(next);
    setPreviewProject(next);
  };

  const setVisibility = (visibility: "public" | "private") => {
    const next = { ...project, manifest: { ...project.manifest, visibility }, updatedAt: Date.now() };
    setProject(next);
    setPreviewProject(next);
  };

  if (route.screen === "not-found") return <NotFound what={route.what} path={route.path} />;

  if (screen === "projects") {
    return <ProjectDashboard projects={projects} deployments={deployments} ownerSession={ownerSession} missing={missing} onDeleteMissing={deleteMissingProject} onDeleteProject={deleteEditableProject} onOpenProject={openProject} onCreateProject={createProject} modelConnected={agentThread.live} returnFocusTo={leftProject.current} />;
  }

  if (screen === "editor" && missing.some((item) => item.id === route.projectId)) return <main className="platform-page"><div className="platform-main"><section className="platform-card" aria-live="polite"><h1>Could not open this app</h1><p role="alert">This app has no readable app/ source folder. The editor did not replace its files.</p><Button onClick={() => window.location.reload()}>Try again</Button><Button variant="secondary" onClick={goToProjects}>Projects</Button></section></div></main>;

  if (screen === "editor" && loadingProjectId === route.projectId) {
    return <main className="platform-page"><div className="platform-main"><section className="platform-card" aria-live="polite"><h1>{projectLoadError ? "Could not open this project" : "Opening project"}</h1>{projectLoadError ? <><p role="alert">{projectLoadError}</p><Button onClick={retryProjectLoad}>Try again</Button><Button variant="secondary" onClick={goToProjects}>Projects</Button></> : <p>Loading the app files…</p>}</section></div></main>;
  }

  const signInHref = platformAccessHref(window.location.pathname);

  const actionsRefusal = actionReadout(project.files["actions.json"] ?? emptyActionProgram, project.manifest.visibility).refusal;
  const quietDeploy = asCreated && agentThread.draft.trim() !== "";
  const sections: { id: PanelId; label: string; icon: typeof Code2 }[] = [
    { id: "preview", label: "Preview", icon: Play },
    { id: "agent", label: "Describe", icon: Sparkles },
    { id: "code", label: "Files", icon: Code2 },
    { id: "data", label: "Data", icon: Database },
    { id: "auth", label: "Access", icon: UserRound },
    { id: "files", label: "Uploads", icon: FolderOpen },
    { id: "activity", label: "What happened", icon: Activity },
  ];
  const navItems: TabBarItem<PanelId>[] = sections
    .filter((section) => !(section.id === "agent" && wide))
    .map((section) => {
      const Icon = section.icon;
      return {
        id: section.id,
        label: section.label,
        icon: <Icon size={14} aria-hidden="true" />,
        tabId: `section-${section.id}`,
        badge: section.id === "code" && actionsRefusal ? "refused" : undefined,
      };
    });
  const usageHref = activeDeployment ? `/usage?deployment=${encodeURIComponent(activeDeployment.id)}` : "/usage";

  const reveal = show;

  const openRefusal = () => {
    const source = projectRef.current.files["actions.json"] ?? emptyActionProgram;
    const spot = refusedSpot(source, actionReadout(source, projectRef.current.manifest.visibility).refusal);
    setActiveFile("actions.json");
    setSelectedPath("actions.json");
    reveal("code");
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const editor = editorRef.current;
      if (!editor) return;
      editor.focus();
      const end = spot?.value?.end ?? spot?.index ?? 0;
      editor.setSelectionRange(end, end);
      const before = editor.value.slice(0, end).split("\n");
      const style = getComputedStyle(editor);
      const height = parseFloat(style.lineHeight) || 20;
      editor.scrollTop = Math.max(0, (before.length - 1) * height - editor.clientHeight / 3);
      const measure = document.createElement("canvas").getContext("2d");
      if (measure) {
        measure.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
        const endX = measure.measureText(before.at(-1) ?? "").width + (parseFloat(style.paddingLeft) || 0);
        editor.scrollLeft = Math.max(0, endX - editor.clientWidth + 48);
      }
    }));
  };

  const filesShowing = shownSection === "code";

  const toggleExplorer = ({ focus = false }: { focus?: boolean } = {}) => {
    if (explorerOpen && filesShowing) {
      setExplorerOpen(false);
      return;
    }
    setExplorerOpen(true);
    if (!filesShowing) reveal("code");
    if (focus) requestAnimationFrame(() => document.getElementById("explorer-new-file")?.focus());
  };

  const runtimeProblems = previewProblems.filter((problem) => problem.kind === "error" || problem.kind === "unhandledrejection").length;
  const schemaProblem = previewProblems.find((problem) => problem.kind === "schema");

  const indentWithTab = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Escape") {
      codeTabEscape.current = true;
      return;
    }
    if (event.key !== "Tab" || event.metaKey || event.ctrlKey || event.altKey) {
      codeTabEscape.current = false;
      return;
    }
    if (codeTabEscape.current) {
      codeTabEscape.current = false;
      return;
    }
    event.preventDefault();
    const element = event.currentTarget;
    const { selectionStart: start, selectionEnd: end, value } = element;
    let next: string;
    let caret: [number, number];
    if (event.shiftKey) {
      const lineStart = value.lastIndexOf("\n", start - 1) + 1;
      const removed = value.startsWith("  ", lineStart) ? 2 : value.startsWith(" ", lineStart) ? 1 : 0;
      if (!removed) return;
      next = value.slice(0, lineStart) + value.slice(lineStart + removed);
      caret = [Math.max(lineStart, start - removed), Math.max(lineStart, end - removed)];
    } else {
      next = `${value.slice(0, start)}  ${value.slice(end)}`;
      caret = [start + 2, start + 2];
    }
    updateFile(next);
    requestAnimationFrame(() => editorRef.current?.setSelectionRange(...caret));
  };

  const fileLead = ["actions.json", "schema.sql", "index.html", "styles.css", "app.js"];
  const explorerFiles = [...new Set([...Object.keys(project.files), ACTION_CONTRACT_MARKDOWN])].sort((a, b) => {
    const ai = fileLead.indexOf(a);
    const bi = fileLead.indexOf(b);
    return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi) || a.localeCompare(b);
  });
  const canEditSelection = Boolean(selectedPath) && !protectedProjectPaths.has(selectedPath) && selectedPath !== ACTION_CONTRACT_MARKDOWN;

  const explorerNav = (
    <nav className="explorer-panel" id="file-list" aria-label="Project files">
      <div className="explorer-heading">
        <button id="explorer-new-file" aria-label="New file" title="New file" onClick={() => startNewEntry("file")}><FilePlus2 size={12} /></button>
        <button aria-label="New folder" title="New folder" onClick={() => startNewEntry("folder")}><FolderPlus size={12} /></button>
        <button aria-label="Rename selected path" title="Rename selected path" disabled={!canEditSelection} onClick={startRename}><Pencil size={12} /></button>
        <button aria-label="Delete selected path" title="Delete selected path" disabled={!canEditSelection} onClick={() => setPendingDelete(selectedPath)}><Trash2 size={12} /></button>
      </div>
      {newEntry && <div className="explorer-create">
        <label htmlFor="new-project-path">{newEntry === "file" ? "File path" : "Folder path"}</label>
        <input id="new-project-path" autoFocus value={newEntryPath} aria-invalid={treeError ? true : undefined} aria-describedby={treeError ? "tree-error" : undefined} placeholder={newEntry === "file" ? "src/features/tasks.ts" : "src/features"} onChange={(event) => { setNewEntryPath(event.target.value); setTreeError(""); }} onKeyDown={(event) => { if (event.key === "Enter") addTreeEntry(); if (event.key === "Escape") setNewEntry(null); }} />
        <button onClick={addTreeEntry}>Create</button>
        <button onClick={() => setNewEntry(null)}>Cancel</button>
        {treeError && <span id="tree-error" role="alert">{treeError}</span>}
      </div>}
      {renaming && <div className="explorer-create">
        <label htmlFor="rename-project-path">New path for {selectedPath}</label>
        <input id="rename-project-path" autoFocus value={renameValue} aria-invalid={treeError ? true : undefined} aria-describedby={treeError ? "tree-error" : undefined} onChange={(event) => { setRenameValue(event.target.value); setTreeError(""); }} onKeyDown={(event) => { if (event.key === "Enter") renameSelectedPath(); if (event.key === "Escape") setRenaming(false); }} />
        <button onClick={renameSelectedPath}>Rename</button>
        <button onClick={() => setRenaming(false)}>Cancel</button>
        {treeError && <span id="tree-error" role="alert">{treeError}</span>}
      </div>}
      <ExplorerTree
        files={explorerFiles}
        directories={project.directories}
        activeFile={activeFile}
        selectedPath={selectedPath}
        iconFor={(path) => fileMetaFor(path).icon}
        noteFor={(path) => path === "actions.json" ? "who may read and write" : path === "schema.sql" ? "the tables" : path === ACTION_CONTRACT_MARKDOWN ? "generated" : ""}
        onSelect={setSelectedPath}
        onOpenFile={(file) => { setActiveFile(file); setSelectedPath(file); }}
      />
    </nav>
  );

  const codePanel: PanelRender = {
    id: "code",
    title: activeMeta.label,
    icon: <Code2 size={11} />,
    actions: (
      <>
        <select className={`file-switch ${compact && explorerFiles.length > 4 ? "is-overflow" : ""}`} aria-label="Open file" value={activeFile} onChange={(event) => { setActiveFile(event.target.value); setSelectedPath(event.target.value); }}>
          {explorerFiles.map((file) => <option key={file} value={file}>{file}</option>)}
        </select>
        <span className="panel-meta">{activeFile === "actions.json" ? `${listedActions(activeSource)} action${listedActions(activeSource) === 1 ? "" : "s"}` : activeEncoded ? formatBytes(sourceBytes(project, activeFile).length) : `${activeMeta.language} · ${lines} lines`}</span>
        <button
          className={`file-list-toggle ${explorerOpen ? "is-open" : ""}`}
          aria-expanded={explorerOpen}
          aria-controls={explorerOpen ? "file-list" : undefined}
          title="Show the project's files, with New file and New folder (Ctrl+B or Cmd+B)"
          onClick={() => toggleExplorer()}
        >
          <FolderTree size={11} /> File list
        </button>
        <button className="active" onClick={() => runPreview({ show: true })}><Play size={11} fill="currentColor" /> {activeFile === "schema.sql" || activeFile === "actions.json" ? "Check" : "Run"}</button>
      </>
    ),
    body: (
      <div className={`code-workspace ${explorerOpen ? "has-file-list" : ""}`}>
        {explorerOpen && explorerNav}
        <div className="code-panel">
          {compact && explorerFiles.length > 4 && (
            <ul className="compact-file-list" aria-label="Project files">
              {explorerFiles.map((file) => (
                <li key={file}>
                  <button
                    type="button"
                    className={file === activeFile ? "active" : undefined}
                    aria-current={file === activeFile ? "page" : undefined}
                    onClick={() => { setActiveFile(file); setSelectedPath(file); }}
                  >
                    {file}
                  </button>
                </li>
              ))}
            </ul>
          )}
          {activeFile === "actions.json" && <ActionReadout source={activeSource} visibility={project.manifest.visibility} onJump={openRefusal} />}
          {activeFile !== "actions.json" && actionsRefusal && <Alert tone="danger" className="code-refusal" title="actions.json is refused" action={<Button size="sm" onClick={openRefusal}>{refusedLabel(project.files["actions.json"] ?? "", actionsRefusal)}</Button>}>{actionsRefusal}</Alert>}
          {contractOpen ? <ActionContractView state={actionContract} /> : activeEncoded ? <Alert className="code-not-text" title={`${activeFile} is not text`}>The editor saves its {formatBytes(sourceBytes(project, activeFile).length)} with the app as they are, and cannot change them. To replace it, export the app, put the new file in the folder's app/, and import the folder with Sources only, in App settings under Import.</Alert> : <div className="code-area">
            <div className="line-numbers" aria-hidden="true" ref={gutterRef}>
              {Array.from({ length: lines }, (_, index) => <span key={index}>{index + 1}</span>)}
            </div>
            <textarea
              ref={editorRef}
              value={activeSource}
              onChange={(event) => updateFile(event.target.value)}
              onKeyDown={indentWithTab}
              aria-describedby="code-editor-keys"
              onScroll={(event) => {
                if (gutterRef.current) gutterRef.current.scrollTop = event.currentTarget.scrollTop;
              }}
              spellCheck={false}
              aria-label={`${activeFile} code editor`}
            />
          </div>}
          <p id="code-editor-keys" className="editor-keys">Tab inserts two spaces. Press Escape, then Tab, to leave the editor.</p>
          {(runtimeProblems > 0 || schemaProblem) && (
            <footer className="editor-status">
              <span className={schemaProblem ? "is-failing" : ""} title={schemaProblem?.message}>
                <Wifi size={12} /> {schemaProblem ? "schema.sql failed" : `${runtimeProblems} preview error${runtimeProblems === 1 ? "" : "s"}`}
              </span>
            </footer>
          )}
        </div>
      </div>
    ),
  };

  const capabilityPanel = (id: "data" | "files" | "auth", label: string, icon: ReactNode): PanelRender => ({
    id,
    title: label,
    icon,
    body: visited.has(id) && (
        <CapabilityWorkspace
          fallback={<LoadingBlock label="Loading this panel" rows={4} className="panel-loading" />}
          view={id === "data" ? "database" : id}
          host={hostRef.current}
          onEditSchema={() => { setActiveFile("schema.sql"); reveal("code"); }}
          onExportDatabase={() => void exportDatabase()}
          onFilesChanged={() => runPreview()}
          deployedDatabase={deployedDatabase}
          databaseTarget={databaseTarget}
          deployedLabel={activeDeployment?.url.replace(/^https?:\/\//, "")}
          onDatabaseTargetChange={setDatabaseTarget}
          actionsSource={project.files["actions.json"]}
          onSetAccess={setActionAccess}
          visibility={project.manifest.visibility}
          onSetVisibility={setVisibility}
          projectId={project.id}
          deployment={activeDeployment ? { id: activeDeployment.id, url: activeDeployment.url } : undefined}
          authView={authView}
          onAuthViewChange={setAuthView}
          inviteFocus={inviteFocus}
          accessUndeployed={accessNotDeployed(project, activeDeployment)}
          onDeploy={() => { setDeployment(null); setDeployError(""); setDeployOpen(true); refreshOwnerSession(); }}
        />
    ),
  });

  const activityPanel: PanelRender = {
    id: "activity",
    title: "What happened",
    icon: <Activity size={11} />,
    body: visited.has("activity") && (
        <WhatHappenedView
          fallback={<LoadingBlock label="Loading this panel" rows={4} className="panel-loading" />}
          deployment={activeDeployment ? { id: activeDeployment.id, url: activeDeployment.url } : undefined}
          onOpen={(place) => {
            if (place === "capsule") { setActiveFile("actions.json"); reveal("code"); return; }
            setAuthView(place);
            reveal("auth");
          }}
        />
    ),
  };

  const previewTools = (
    <div className="preview-tools" role="group" aria-label="Preview">
      <IconButton label="Desktop preview" aria-pressed={viewport === "desktop"} onClick={() => setViewport("desktop")}><Laptop size={14} /></IconButton>
      <IconButton label="Mobile preview" aria-pressed={viewport === "mobile"} onClick={() => setViewport("mobile")}><Smartphone size={14} /></IconButton>
      <IconButton label="Reload preview" onClick={() => runPreview()}><RefreshCw size={14} /></IconButton>
    </div>
  );

  const previewPanel: PanelRender = {
    id: "preview",
    title: project.name,
    icon: <Play size={12} />,
    body: (
      <div className="preview-panel-body">
        <p className="preview-caption">
          {activeDeployment
            ? <>This preview uses test rows. The live app has {liveRows === null ? "its own rows" : `${liveRows} row${liveRows === 1 ? "" : "s"}`}. <button type="button" className="link-button" onClick={() => { setDatabaseTarget("deployed"); show("data"); }}>See live rows</button></>
            : <>This preview uses test rows. Deploy to give the app its own address and rows.</>}
        </p>
        {previewProblems.length > 0 && (
          <section className="preview-problems" aria-label="Preview problems" aria-live="polite">
            <header>
              <CircleAlert size={13} />
              <strong>{previewProblems.length} problem{previewProblems.length === 1 ? "" : "s"} in the preview</strong>
              <button onClick={() => runPreview()}><RefreshCw size={11} /> Rerun</button>
            </header>
            <ul>
              {previewProblems.slice(0, 4).map((problem, index) => (
                <li key={`${problem.kind}:${index}`}>
                  <em>{problemLabels[problem.kind]}</em>
                  <span title={problem.stack || problem.message}>{problem.message}</span>
                  {problem.count > 1 && <small>×{problem.count}</small>}
                </li>
              ))}
            </ul>
            {previewProblems.length > 4 && <p>{previewProblems.length - 4} more not shown.</p>}
          </section>
        )}
        <div className={`preview-canvas ${viewport}`}>
          <iframe key={`${activePreviewId}:${previewEpoch}:${previewDocumentKey}`} ref={setPreviewFrame} title="Mini-app preview" sandbox="allow-scripts allow-forms" srcDoc={previewDocument} />
        </div>
      </div>
    ),
  };

  const agentBody = <AgentPanel thread={agentThread} sessionConfig={agentSession} onOpenSettings={() => setSettingsOpen(true)} suggestions={suggestedChanges(project)} proposalReady={proposalReady} proposalError={proposalError} />;
  const agentPanel: PanelRender = {
    id: "agent",
    title: "Describe",
    icon: <Sparkles size={12} />,
    body: agentBody,
  };
  const describedRefusal = agentThread.footer.kind === "blocked" && !(rail || shownSection === "agent");
  const openDeploy = () => {
    setDeployment(null); setDeployError(""); setDeployOpen(true); refreshOwnerSession();
    if (activeDeployment) void platformMcp.call<{ deployment: DeploymentReceipt | null }>("project_inspect", { projectId: project.id })
      .then((inspected) => { if (inspected.deployment) setDeployments((current) => ({ ...current, [project.id]: inspected.deployment! })); }, () => {});
  };
  const moreItems: MenuItem[] = [
    ...(compact && activeDeployment ? [{ label: "Open the app", icon: <ExternalLink size={14} />, onSelect: () => { window.open(activeDeployment.url, "_blank", "noopener,noreferrer"); } }] : []),
    { label: exporting ? "Exporting…" : "Export", icon: <Download size={14} />, disabled: exporting, onSelect: () => void exportProject() },
    { label: "App settings", icon: <Settings2 size={14} />, onSelect: () => setAppSettingsOpen(true) },
    { label: ownerSession.state === "signed-in" ? "Owner account" : "Owner sign-in", icon: <UserRound size={14} />, onSelect: () => { window.location.href = signInHref; } },
    { label: "Start over…", icon: <RotateCcw size={14} />, danger: true, onSelect: () => setResetOpen(true) },
  ];
  const crumbProjects = switcherProjects(projects, missing, deployments);

  return (
    <div className={`workshop ${compact ? "is-compact" : ""} ${rail ? "has-rail" : ""} ${railFolded ? "has-rail-strip" : ""} stage-${shownSection}`}>
      <header className="topbar">
        <a className="brand" href="/" aria-label="Konae, projects" onClick={(event) => {
          if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
          event.preventDefault();
          goToProjects();
        }}>
          <span className="brand-mark"><Box size={17} strokeWidth={2.4} /></span>
          <span>konae</span>
        </a>

        <div className="topbar-place">
          <ProjectCrumb
            current={{ id: project.id, name: project.name }}
            projects={crumbProjects}
            onProjects={goToProjects}
            onOpen={(id) => projects.some((item) => item.id === id) ? openProject(id) : openEditorRoute(id)}
          />
          <AppStatus className="topbar-status" url={activeDeployment?.url} publishedAt={activeDeployment ? releasedAt(activeDeployment) : undefined} />
          {undeployed && <UndeployedMarker className="topbar-changes" onDeploy={openDeploy} />}
        </div>

        <div className="top-actions">
          {exporting && <span className="save-state" role="status"><RefreshCw size={12} className="spin" /> Exporting</span>}
          {!saved && <span
            className={`save-state ${saveError ? "is-failed" : ""}`}
            title={saveError || undefined}
            role={saveError ? "alert" : undefined}
          >
            {saveError ? <CircleAlert size={12} /> : <RefreshCw size={12} className="spin" />}
            {saveError ? "Not saved" : "Saving"}
          </span>}
          {saveError && <Button onClick={retrySave} disabled={pendingSaves.current > 0}>Retry save</Button>}
          <Button variant={quietDeploy ? "secondary" : "primary"} className={`deploy-button${quietDeploy ? " is-quiet" : ""}`} title={actionsRefusal ? `Deploy is refused: ${actionsRefusal}` : undefined} onClick={openDeploy}><Rocket size={15} aria-hidden="true" /> <span>{activeDeployment ? "Deploy changes" : "Deploy"}</span></Button>
          <Menu label="More" trigger={<><MoreHorizontal size={15} aria-hidden="true" /><span className="more-label">More</span></>} triggerClassName="more-button" items={moreItems} />
          <AccountChip session={ownerSession} href={signInHref} />
        </div>
      </header>

      <nav className="section-bar" aria-label="Workspace sections">
        <TabBar
          label="Sections"
          items={compact ? navItems.filter((item) => item.id === "preview" || item.id === "agent" || item.id === compactPanel) : navItems}
          value={shownSection}
          onChange={(id) => show(id)}
          end={<>
            {!compact && (shownSection === "preview" || shownSection === "code") && previewTools}
            {compact
              ? <Menu label="Tools" align="end" triggerClassName="tools-button" trigger={<><FolderTree size={14} aria-hidden="true" /> Tools</>} items={[
                ...sections.filter((section) => section.id !== "preview" && section.id !== "agent").map((section) => ({ label: section.label, current: compactPanel === section.id, onSelect: () => show(section.id) })),
                { label: "Usage", onSelect: () => { window.location.href = usageHref; } },
              ]} />
              : <a className="usage-link" href={usageHref}><BarChart3 size={14} aria-hidden="true" /> Usage</a>}
          </>}
        />
      </nav>

      <div className="stage-notices">
        {operationError && <Alert tone="danger" action={<Button size="sm" onClick={() => setOperationError("")}>Dismiss</Button>}>{operationError}</Alert>}
        {sourceConflict && <Alert tone="warning" action={<Button size="sm" onClick={() => void reloadConflictingSources()}>Reload saved sources</Button>}>This app changed on another machine. Your draft is still here. Reload the saved sources to continue.</Alert>}
        {exportAhead && <Alert tone="warning" title="The export runs your last deploy" action={<Button size="sm" onClick={() => setExportAhead(false)}>Dismiss</Button>}>The folder holds your saved files. An app started from it runs your last deploy. Deploy changes, then export again, to make them match.</Alert>}
        {describedRefusal && agentThread.footer.kind === "blocked" && (
          <Alert tone="danger" title="A check refused the proposed change" action={<Button size="sm" onClick={() => show("agent")}>Open Describe</Button>}>
            {agentThread.footer.message}{agentThread.footer.detail && <span className="gate-refusal-detail">{agentThread.footer.detail}</span>}
          </Alert>
        )}
        {agentThread.footer.kind === "validated" && agentThread.footer.owner && (
          <Alert tone="warning" title="Apply works. Deploy waits for a secret" action={<Button size="sm" onClick={() => { setAuthView("integrations"); show("auth"); }}>Open Access &gt; Integrations</Button>}>
            {agentThread.footer.owner.message}{agentThread.footer.owner.detail && <span className="gate-refusal-detail">{agentThread.footer.owner.detail}</span>}
          </Alert>
        )}
      </div>

      <WorkbenchLayout layout={displayedLayout} compact={compact} onLayoutChange={updateLayout} panels={[codePanel, capabilityPanel("data", "Data", <Database size={11} />), capabilityPanel("files", "Uploads", <FolderOpen size={11} />), capabilityPanel("auth", "Access", <UserRound size={11} />), activityPanel, previewPanel, ...(rail ? [] : [agentPanel])]} />

      {rail && (
        <aside className="describe-rail" aria-labelledby="describe-rail-title">
          <header className="describe-rail-head">
            <h2 id="describe-rail-title"><Sparkles size={14} aria-hidden="true" /> Describe</h2>
            <span className="describe-rail-tools">
              <IconButton label="Model settings" onClick={() => setSettingsOpen(true)}><Settings2 size={14} /></IconButton>
              <IconButton id="describe-rail-fold" label="Fold Describe" aria-expanded onClick={() => foldRail(false)}><PanelRightClose size={14} /></IconButton>
            </span>
          </header>
          {agentBody}
        </aside>
      )}
      {railFolded && (
        <aside className="describe-strip" aria-label="Describe">
          <IconButton id="describe-rail-open" label="Open Describe" aria-expanded={false} onClick={() => foldRail(true)}><PanelRightOpen size={14} /></IconButton>
        </aside>
      )}

      <ToastRegion toasts={toasts} onDismiss={(id) => setToasts((current) => current.filter((item) => item.id !== id))} />

      <AgentSettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        sessionConfig={agentSession}
        onSessionConfigChange={setAgentSession}
        keyStore={keyStore}
        onKeyStoreChange={() => setKeyVersion((version) => version + 1)}
        providers={PROVIDERS}
      />

      {appSettingsOpen && <AppSettingsDialog fallback={<Dialog title="App settings" onClose={() => setAppSettingsOpen(false)}><div className="ui-dialog-body"><LoadingBlock label="Loading app settings" rows={3} /></div></Dialog>} key={`${project.id}-${domainVersion}`} projectName={project.name} modules={project.modules} files={Object.keys(project.files)} deploymentId={activeDeployment?.id} importTarget={activeDeployment ? { project: { id: project.id, name: project.name, slug: project.slug }, visibility: activeDeployment.manifest.visibility, onImported: () => window.location.reload() } : undefined} onClose={() => setAppSettingsOpen(false)} onSignedOut={refreshOwnerSession} onAddressMoved={(url) => setDeployments((current) => current[project.id] ? { ...current, [project.id]: { ...current[project.id]!, url } } : current)} signInHref={signInHref} onExport={() => { setAppSettingsOpen(false); void exportProject(); }} />}

      {platformConfirmation && <Dialog title="Allow the agent to do this?" description={project.name} onClose={() => { resolvePlatformConfirmation.current?.(false); resolvePlatformConfirmation.current = null; setPlatformConfirmation(null); }} footer={<>
        <Button onClick={() => { resolvePlatformConfirmation.current?.(false); resolvePlatformConfirmation.current = null; setPlatformConfirmation(null); }}>Cancel</Button>
        <Button variant="primary" onClick={() => { resolvePlatformConfirmation.current?.(true); resolvePlatformConfirmation.current = null; setPlatformConfirmation(null); }}>Allow</Button>
      </>}><div className="ui-dialog-body"><p style={{ overflowWrap: "anywhere" }}>{platformConfirmation.summary}</p><p>It runs as you, on this app only, through <code>{platformConfirmation.tool}</code>.</p></div></Dialog>}

      {deployOpen && <DeployDialog project={project} deployment={deployment} unchanged={sameAsLive} ownerEmail={ownerSession.email} publishing={publishing} error={deployError} errorCode={deployErrorCode} session={ownerSession} signInHref={signInHref} onClose={() => { resolveAgentPublish.current?.(deployError ? { status: "failed", error: deployError } : { status: "declined" }); resolveAgentPublish.current = null; setDeployOpen(false); }} current={activeDeployment} onOpenAppSettings={() => { setDeployOpen(false); setAppSettingsOpen(true); }} onDeploy={(visibility, signupMode) => void deploy(visibility, signupMode)} onEditAuth={() => { setDeployOpen(false); setAuthView("rules"); reveal("auth"); }} onOpenActions={() => { setDeployOpen(false); openRefusal(); }} onOpenUsers={() => { setDeployOpen(false); setAuthView("users"); setInviteFocus((count) => count + 1); reveal("auth"); }} onOpenIntegrations={() => { setDeployOpen(false); setAuthView("integrations"); reveal("auth"); }} />}
      {/* After the Deploy dialog, so it paints over it. */}
      {schemaChange && <SchemaChangeDialog change={schemaChange.change} onAnswer={(confirmed) => { schemaChange.resolve(confirmed); setSchemaChange(null); }} />}

      <Dialog
        open={resetOpen}
        title={`Start ${project.name} over?`}
        description={`Every file of ${project.name} goes back to the first example app, and the test rows and uploads in this browser are deleted. The published app keeps its files, rows and accounts.`}
        onClose={() => setResetOpen(false)}
        footer={<>
          <Button onClick={() => setResetOpen(false)}>Cancel</Button>
          <Button variant="danger" className="reset-confirm" onClick={() => { setResetOpen(false); resetProject(); }}><RotateCcw size={14} /> Start over</Button>
        </>}
      >
        {null}
      </Dialog>

      <Dialog
        open={pendingDelete !== null}
        title={pendingDelete && project.directories.includes(pendingDelete) ? "Delete this folder?" : "Delete this file?"}
        description={describeDelete(project, pendingDelete)}
        onClose={() => setPendingDelete(null)}
        footer={<>
          <Button onClick={() => setPendingDelete(null)}>Cancel</Button>
          <Button variant="danger" onClick={() => pendingDelete && deletePath(pendingDelete)}><Trash2 size={14} /> Delete</Button>
        </>}
      >
        {null}
      </Dialog>
    </div>
  );
}

const problemLabels: Record<string, string> = {
  actions: "actions.json",
  schema: "schema.sql",
  error: "Runtime error",
  unhandledrejection: "Unhandled rejection",
};

const workbenchPanels: PanelId[] = panelIds.filter((id) => id !== "explorer");

function listedActions(source: string): number {
  try {
    const actions = (JSON.parse(source) as { actions?: unknown }).actions;
    return actions && typeof actions === "object" ? Object.keys(actions).length : 0;
  } catch {
    return 0;
  }
}

function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const media = window.matchMedia(query);
    const follow = () => setMatches(media.matches);
    follow();
    media.addEventListener("change", follow);
    return () => media.removeEventListener("change", follow);
  }, [query]);
  return matches;
}

function useLiveRowCount(database: HostedDatabaseClient | undefined, active: boolean): number | null {
  const [rows, setRows] = useState<number | null>(null);
  useEffect(() => {
    setRows(null);
  }, [database]);
  useEffect(() => {
    if (!database || !active) return;
    let live = true;
    database.inspectDatabase().then((tables) => { if (live) setRows(tables.reduce((sum, table) => sum + table.totalRows, 0)); }, () => undefined);
    return () => { live = false; };
  }, [database, active]);
  return rows;
}

function refusedLabel(source: string, refusal: string): string {
  const spot = refusedSpot(source, refusal);
  return spot ? `Go to ${spot.action}` : "Go to actions.json";
}

function ActionReadout({ source, visibility, onJump }: { source: string; visibility: "public" | "private"; onJump: () => void }) {
  const { refusal, lines } = actionReadout(source, visibility);
  if (refusal) return <Alert tone="danger" className="action-readout-block" title="Deploy refuses this file" action={refusedSpot(source, refusal) ? <Button size="sm" onClick={onJump}>{refusedLabel(source, refusal)}</Button> : undefined}>{refusal}</Alert>;
  return (
    <details className="action-readout-block">
      <summary>What each action does</summary>
      <p className="action-readout-note">Every list has a row limit, so the app can move to another machine.</p>
      {lines.length > 0 && (
        <ul className="action-readout">
          {lines.map((line) => <li key={line}>{line}</li>)}
        </ul>
      )}
    </details>
  );
}

function describeDelete(project: MiniAppProject, path: string | null): string {
  if (!path) return "";
  if (!project.directories.includes(path)) return `${path} will be removed from the project.`;
  const count = filesUnder(project.files, path).length;
  return count === 0
    ? `${path} is empty. Deleting it removes only the folder.`
    : `${path} and the ${count} file${count === 1 ? "" : "s"} inside it will be removed from the project.`;
}

type KeyStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;
const createdDigestKey = (projectId: string) => `created-digest:${projectId}`;

function rememberCreatedDigest(storage: KeyStorage, project: MiniAppProject) {
  try { storage.setItem(createdDigestKey(project.id), "pending"); } catch { return; }
  void cachedSourceDigest(project).then((digest) => {
    try { if (storage.getItem(createdDigestKey(project.id)) === "pending") storage.setItem(createdDigestKey(project.id), digest); } catch { /* the button just stays as it is */ }
  }, () => forgetCreatedDigest(storage, project.id));
}

function readCreatedDigest(storage: KeyStorage, projectId: string): string | null {
  try { return storage.getItem(createdDigestKey(projectId)); } catch { return null; }
}

function forgetCreatedDigest(storage: KeyStorage, projectId: string) {
  try { storage.removeItem(createdDigestKey(projectId)); } catch { /* nothing to forget */ }
}

function AccountChip({ session, href }: { session: OwnerSession; href: string }) {
  const initials = ownerInitials(session);
  const label = session.state === "signed-in"
    ? `Signed in${session.email ? ` as ${session.email}` : ""}. Open owner account`
    : "Owner sign-in";
  return (
    <a className={`avatar account-chip is-${session.state}`} href={href} aria-label={label} title={label}>
      {initials ?? <UserRound size={14} />}
      {session.state === "signed-out" && <span className="account-chip-label">Owner sign-in</span>}
    </a>
  );
}

function downloadBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

function fileMetaFor(file: string): { label: string; icon: typeof FileText; language: string } {
  const known = knownFileMeta[file];
  if (known) return known;
  const extension = file.split(".").at(-1)?.toLowerCase();
  const language = extension === "ts" || extension === "tsx" ? "TYPESCRIPT"
    : extension === "css" ? "CSS"
      : extension === "json" ? "JSON"
        : extension === "html" ? "HTML"
          : extension === "js" || extension === "jsx" ? "JAVASCRIPT" : "TEXT";
  return { label: file, icon: extension === "json" ? FileJson2 : extension === "html" ? FileCode2 : FileText, language };
}

function isEditorFile(file: string): file is EditorFile {
  return Boolean(file);
}

function documentFingerprint(text: string) {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index++) hash = Math.imul(hash ^ text.charCodeAt(index), 0x01000193);
  return `${text.length.toString(36)}-${(hash >>> 0).toString(36)}`;
}
