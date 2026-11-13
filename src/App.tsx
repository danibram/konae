import {
  Activity,
  Box,
  ChevronDown,
  CircleAlert,
  Code2,
  Database,
  Download,
  FileCode2,
  FilePlus2,
  FileJson2,
  FileText,
  FolderPlus,
  FolderOpen,
  FolderTree,
  Laptop,
  MoreHorizontal,
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
import { mergeProposal } from "./agent/apply";
import type { AgentProposal, AgentSessionConfig } from "./agent/contracts";
import type { KonaeAgentHandle, PlatformConfirmation, PublishOutcome } from "./agent/harness";
import { createAgentKeyStore } from "./agent/key-store";
import { Button, Dialog, LoadingBlock } from "./components/ui";
import { AgentPanel } from "./components/AgentPanel";
import { AgentSettingsModal } from "./components/AgentSettingsModal";
import { useAgentThread } from "./components/useAgentThread";
import { focusPanelTab, WorkbenchLayout, type PanelRender } from "./components/WorkbenchLayout";
import { closePanel, isOpen, isVisible, loadLayout, movePanel, openPanel, panelIds, resetLayout, saveLayout, stack, togglePanel, type LayoutState, type PanelId } from "./ui/layout";
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
import { filesUnder, renamePath } from "./components/project-tree";
import { usePreviewProblems } from "./components/usePreviewProblems";
import { ProjectDashboard } from "./components/ProjectDashboard";
import { NotFound } from "./components/NotFound";
import { deployProject, HostedDatabaseClient, type DeploymentReceipt, type SignupMode } from "./deployments/client";
import { McpError, platformMcp } from "./mcp/client";
import { readReleaseFacts } from "./deployments/release-facts";
import type { DeploymentVisibility, RuntimeActionAccess } from "./deployments/manifest";
import { parseServerActionSource } from "./kernel/actions";
import { applyLocalAgentEdit } from "./lib/agent";
import { parseRoute, pushRoute } from "./lib/routing";
import { PublishedAppView } from "./components/PublishedAppView";
import { applyTemplate, type TemplateId } from "./lib/templates";
import { cloneStarter } from "./lib/starter";
import type { MiniAppProject, ProjectFile } from "./lib/types";
import { createDirectory, createFile, directoriesForFiles, normalizeProjectPath, protectedProjectPaths, removePath } from "./lib/project-files";
import { actionReadout } from "./components/action-readiness";
import { createWorkspaceProject, dismissDrafts, loadWorkspaceState, persistWorkspace, readDismissedDrafts, recoveryDrafts, resetWorkspaceProject, storedLocalProjects } from "./lib/workspace";
import { ProjectSources, type ProjectMetadata } from "./lib/project-sources";
import { createPreviewDocument } from "./runtime/preview";
import { buildProjectInBrowser, compileProjectInBrowser } from "./runtime/browser-build";
import { useRuntimeBridge } from "./runtime/useRuntimeBridge";

const WhatHappenedView = lazy(() =>
  import("./components/WhatHappenedView").then((module) => ({ default: module.WhatHappenedView })),
);
const CapabilityWorkspace = lazy(() =>
  import("./components/CapabilityWorkspace").then((module) => ({ default: module.CapabilityWorkspace })),
);
const AppSettingsDialog = lazy(() => import("./components/AppSettingsDialog").then((module) => ({ default: module.AppSettingsDialog })));

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

/** Shortcut hints follow the keyboard in front of the owner. */
const modKey = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? "⌘" : "Ctrl+";

export function App() {
  if (window.location.pathname === "/platform-access") return <PlatformAccess />;
  // Public: outside PlatformGate, so a signed-out visitor is not sent to sign-in.
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
  // A boot that took long enough to show its skeleton keeps it for a moment, so it never flashes.
  const booting = useLoadingHold(!loaded, { sinceNavigation: true });
  if (error) return <main className="platform-page"><div className="platform-main"><CardForPlatformError message={error} onRetry={() => window.location.reload()} /></div></main>;
  if (!loaded || booting) return <BootShell ownerSession={ownerSession} />;
  return <WorkspaceApp ownerSession={ownerSession} refreshOwnerSession={refreshOwnerSession} sources={sources} loaded={loaded} />;
}

function CardForPlatformError({ message, onRetry }: { message: string; onRetry: () => void }) {
  return <div className="platform-card"><h1>Platform unavailable</h1><p role="alert">Could not check your session. {message}</p><Button onClick={onRetry}>Try again</Button></div>;
}

function WorkspaceApp({ ownerSession, refreshOwnerSession, sources, loaded }: { ownerSession: Extract<OwnerSession, { state: "signed-in" }>; refreshOwnerSession: () => void; sources: ProjectSources; loaded: { projects: MiniAppProject[]; missing: ProjectMetadata[]; deployments: Record<string, DeploymentReceipt> } }) {
  const workspaceStorage = useMemo(() => ({
    getItem: (key: string) => localStorage.getItem(`konae:owner:${encodeURIComponent(ownerSession.ownerId)}:${key}`),
    setItem: (key: string, value: string) => localStorage.setItem(`konae:owner:${encodeURIComponent(ownerSession.ownerId)}:${key}`, value),
  }), [ownerSession.ownerId]);
  const [initialWorkspace] = useState(() => {
    const cached = loadWorkspaceState(workspaceStorage);
    return { projects: loaded.projects, deployments: loaded.deployments, activeProjectId: loaded.projects.some((item) => item.id === cached.activeProjectId) ? cached.activeProjectId : loaded.projects[0]?.id ?? "" };
  });
  const [drafts, setDrafts] = useState(() => {
    const candidates: MiniAppProject[] = [];
    try {
      const recovery = JSON.parse(workspaceStorage.getItem("recovery") ?? "[]") as MiniAppProject[];
      if (Array.isArray(recovery)) candidates.push(...recovery);
      candidates.push(...storedLocalProjects(workspaceStorage), ...storedLocalProjects());
    } catch { /* Invalid local drafts remain untouched. */ }
    return recoveryDrafts(candidates, loaded.projects, readDismissedDrafts(workspaceStorage));
  });
  const [missing, setMissing] = useState(loaded.missing);
  const [projects, setProjects] = useState(initialWorkspace.projects);
  const [deployments, setDeployments] = useState(initialWorkspace.deployments);
  const [project, setProject] = useState(() => initialWorkspace.projects.find((item) => item.id === initialWorkspace.activeProjectId) ?? initialWorkspace.projects[0] ?? cloneStarter());
  const [previewProject, setPreviewProject] = useState(project);
  const [previewEpoch, setPreviewEpoch] = useState(0);
  const [route, setRoute] = useState(() => {
    const requested = parseRoute(window.location.pathname);
    if (requested.screen !== "editor" || loaded.projects.some((item) => item.id === requested.projectId)) return requested;
    // A published app with no editor files opens on its data. An unpublished one is on the list below; any other id has no page.
    if (loaded.missing.some((item) => item.id === requested.projectId)) return loaded.deployments[requested.projectId] ? requested : { screen: "projects" as const };
    return { screen: "not-found" as const, what: "project" as const, path: window.location.pathname };
  });
  const screen = route.screen;
  // A link to an unpublished app this account holds without editor files says so, and the address bar stops naming it.
  const [routeNotice, setRouteNotice] = useState("");
  const explainMissingRoute = () => {
    window.history.replaceState({}, "", "/");
    setRouteNotice("That app has no editor files in this account and is not published. See Apps without editor files below.");
  };
  useEffect(() => {
    const requested = parseRoute(window.location.pathname);
    if (requested.screen === "editor" && loaded.missing.some((item) => item.id === requested.projectId) && !loaded.deployments[requested.projectId]) explainMissingRoute();
  }, []);
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
  const [databaseTarget, setDatabaseTarget] = useState<"local" | "deployed">("local");
  const [authView, setAuthView] = useState<"rules" | "users" | "integrations">("rules");
  const [exporting, setExporting] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [deployError, setDeployError] = useState("");
  const [deployErrorCode, setDeployErrorCode] = useState<string | undefined>();
  const [schemaChange, setSchemaChange] = useState<{ change: SchemaChange; resolve: (confirmed: boolean) => void } | null>(null);
  const [saved, setSaved] = useState(true);
  const [saveError, setSaveError] = useState("");
  const [operationError, setOperationError] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [appSettingsOpen, setAppSettingsOpen] = useState(false);
  const [domainVersion, setDomainVersion] = useState(0);
  // A platform tool the agent wants to run that waits for the owner (ADR-0007).
  const [platformConfirmation, setPlatformConfirmation] = useState<PlatformConfirmation | null>(null);
  const resolvePlatformConfirmation = useRef<((approved: boolean) => void) | null>(null);
  // The agent's publish tool waits on the Deploy dialog the owner drives.
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
  const demoProject = useRef<MiniAppProject | null>(null);
  const projectMenu = useRef<HTMLDetailsElement>(null);
  const switcherRef = useRef<HTMLDetailsElement>(null);
  const workspaceRef = useRef({ projects, project, deployments });
  const synchronized = useRef(new Map(loaded.projects.map((item) => [item.id, JSON.stringify(item)])));
  const pendingSaves = useRef(0);
  const failedSave = useRef(false);
  const failedProjects = useRef(new Set<string>());
  const [saveAttempt, setSaveAttempt] = useState(0);
  workspaceRef.current = { projects, project, deployments };

  useEffect(() => {
    const flush = () => {
      const current = workspaceRef.current;
      const items = current.projects.map((item) => item.id === current.project.id ? current.project : item);
      try { persistWorkspace(items, current.project.id, current.deployments, workspaceStorage); workspaceStorage.setItem("recovery", JSON.stringify([...drafts, ...items])); }
      catch { /* The regular save reports storage failures in the editor. */ }
    };
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", flush);
    };
  }, [workspaceStorage, drafts]);

  const gutterRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const keyStore = useMemo(() => createAgentKeyStore(localStorage, ownerSession.ownerId), [ownerSession.ownerId]);
  // The file list lives inside Files now. A layout saved while it was a panel of its own drops it.
  const [layout, setLayout] = useState<LayoutState>(() => {
    const saved = loadLayout();
    return isOpen(saved, "explorer") ? closePanel(saved, "explorer") : saved;
  });
  const [explorerOpen, setExplorerOpen] = useState(false);
  const [compact, setCompact] = useState(() => window.matchMedia("(max-width: 900px)").matches);
  const [compactPanel, setCompactPanel] = useState<PanelId>("preview");
  // A window that crosses the breakpoint, such as a rotated tablet, keeps the section it showed.
  const shownPanel = useRef({ compactPanel, focused: layout.focused });
  shownPanel.current = { compactPanel, focused: layout.focused };

  useEffect(() => {
    const media = window.matchMedia("(max-width: 900px)");
    const follow = () => {
      const next = media.matches;
      setCompact(next);
      if (next) {
        setCompactPanel(shownPanel.current.focused ?? "preview");
        return;
      }
      setLayout((current) => {
        const shown = openPanel(current, shownPanel.current.compactPanel);
        saveLayout(shown);
        return shown;
      });
    };
    media.addEventListener("change", follow);
    return () => media.removeEventListener("change", follow);
  }, []);

  useEffect(() => {
    const closeMenus = () => {
      projectMenu.current?.removeAttribute("open");
      switcherRef.current?.removeAttribute("open");
    };
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") closeMenus(); };
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (projectMenu.current?.open && !projectMenu.current.contains(target)) projectMenu.current.removeAttribute("open");
      if (switcherRef.current?.open && !switcherRef.current.contains(target)) switcherRef.current.removeAttribute("open");
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onPointer);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onPointer);
    };
  }, []);

  useEffect(() => {
    if (deployOpen || resetOpen || settingsOpen || appSettingsOpen || platformConfirmation) {
      projectMenu.current?.removeAttribute("open");
      switcherRef.current?.removeAttribute("open");
    }
  }, [deployOpen, resetOpen, settingsOpen, appSettingsOpen, platformConfirmation]);

  useEffect(() => {
    setAppSettingsOpen(false);
    setPlatformConfirmation(null);
    return () => {
      resolvePlatformConfirmation.current?.(false);
      resolvePlatformConfirmation.current = null;
      resolveAgentPublish.current?.({ status: "declined" });
      resolveAgentPublish.current = null;
    };
  }, [project.id]);

  const updateLayout = (next: LayoutState) => {
    if (compact) {
      if (next.focused) setCompactPanel(next.focused);
      return;
    }
    setLayout(next);
    saveLayout(next);
  };
  const displayedLayout: LayoutState = compact
    ? { root: stack(workbenchPanels, workbenchPanels.indexOf(compactPanel)), closed: [], focused: compactPanel }
    : layout;
  const [previewFrame, setPreviewFrame] = useState<HTMLIFrameElement | null>(null);
  const hostRef = useRuntimeBridge(previewFrame, project.id, previewProject.files["schema.sql"], previewProject.files["actions.json"], previewProject.manifest.fileAccess);
  const previewProblems = usePreviewProblems(hostRef, project.id);

  const previewDocument = useMemo(() => createPreviewDocument(previewProject), [previewProject]);
  /**
   * Assigning a new srcDoc to a live iframe is a navigation that adds a browser
   * history entry, so the editor's Back button stepped through old previews.
   * A new document gets a new iframe instead.
   */
  const previewDocumentKey = useMemo(() => documentFingerprint(previewDocument), [previewDocument]);
  const activeDeployment = deployments[project.id] ?? null;
  const deployedDatabase = useMemo(() => activeDeployment ? new HostedDatabaseClient(activeDeployment.id) : undefined, [activeDeployment]);
  const actionContract = useActionContract(project.files["actions.json"], project.files["schema.sql"], project);
  const contractOpen = activeFile === ACTION_CONTRACT_MARKDOWN;
  const activeSource = contractOpen ? (actionContract.ok ? actionContract.markdown : actionContract.reason) : project.files[activeFile] ?? "";
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
        workspaceStorage.setItem("recovery", JSON.stringify([...drafts, ...projects]));
      } catch (error) {
        // A full or unavailable localStorage must say so. Reporting "Saving"
        // forever, or worse "Saved locally", would hide that the work of the
        // whole session is only in memory.
        setSaveError(error instanceof Error ? error.message : String(error));
      }
    }, 350);
    return () => window.clearTimeout(timeout);
  }, [deployments, project, projects, workspaceStorage, drafts]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      for (const item of projects) {
        const snapshot = JSON.stringify(item);
        if (synchronized.current.get(item.id) === snapshot) continue;
        synchronized.current.set(item.id, snapshot);
        pendingSaves.current++;
        setSaved(false);
        void sources.save(item).catch((error) => {
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
  }, [projects, sources, saveAttempt]);

  const retrySave = () => {
    if (pendingSaves.current) return;
    for (const id of failedProjects.current) { sources.retry(id); synchronized.current.delete(id); }
    failedProjects.current.clear();
    failedSave.current = false;
    setSaveError("");
    setSaveAttempt((value) => value + 1);
  };

  // Opening another file leaves the textarea wherever it was; the gutter follows it.
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
      if (compact) {
        setCompactPanel(action.panel);
        requestAnimationFrame(() => focusPanelTab(action.panel));
        return;
      }
      const next = action.kind === "reveal" ? openPanel(layout, action.panel) : togglePanel(layout, action.panel);
      updateLayout(next);
      // Focus follows the keyboard: revealing a panel hides whatever was in front
      // of it, and leaving focus on that hidden body drops the next keystrokes.
      if (isOpen(next, action.panel)) requestAnimationFrame(() => focusPanelTab(action.panel));
    };
    window.addEventListener("keydown", shortcut);
    return () => window.removeEventListener("keydown", shortcut);
  });

  const updateFile = (value: string) => {
    if (contractOpen) return;
    setProject((current) => ({
      ...current,
      updatedAt: Date.now(),
      files: { ...current.files, [activeFile]: value },
    }));
  };

  // Tree edits are computed before React sees them, because a throw inside a
  // state updater escapes this try/catch and takes the whole editor down.
  const addTreeEntry = () => {
    if (!newEntry) return;
    if (!newEntryPath.trim()) {
      setTreeError(newEntry === "file" ? "Enter a file path, for example src/tasks.js." : "Enter a folder path, for example src/features.");
      return;
    }
    try {
      const current = projectRef.current;
      const tree = { files: current.files, directories: current.directories };
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
      const next = renamePath({ files: current.files, directories: current.directories }, from, renameValue);
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
      ...removePath({ files: current.files, directories: current.directories }, path),
      updatedAt: Date.now(),
    });
    if (activeFile === path || activeFile.startsWith(`${path}/`)) setActiveFile("app.js");
    setSelectedPath("app.js");
    setPendingDelete(null);
  };

  /**
   * Rebuilds the preview. Run and its shortcut also bring the Preview forward,
   * beside Files when Files is showing, so the person sees what the edit did.
   * A failed build shows nothing new; the "Build failed" alert says why.
   */
  const runPreview = ({ show = false }: { show?: boolean } = {}) => {
    setOperationError("");
    void compileProjectInBrowser(project)
      .then((compiled) => {
        setPreviewProject({
          ...project,
          files: { ...project.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script },
        });
        setPreviewEpoch((value) => value + 1);
        if (!show) return;
        if (compact) {
          setCompactPanel("preview");
          return;
        }
        setLayout((current) => {
          const next = withPreviewShowing(current);
          saveLayout(next);
          return next;
        });
      })
      .catch((error) => setOperationError(`Build failed: ${error instanceof Error ? error.message : String(error)}`));
  };

  useEffect(() => {
    let cancelled = false;
    // A build failure belongs to the project that failed. Opening another one starts clean.
    setOperationError("");
    void compileProjectInBrowser(project)
      .then((compiled) => {
        if (!cancelled) setPreviewProject({
          ...project,
          files: { ...project.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script },
        });
      })
      .catch((error) => {
        if (!cancelled) setOperationError(`Build failed: ${error instanceof Error ? error.message : String(error)}`);
      });
    return () => { cancelled = true; };
  }, [project.id]);

  projectRef.current = project;

  /**
   * ADR-0007 lever 7: pi-agent-core, the provider catalogs and sql.js are imported
   * on the first prompt, never at editor start.
   */
  const ensureAgent = () => {
    if (agentRef.current) return Promise.resolve<KonaeAgentHandle | null>(agentRef.current);
    if (!agentLoad.current) {
      agentLoad.current = import("./agent/harness")
        .then(({ createKonaeAgent }) => {
          const handle = createKonaeAgent(projectRef.current, agentSession, keyStore, {
            // The Validation Gate's preview-rows stage needs the live database, not just a fresh one.
            liveDatabase: () => hostRef.current.exportDatabase(),
            // The other half of the gate: what the running preview actually threw.
            previewErrors: () => hostRef.current.previewErrors(),
            // What release_plan reads, as the Deploy dialog reads it: secret names and the installation's hosts.
            releaseFacts: async () => {
              const current = projectRef.current;
              const deploymentId = deploymentsRef.current[current.id]?.id;
              const facts = await readReleaseFacts(current.id, deploymentId);
              return { ...facts, ...(Object.hasOwn(current.manifest, "fileAccess") ? { fileAccess: current.manifest.fileAccess } : {}) };
            },
            // ADR-0007: the owner's own MCP session, pinned to the open project.
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

  // The session is never allowed to keep reasoning about a Project it no longer
  // edits — including a Capsule file the owner changed by hand. `retarget` knows
  // the difference: it re-seeds the workspace and drops a pending proposal that
  // was validated against files that have since moved, rather than letting Apply
  // write it onto a Capsule the gate never saw. An Apply of its own is not a
  // change from outside: `reseed` has already told the session about it.
  useEffect(() => {
    agentRef.current?.retarget(project);
  }, [project.id, project.manifest, project.files]);

  useEffect(() => {
    agentRef.current?.configure(agentSession);
  }, [agent, agentSession]);

  useEffect(() => () => agentRef.current?.dispose(), []);

  // The address bar leads: a reload or a back button lands on the same project.
  useEffect(() => {
    const follow = () => setRoute(parseRoute(window.location.pathname));
    window.addEventListener("popstate", follow);
    return () => window.removeEventListener("popstate", follow);
  }, []);

  useEffect(() => {
    if (route.screen !== "editor" || route.projectId === project.id) return;
    const target = projects.find((item) => item.id === route.projectId);
    // A link to a project this workspace does not have says so; the list would hide the wrong address.
    if (!target) {
      if (missing.some((item) => item.id === route.projectId)) {
        // A published app with no editor files has its own page, which this route already names.
        if (deployments[route.projectId]) return;
        setRoute({ screen: "projects" });
        explainMissingRoute();
      } else {
        setRoute({ screen: "not-found", what: "project", path: window.location.pathname });
      }
      return;
    }
    setProject(target);
    setPreviewProject({ ...target, files: { ...target.files } });
    setThreadResetToken((token) => token + 1);
    // The open file belongs to the project being left; typing would create it in this one.
    setActiveFile("app.js");
    setSelectedPath("app.js");
    setDatabaseTarget("local");
  }, [route, projects, project.id]);

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
    setCompactPanel(target === "preview" ? "preview" : target);
    setLayout((current) => {
      const next = focusRunningApp(current, target);
      saveLayout(next);
      return next;
    });
  }, [route]);

  const applyAgentProposal = (proposal: AgentProposal): boolean => {
    // The Validation Gate is structural here, not just a disabled button in the
    // panel: the merge refuses a proposal whose untouched files have moved since
    // the gate saw them, because that combination was never validated.
    const merge = mergeProposal(project.files, proposal);
    if (!merge.ok) return false;
    const next: MiniAppProject = {
      ...project,
      files: merge.files,
      directories: [...new Set([...project.directories, ...directoriesForFiles(merge.files)])].sort(),
      updatedAt: Date.now(),
    };
    setProject(next);
    setPreviewProject(next);
    agentRef.current?.reseed(next.files);
    if (merge.changedFiles[0] && isEditorFile(merge.changedFiles[0])) {
      setActiveFile(merge.changedFiles[0]);
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
    onDiscard: () => agentRef.current?.discard(),
    onDemoSend: (text) => sendDemoPrompt(text),
    onDemoApply: () => {
      const next = demoProject.current;
      if (!next) return;
      demoProject.current = null;
      setProject(next);
      setPreviewProject(next);
    },
    providers: PROVIDERS,
  });

  /** Offline fallback: the scripted demo proposes a change and waits to apply it. */
  const sendDemoPrompt = async (value: string) => {
    await new Promise((resolve) => window.setTimeout(resolve, 450));
    const edit = applyLocalAgentEdit(projectRef.current, value);
    demoProject.current = edit.changedFiles.length > 0 ? edit.project : null;
    return { message: edit.message, changedFiles: edit.changedFiles };
  };

  const resetProject = () => {
    const fresh = resetWorkspaceProject(project);
    hostRef.current.reset();
    setProject(fresh);
    setPreviewProject(fresh);
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setSelectedPath("app.js");
  };

  const exportProject = async () => {
    setExporting(true);
    setOperationError("");
    try {
      const { createProjectExport } = await import("./lib/export-project");
      const artifact = await buildProjectInBrowser(project);
      const result = await createProjectExport(project, artifact, activeDeployment?.id);
      downloadBlob(result.blob, result.filename);
      // The editor saves between releases, so the published app may run older sources than app/ holds.
      if (result.sourcesAhead) setOperationError("Exported. The folder's app/actions.json differs from the actions the published app runs, so app/ is ahead of dist/ and data/. Publish, then export again, to make them match.");
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
      const artifact = await buildProjectInBrowser(project);
      // The signup policy goes only with a first release; the Kernel keeps it on a republish.
      const initialSignupPolicy = !activeDeployment && signupMode ? { mode: signupMode, allowlist: [] } : undefined;
      const confirmSchema = (change: SchemaChange) => new Promise<boolean>((resolve) => setSchemaChange({ change, resolve }));
      const record = await deployProject(project, visibility, artifact, confirmSchema, initialSignupPolicy);
      setProject((current) => ({ ...current, manifest: record.manifest, updatedAt: Date.now() }));
      setDeployment(record);
      setDeployments((current) => ({ ...current, [project.id]: record }));
      resolveAgentPublish.current?.({ status: "published", url: record.url, visibility: record.manifest.visibility });
      resolveAgentPublish.current = null;
    } catch (error) {
      // The owner kept the data: nothing was published and nothing failed.
      if (error instanceof SchemaChangeDeclined) return;
      if (isSignedOutError(error)) refreshOwnerSession();
      setDeployError(error instanceof Error ? error.message : String(error));
      setDeployErrorCode(error instanceof McpError ? error.refusal?.code : undefined);
    } finally {
      setPublishing(false);
    }
  };

  const openEditorRoute = (projectId: string) => {
    setRouteNotice("");
    const next = { screen: "editor" as const, projectId };
    setRoute(next);
    pushRoute(next);
  };

  const goToProjects = () => {
    const next = { screen: "projects" as const };
    setRoute(next);
    pushRoute(next);
  };

  const openProject = (projectId: string, entry: "preview" | "agent" | "code" = "preview") => {
    const next = projects.find((item) => item.id === projectId);
    if (!next) return;
    arrival.current = entry === "code" ? "code" : entry === "agent" ? "agent" : "preview";
    setProject(next);
    setPreviewProject({ ...next, files: { ...next.files } });
    setThreadResetToken((token) => token + 1);
    setActiveFile(entry === "code" ? "app.js" : "index.html");
    setSelectedPath(entry === "code" ? "app.js" : "index.html");
    setDatabaseTarget("local");
    const target = entry === "code" ? "code" : entry;
    setCompactPanel(target);
    if (!compact) updateLayout(focusRunningApp(layout, target));
    openEditorRoute(next.id);
  };

  const createProject = (name: string, template: TemplateId | "scratch" = "tasks") => {
    arrival.current = template === "scratch" ? "agent" : "preview";
    const base = createWorkspaceProject(name, projects);
    // A project described from scratch keeps the private default; only a template made for visitors starts public.
    const next = template === "scratch" ? base : applyTemplate(base, template);
    setProjects((current) => [...current, next]);
    setProject(next);
    setPreviewProject(next);
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setSelectedPath("app.js");
    setDatabaseTarget("local");
    const target = template === "scratch" ? "agent" : "preview";
    setCompactPanel(target);
    if (!compact) updateLayout(focusRunningApp(layout, target));
    openEditorRoute(next.id);
  };

  const importDraft = (index: number) => {
    const draft = drafts[index];
    if (!draft) return;
    const restore = missing.some((item) => item.id === draft.id);
    const copy = { ...draft, id: restore ? draft.id : `app_${crypto.randomUUID()}`, updatedAt: Date.now() };
    setProjects((current) => [...current, copy]);
    setDrafts((current) => current.filter((_, itemIndex) => itemIndex !== index));
  };

  const dismissDraft = (index: number) => {
    const draft = drafts[index];
    if (!draft) return;
    dismissDrafts(workspaceStorage, [draft]);
    setDrafts((current) => current.filter((_, itemIndex) => itemIndex !== index));
  };

  const deleteMissingProject = async (item: ProjectMetadata, name: string) => {
    await sources.deleteProject(item.id, name);
    setMissing((current) => current.filter((entry) => entry.id !== item.id));
    setDeployments((current) => {
      const { [item.id]: _removed, ...rest } = current;
      return rest;
    });
    // A local copy of a deleted app would otherwise come back as an import.
    const copies = drafts.filter((draft) => draft.id === item.id);
    if (copies.length) {
      dismissDrafts(workspaceStorage, copies);
      setDrafts((current) => current.filter((draft) => draft.id !== item.id));
    }
  };

  const setActionAccess = (name: string, access: RuntimeActionAccess) => {
    let parsed: { actions?: Record<string, { access?: string }> };
    try {
      parsed = JSON.parse(project.files["actions.json"] ?? "");
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
    const declarations = Object.values(program.actions).map((action) => ({ name: action.name, kind: action.kind, access: action.access }));
    const next = {
      ...project,
      files: { ...project.files, "actions.json": source },
      manifest: {
        ...project.manifest,
        runtime: {
          ...project.manifest.runtime,
          database: { ...project.manifest.runtime.database, actions: declarations },
        },
      },
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

  const setFileAccess = (shared: boolean) => {
    const { fileAccess: _fileAccess, ...manifest } = project.manifest;
    const next = { ...project, manifest: { ...manifest, ...(shared ? { fileAccess: "app-users" as const } : {}) }, updatedAt: Date.now() };
    setProject(next);
    setPreviewProject(next);
  };

  if (route.screen === "not-found") return <NotFound what={route.what} path={route.path} />;

  if (screen === "projects") {
    return <ProjectDashboard projects={projects} deployments={deployments} ownerSession={ownerSession} missing={missing} notice={routeNotice} drafts={drafts} onImportDraft={importDraft} onDismissDraft={dismissDraft} onDeleteMissing={deleteMissingProject} onOpenProject={openProject} onOpenPublished={openEditorRoute} onCreateProject={createProject} />;
  }

  // A published app with no editor files opens on its data, accounts and activity.
  const published = missing.find((item) => item.id === route.projectId);
  if (published && deployments[published.id]) {
    return <PublishedAppView app={published} deployment={deployments[published.id]} ownerSession={ownerSession} projectCount={projects.length + missing.length} onBack={goToProjects} />;
  }

  const signInHref = platformAccessHref(window.location.pathname);

  const sections: { id: PanelId; label: string; icon: typeof Code2 }[] = [
    { id: "preview", label: "Preview", icon: Play },
    { id: "agent", label: "Describe", icon: Sparkles },
    { id: "auth", label: "Auth", icon: UserRound },
    { id: "code", label: "Files", icon: Code2 },
    { id: "data", label: "Data", icon: Database },
    { id: "files", label: "Uploads", icon: FolderOpen },
    { id: "activity", label: "What happened", icon: Activity },
  ];

  const reveal = (panel: PanelId) => {
    if (compact) {
      setCompactPanel(panel);
      return;
    }
    updateLayout(focusRunningApp(layout, panel));
  };

  const filesShowing = compact ? compactPanel === "code" : isVisible(layout, "code");

  /** The File list is part of Files: opening it shows Files, and it stays until closed. */
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

  // Tab indents. Escape first, then Tab, leaves the editor, so the keyboard is never trapped.
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
        noteFor={(path) => path === "actions.json" ? "returns at most 100" : path === "schema.sql" ? "kernel runs this" : path === ACTION_CONTRACT_MARKDOWN ? "generated" : ""}
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
        <span className="panel-meta">{activeFile === "actions.json" ? `${listedActions(activeSource)} actions.` : `${activeMeta.language} · ${lines} lines`}</span>
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
          {activeFile === "actions.json" && <ActionReadout source={activeSource} visibility={project.manifest.visibility} />}
          {contractOpen ? <ActionContractView state={actionContract} /> : <div className="code-area">
            <div className="line-numbers" aria-hidden="true" ref={gutterRef}>
              {Array.from({ length: lines }, (_, index) => <span key={index}>{index + 1}</span>)}
            </div>
            <textarea
              ref={editorRef}
              value={activeSource}
              onChange={(event) => updateFile(event.target.value)}
              onKeyDown={indentWithTab}
              aria-describedby="code-editor-keys"
              // The textarea is the only scroller; the gutter is dragged along by
              // hand, or every number is wrong from the first scroll onwards.
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
    body: (
      <Suspense fallback={<LoadingBlock label="Loading this panel" rows={4} className="panel-loading" />}>
        <CapabilityWorkspace
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
          fileAccess={project.manifest.fileAccess}
          onSetFileAccess={setFileAccess}
          projectId={project.id}
          deployment={activeDeployment ? { id: activeDeployment.id, url: activeDeployment.url } : undefined}
          authView={authView}
          onAuthViewChange={setAuthView}
        />
      </Suspense>
    ),
  });

  const activityPanel: PanelRender = {
    id: "activity",
    title: "What happened",
    icon: <Activity size={11} />,
    body: (
      <Suspense fallback={<LoadingBlock label="Loading this panel" rows={4} className="panel-loading" />}>
        <WhatHappenedView
          deployment={activeDeployment ? { id: activeDeployment.id, url: activeDeployment.url } : undefined}
          onDeploy={() => { setDeployment(null); setDeployError(""); setDeployOpen(true); refreshOwnerSession(); }}
          onOpen={(place) => {
            if (place === "capsule") { setActiveFile("actions.json"); reveal("code"); return; }
            setAuthView(place);
            reveal("auth");
          }}
        />
      </Suspense>
    ),
  };

  const previewPanel: PanelRender = {
    id: "preview",
    title: project.name,
    icon: <Play size={12} />,
    actions: (
      <>
        <button className={viewport === "desktop" ? "active" : ""} title="Desktop" aria-label="Desktop preview" aria-pressed={viewport === "desktop"} onClick={() => setViewport("desktop")}><Laptop size={12} /></button>
        <button className={viewport === "mobile" ? "active" : ""} title="Mobile" aria-label="Mobile preview" aria-pressed={viewport === "mobile"} onClick={() => setViewport("mobile")}><Smartphone size={12} /></button>
        <button title="Reload preview" aria-label="Reload preview" onClick={() => runPreview()}><RefreshCw size={11} /></button>
      </>
    ),
    body: (
      <div className="preview-panel-body">
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
          <iframe key={`${project.id}:${previewEpoch}:${previewDocumentKey}`} ref={setPreviewFrame} title="Mini-app preview" sandbox="allow-scripts allow-forms" srcDoc={previewDocument} />
        </div>
      </div>
    ),
  };

  const agentPanel: PanelRender = {
    id: "agent",
    title: "Describe",
    icon: <Sparkles size={12} />,
    actions: (
      <button title="Agent settings" onClick={() => setSettingsOpen(true)}><Settings2 size={11} /></button>
    ),
    body: <AgentPanel thread={agentThread} sessionConfig={agentSession} onOpenSettings={() => setSettingsOpen(true)} />,
  };

  return (
    <div className={`workshop ${compact ? "is-compact" : ""} ${isVisible(displayedLayout, "code") ? "is-code-open" : ""} ${operationError || agentThread.footer.kind === "blocked" || (agentThread.footer.kind === "validated" && agentThread.footer.owner) ? "has-error" : ""}`}>
      <header className="topbar">
        <button className="brand" aria-label="Konae home" onClick={() => goToProjects()}>
          <span className="brand-mark"><Box size={17} strokeWidth={2.4} /></span>
          <span>konae</span>
        </button>

        <details className="project-switcher" ref={switcherRef} onToggle={(event) => { if (event.currentTarget.open) projectMenu.current?.removeAttribute("open"); }}>
          <summary>
            <strong title={project.name}>{project.name}</strong>
            <ChevronDown size={14} />
          </summary>
          <div>
            {projects.map((item) => (
              <button key={item.id} aria-current={item.id === project.id ? "true" : undefined} onClick={(event) => { event.currentTarget.closest("details")?.removeAttribute("open"); openProject(item.id); }}>{item.name}</button>
            ))}
            <button onClick={(event) => { event.currentTarget.closest("details")?.removeAttribute("open"); goToProjects(); }}>All projects</button>
          </div>
        </details>

        <div className="top-actions">
          {!saved && <span
            className={`save-state ${saveError ? "is-failed" : ""}`}
            title={saveError || undefined}
            role={saveError ? "alert" : undefined}
          >
            {saveError ? <CircleAlert size={12} /> : <RefreshCw size={12} className="spin" />}
            {saveError ? "Not saved" : "Saving"}
          </span>}
          {saveError && <Button onClick={retrySave} disabled={pendingSaves.current > 0}>Retry save</Button>}
          <div className="take-actions" role="group" aria-label="Take it with you">
            <Button className="export-button" aria-label={exporting ? "Exporting" : "Export"} disabled={exporting} onClick={() => void exportProject()}><Download size={14} /> <span>{exporting ? "Exporting…" : "Export"}</span></Button>
            <Button variant="primary" className="deploy-button" aria-label="Deploy" onClick={() => { setDeployment(null); setDeployError(""); setDeployOpen(true); refreshOwnerSession(); }}><Rocket size={15} /> <span>Deploy</span></Button>
          </div>
          <details className="mobile-project-actions" ref={projectMenu} onToggle={(event) => { if (event.currentTarget.open) switcherRef.current?.removeAttribute("open"); }}>
            <summary>More</summary>
            <div>
              <button onClick={(event) => { event.currentTarget.closest("details")?.removeAttribute("open"); setAppSettingsOpen(true); }}>App settings</button>
              <button onClick={(event) => { event.currentTarget.closest("details")?.removeAttribute("open"); updateLayout(resetLayout()); }}>Reset layout</button>
              <button className="is-danger" onClick={(event) => { event.currentTarget.closest("details")?.removeAttribute("open"); setResetOpen(true); }}><RotateCcw size={15} /> Reset project data…</button>
            </div>
          </details>
          <AccountChip session={ownerSession} href={signInHref} />
        </div>
      </header>

      {operationError && <div className="operation-error" role="alert">{operationError}<button onClick={() => setOperationError("")}>Dismiss</button></div>}
      {agentThread.footer.kind === "blocked" && (
        <div className="gate-refusal" role="alert">
          <div>
            <strong>A check refused this change</strong>
            <span>{agentThread.footer.message}</span>
            {agentThread.footer.detail && <span className="gate-refusal-detail">{agentThread.footer.detail}</span>}
          </div>
          <button type="button" onClick={() => reveal("agent")}>Open Describe</button>
        </div>
      )}
      {agentThread.footer.kind === "validated" && agentThread.footer.owner && (
        <div className="gate-refusal is-owner" role="status">
          <div>
            <strong>Apply works. Deploy waits for a secret</strong>
            <span>{agentThread.footer.owner.message}</span>
            {agentThread.footer.owner.detail && <span className="gate-refusal-detail">{agentThread.footer.owner.detail}</span>}
          </div>
          <button type="button" onClick={() => { setAuthView("integrations"); reveal("auth"); }}>Open Auth &gt; Integrations</button>
        </div>
      )}
      <nav className="section-nav" aria-label="Workspace sections">
        <a className="usage-open-link" href={activeDeployment ? `/usage?deployment=${encodeURIComponent(activeDeployment.id)}` : "/usage"}>Usage</a>
        {sections.filter((section) => !compact || section.id === "preview" || section.id === "agent").map((section) => {
          const Icon = section.icon;
          const door = section.id === "preview" || section.id === "agent";
          const pressed = compact ? compactPanel === section.id : isVisible(layout, section.id);
          return (
            <button
              key={section.id}
              className={`${door ? "section-door" : "section-tool"} ${pressed ? "active" : ""}`}
              aria-pressed={pressed}
              onClick={() => reveal(section.id)}
            >
              <Icon size={13} /> {section.label}
            </button>
          );
        })}
        {compact && sections.filter((section) => section.id !== "preview" && section.id !== "agent").map((section) => (
          <button
            key={section.id}
            className={`section-tool ${compactPanel === section.id ? "active" : ""}`}
            aria-pressed={compactPanel === section.id}
            onClick={() => reveal(section.id)}
          >
            {section.label}
          </button>
        ))}
      </nav>

      <WorkbenchLayout layout={displayedLayout} compact={compact} onLayoutChange={updateLayout} panels={[codePanel, capabilityPanel("data", "Data", <Database size={11} />), capabilityPanel("files", "Uploads", <FolderOpen size={11} />), capabilityPanel("auth", "Auth", <UserRound size={11} />), activityPanel, previewPanel, agentPanel]} />

      <AgentSettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        sessionConfig={agentSession}
        onSessionConfigChange={setAgentSession}
        keyStore={keyStore}
        onKeyStoreChange={() => setKeyVersion((version) => version + 1)}
        providers={PROVIDERS}
      />

      {appSettingsOpen && <Suspense fallback={<Dialog title="App settings" onClose={() => setAppSettingsOpen(false)}><div className="ui-dialog-body"><LoadingBlock label="Loading app settings" rows={3} /></div></Dialog>}><AppSettingsDialog key={`${project.id}-${domainVersion}`} projectName={project.name} deploymentId={activeDeployment?.id} importTarget={activeDeployment ? { project: { id: project.id, name: project.name, slug: project.slug }, visibility: activeDeployment.manifest.visibility, onImported: () => window.location.reload() } : undefined} onClose={() => setAppSettingsOpen(false)} onDeploy={() => { setAppSettingsOpen(false); setDeployment(null); setDeployError(""); setDeployOpen(true); }} onSignedOut={refreshOwnerSession} onAddressMoved={(url) => setDeployments((current) => current[project.id] ? { ...current, [project.id]: { ...current[project.id]!, url } } : current)} signInHref={signInHref} /></Suspense>}

      {platformConfirmation && <Dialog title="Allow the agent to do this?" description={project.name} onClose={() => { resolvePlatformConfirmation.current?.(false); resolvePlatformConfirmation.current = null; setPlatformConfirmation(null); }} footer={<>
        <Button onClick={() => { resolvePlatformConfirmation.current?.(false); resolvePlatformConfirmation.current = null; setPlatformConfirmation(null); }}>Cancel</Button>
        <Button variant="primary" onClick={() => { resolvePlatformConfirmation.current?.(true); resolvePlatformConfirmation.current = null; setPlatformConfirmation(null); }}>Allow</Button>
      </>}><div className="ui-dialog-body"><p style={{ overflowWrap: "anywhere" }}>{platformConfirmation.summary}</p><p>It runs as you, on this app only, through <code>{platformConfirmation.tool}</code>.</p></div></Dialog>}

      {deployOpen && <DeployDialog project={project} deployment={deployment} publishing={publishing} error={deployError} errorCode={deployErrorCode} session={ownerSession} signInHref={signInHref} onClose={() => { resolveAgentPublish.current?.(deployError ? { status: "failed", error: deployError } : { status: "declined" }); resolveAgentPublish.current = null; setDeployOpen(false); }} current={activeDeployment} onOpenAppSettings={() => { setDeployOpen(false); setAppSettingsOpen(true); }} onDeploy={(visibility, signupMode) => void deploy(visibility, signupMode)} onEditAuth={() => { setDeployOpen(false); setAuthView("rules"); reveal("auth"); }} onOpenUsers={() => { setDeployOpen(false); setAuthView("users"); reveal("auth"); }} onOpenIntegrations={() => { setDeployOpen(false); setAuthView("integrations"); reveal("auth"); }} />}
      {/* After the Deploy dialog, so it paints over it. */}
      {schemaChange && <SchemaChangeDialog change={schemaChange.change} onAnswer={(confirmed) => { schemaChange.resolve(confirmed); setSchemaChange(null); }} />}

      <Dialog
        open={resetOpen}
        title={`Delete the rows and files in ${project.name}?`}
        description={`This deletes the rows and files of ${project.name} on this computer and puts the starter back. A published copy stays.`}
        onClose={() => setResetOpen(false)}
        footer={<>
          <Button onClick={() => setResetOpen(false)}>Cancel</Button>
          <Button variant="danger" className="reset-confirm" onClick={() => { setResetOpen(false); resetProject(); }}><RotateCcw size={14} /> Delete the rows and files</Button>
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

/** Panels the workbench lays out. The File list is drawn inside Files instead. */
const workbenchPanels: PanelId[] = panelIds.filter((id) => id !== "explorer");

/** Preview in view: beside Files when Files is showing, otherwise brought forward where it is. */
function withPreviewShowing(current: LayoutState): LayoutState {
  if (isVisible(current, "preview")) return current;
  if (!isOpen(current, "preview") && isVisible(current, "code")) {
    return movePanel(current, "preview", { target: "code", zone: "right" });
  }
  return openPanel(current, "preview");
}

function focusRunningApp(current: LayoutState, target: PanelId): LayoutState {
  let next = current;
  for (const id of panelIds) {
    if (id !== target && isOpen(next, id)) next = closePanel(next, id);
  }
  return openPanel(next, target);
}

function listedActions(source: string): number {
  try {
    return Object.keys(parseServerActionSource(source).actions).length;
  } catch {
    return 0;
  }
}

function ActionReadout({ source, visibility }: { source: string; visibility: "public" | "private" }) {
  const { refusal, lines } = actionReadout(source, visibility);
  return (
    <div className="action-readout-block">
      <p className={`action-check ${refusal ? "is-refused" : ""}`}>
        {refusal ?? "A list that reads rows needs a LIMIT. Every query here names one, so they can leave."}
      </p>
      {lines.length > 0 && (
        <ul className="action-readout">
          {lines.map((line) => <li key={line}><strong className="action-limit">{line}</strong></li>)}
        </ul>
      )}
    </div>
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

function AccountChip({ session, href }: { session: OwnerSession; href: string }) {
  const initials = ownerInitials(session);
  const label = session.state === "signed-in"
    ? `Signed in${session.email ? ` as ${session.email}` : ""}. Open owner sign-in`
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
