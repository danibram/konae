import {
  Box,
  Check,
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
  Laptop,
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
import { lazy, Suspense, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { PROVIDERS, defaultSessionConfig } from "./agent/catalog";
import { mergeProposal } from "./agent/apply";
import type { AgentProposal, AgentSessionConfig } from "./agent/contracts";
import type { LittleboxAgentHandle } from "./agent/harness";
import { createAgentKeyStore } from "./agent/key-store";
import { AgentPanel } from "./components/AgentPanel";
import { AgentSettingsModal } from "./components/AgentSettingsModal";
import { useAgentThread } from "./components/useAgentThread";
import { focusPanelTab, WorkbenchLayout, type PanelRender } from "./components/WorkbenchLayout";
import { isOpen, isVisible, loadLayout, openPanel, saveLayout, togglePanel, type LayoutState, type PanelId } from "./ui/layout";
import { inTextEntry, shortcutFor } from "./ui/shortcuts";
import { DeployDialog } from "./components/DeployDialog";
import { ProjectDashboard } from "./components/ProjectDashboard";
import { deployProject, HostedDatabaseClient, type DeploymentReceipt } from "./deployments/client";
import type { DeploymentVisibility } from "./deployments/manifest";
import { createSchemaSnapshot } from "./database-lifecycle/schema-snapshot";
import { applyLocalAgentEdit } from "./lib/agent";
import { parseRoute, pushRoute } from "./lib/routing";
import { withPrivateNotes } from "./lib/private-notes";
import { cloneStarter } from "./lib/starter";
import type { MiniAppProject, ProjectFile } from "./lib/types";
import { createDirectory, createFile, directoriesForFiles, removePath } from "./lib/project-files";
import { createWorkspaceProject, loadWorkspaceState, persistWorkspace, resetWorkspaceProject } from "./lib/workspace";
import { createPreviewDocument } from "./runtime/preview";
import { buildProjectInBrowser, compileProjectInBrowser } from "./runtime/browser-build";
import { useRuntimeBridge } from "./runtime/useRuntimeBridge";

const CapabilityWorkspace = lazy(() =>
  import("./components/CapabilityWorkspace").then((module) => ({ default: module.CapabilityWorkspace })),
);

type EditorFile = string;

const knownFileMeta: Record<string, { label: string; icon: typeof FileText; language: string }> = {
  "index.html": { label: "index.html", icon: FileCode2, language: "HTML" },
  "styles.css": { label: "styles.css", icon: FileText, language: "CSS" },
  "app.js": { label: "app.js", icon: FileJson2, language: "JAVASCRIPT" },
  "schema.ts": { label: "schema.ts", icon: Database, language: "DRIZZLE · TYPESCRIPT" },
  "actions.json": { label: "actions.json", icon: ShieldCheck, language: "SERVER ACTIONS" },
};

export function App() {
  const [initialWorkspace] = useState(loadWorkspaceState);
  const [projects, setProjects] = useState(initialWorkspace.projects);
  const [deployments, setDeployments] = useState(initialWorkspace.deployments);
  const [project, setProject] = useState(() => initialWorkspace.projects.find((item) => item.id === initialWorkspace.activeProjectId) ?? initialWorkspace.projects[0] ?? cloneStarter());
  const [previewProject, setPreviewProject] = useState(project);
  const [previewEpoch, setPreviewEpoch] = useState(0);
  const [route, setRoute] = useState(() => parseRoute(window.location.pathname));
  const screen = route.screen;
  const [activeFile, setActiveFile] = useState<EditorFile>("app.js");
  const [selectedPath, setSelectedPath] = useState("app.js");
  const [newEntry, setNewEntry] = useState<"file" | "folder" | null>(null);
  const [newEntryPath, setNewEntryPath] = useState("");
  const [treeError, setTreeError] = useState("");
  const [viewport, setViewport] = useState<"desktop" | "mobile">("desktop");
  const [deployOpen, setDeployOpen] = useState(false);
  const [deployment, setDeployment] = useState<DeploymentReceipt | null>(null);
  const [databaseTarget, setDatabaseTarget] = useState<"local" | "deployed">("local");
  const [exporting, setExporting] = useState(false);
  const [publishing, setPublishing] = useState(false);
  const [deployError, setDeployError] = useState("");
  const [saved, setSaved] = useState(true);
  const [saveError, setSaveError] = useState("");
  const [operationError, setOperationError] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [agent, setAgent] = useState<LittleboxAgentHandle | null>(null);
  const [agentSession, setAgentSession] = useState<AgentSessionConfig>(defaultSessionConfig);
  const [threadResetToken, setThreadResetToken] = useState(0);
  const [keyVersion, setKeyVersion] = useState(0);
  const agentRef = useRef<LittleboxAgentHandle | null>(null);
  const agentLoad = useRef<Promise<LittleboxAgentHandle | null> | null>(null);
  const projectRef = useRef(project);
  const workspaceRef = useRef({ projects, project, deployments });
  workspaceRef.current = { projects, project, deployments };

  useEffect(() => {
    const flush = () => {
      const current = workspaceRef.current;
      const items = current.projects.some((item) => item.id === current.project.id)
        ? current.projects.map((item) => item.id === current.project.id ? current.project : item)
        : [...current.projects, current.project];
      try { persistWorkspace(items, current.project.id, current.deployments); }
      catch { /* The regular save reports storage failures in the editor. */ }
    };
    window.addEventListener("pagehide", flush);
    window.addEventListener("beforeunload", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("beforeunload", flush);
    };
  }, []);

  const gutterRef = useRef<HTMLDivElement>(null);
  const editorRef = useRef<HTMLTextAreaElement>(null);
  const keyStore = useMemo(() => createAgentKeyStore(), []);
  const [layout, setLayout] = useState<LayoutState>(loadLayout);

  const updateLayout = (next: LayoutState) => {
    setLayout(next);
    saveLayout(next);
  };
  const [previewFrame, setPreviewFrame] = useState<HTMLIFrameElement | null>(null);
  const hostRef = useRuntimeBridge(previewFrame, project.id, previewProject.files["schema.sql"], previewProject.files["actions.json"]);

  const previewDocument = useMemo(() => createPreviewDocument(previewProject), [previewProject]);
  const activeDeployment = deployments[project.id] ?? null;
  const deployedDatabase = useMemo(() => activeDeployment ? new HostedDatabaseClient(activeDeployment.id) : undefined, [activeDeployment]);
  const deployedModel = activeDeployment?.databaseModel ?? project.databaseModel;
  const targetDatabaseModel = databaseTarget === "deployed" && activeDeployment
    ? { ...deployedModel, source: project.databaseModel.source }
    : project.databaseModel;
  const activeSource = activeFile === "schema.ts" ? project.databaseModel.source : project.files[activeFile] ?? "";
  const activeMeta = fileMetaFor(activeFile);
  const lines = activeSource.split("\n").length;

  useEffect(() => {
    setProjects((current) => {
      const index = current.findIndex((item) => item.id === project.id);
      if (index < 0) return [...current, project];
      if (current[index] === project) return current;
      return current.map((item) => item.id === project.id ? project : item);
    });
    setSaved(false);
  }, [project]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      try {
        persistWorkspace(projects, project.id, deployments);
        setSaved(true);
        setSaveError("");
      } catch (error) {
        // A full or unavailable localStorage must say so. Reporting "Saving"
        // forever, or worse "Saved locally", would hide that the work of the
        // whole session is only in memory.
        setSaveError(error instanceof Error ? error.message : String(error));
      }
    }, 350);
    return () => window.clearTimeout(timeout);
  }, [deployments, project, projects]);

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
        runPreview();
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
    setProject((current) => activeFile === "schema.ts" ? {
      ...current,
      updatedAt: Date.now(),
      databaseModel: { ...current.databaseModel, source: value },
    } : {
      ...current,
      updatedAt: Date.now(),
      files: { ...current.files, [activeFile]: value },
    });
  };

  const addTreeEntry = () => {
    if (!newEntry) return;
    try {
      setProject((current) => {
        const tree = { files: current.files, directories: current.directories };
        const next = newEntry === "file" ? createFile(tree, newEntryPath) : createDirectory(tree, newEntryPath);
        return { ...current, ...next, updatedAt: Date.now() };
      });
      if (newEntry === "file") {
        setActiveFile(newEntryPath.trim().replace(/^\/+/, ""));
        setSelectedPath(newEntryPath.trim().replace(/^\/+/, ""));
      }
      setNewEntry(null);
      setNewEntryPath("");
      setTreeError("");
    } catch (error) {
      setTreeError(error instanceof Error ? error.message : String(error));
    }
  };

  const deleteSelectedPath = () => {
    if (!selectedPath || protectedProjectPaths.has(selectedPath)) return;
    setProject((current) => ({
      ...current,
      ...removePath({ files: current.files, directories: current.directories }, selectedPath),
      updatedAt: Date.now(),
    }));
    if (activeFile === selectedPath || activeFile.startsWith(`${selectedPath}/`)) setActiveFile("app.js");
    setSelectedPath("app.js");
  };

  const acceptDatabaseModel = async (source: string, revision: string) => {
    const projectId = project.id;
    const deployed = databaseTarget === "deployed" && deployedDatabase;
    const database = deployed || hostRef.current;
    const schema = await createSchemaSnapshot(database);
    const acceptedModel = { source, acceptedSource: source, revision };
    setProject((current) => current.id !== projectId ? current : ({
      ...current,
      updatedAt: Date.now(),
      files: { ...current.files, "schema.sql": schema },
      databaseModel: deployed ? { ...current.databaseModel, source } : acceptedModel,
    }));
    if (deployed) setDeployments((current) => ({
      ...current,
      [projectId]: { ...current[projectId], databaseModel: acceptedModel },
    }));
  };

  const runPreview = () => {
    setOperationError("");
    void compileProjectInBrowser(project)
      .then((compiled) => {
        setPreviewProject({
          ...project,
          files: { ...project.files, "index.html": compiled.body, "styles.css": compiled.styles, "app.js": compiled.script },
        });
        setPreviewEpoch((value) => value + 1);
      })
      .catch((error) => setOperationError(`Build failed: ${error instanceof Error ? error.message : String(error)}`));
  };

  useEffect(() => {
    let cancelled = false;
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
   * ADR-0003 lever 7: pi-agent-core, the provider catalogs and sql.js are imported
   * on the first prompt, never at editor start.
   */
  const ensureAgent = () => {
    if (agentRef.current) return Promise.resolve<LittleboxAgentHandle | null>(agentRef.current);
    if (!agentLoad.current) {
      agentLoad.current = import("./agent/harness")
        .then(({ createLittleboxAgent }) => {
          const handle = createLittleboxAgent(projectRef.current, agentSession, keyStore, {
            // The Validation Gate's migration stage needs the live database, not just a fresh one.
            liveDatabase: () => hostRef.current.exportDatabase(),
            // The other half of the gate: what the running preview actually threw.
            previewErrors: () => hostRef.current.previewErrors(),
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
    // A link to a project this browser does not have falls back to the list.
    if (!target) {
      setRoute({ screen: "projects" });
      return;
    }
    setProject(target);
    setPreviewProject({ ...target, files: { ...target.files } });
    setThreadResetToken((token) => token + 1);
  }, [route, projects, project.id]);

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
    providers: PROVIDERS,
  });

  /** Offline fallback: the scripted demo answers until a provider key is stored. */
  const sendDemoPrompt = async (value: string) => {
    await new Promise((resolve) => window.setTimeout(resolve, 450));
    const edit = applyLocalAgentEdit(projectRef.current, value);
    setProject(edit.project);
    setPreviewProject(edit.project);
    if (edit.changedFiles[0] && isEditorFile(edit.changedFiles[0])) {
      setActiveFile(edit.changedFiles[0]);
      }
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
      if (project.databaseModel.source !== (activeDeployment ? deployedModel : project.databaseModel).acceptedSource) {
        throw new Error("Apply or revert the schema changes for the exported database first.");
      }
      const artifact = await buildProjectInBrowser(project);
      const state = activeDeployment && deployedDatabase
        ? await deployedDatabase.exportPortableState()
        : { initialDatabase: await hostRef.current.exportDatabase() };
      const result = await createProjectExport(project, artifact, state);
      downloadBlob(result.blob, result.filename);
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

  const deploy = async (visibility: DeploymentVisibility) => {
    setPublishing(true);
    setDeployError("");
    try {
      const model = activeDeployment ? deployedModel : project.databaseModel;
      if (project.databaseModel.source !== model.acceptedSource) {
        throw new Error(activeDeployment
          ? "Apply the schema migration to the deployed database in Data before publishing."
          : "Apply or revert the local schema changes before publishing.");
      }
      const artifact = await buildProjectInBrowser(project);
      const record = { ...await deployProject(project, visibility, artifact), databaseModel: { ...model } };
      setProject((current) => ({ ...current, manifest: record.manifest, updatedAt: Date.now() }));
      setDeployment(record);
      setDeployments((current) => ({ ...current, [project.id]: record }));
      setDatabaseTarget("deployed");
    } catch (error) {
      setDeployError(error instanceof Error ? error.message : String(error));
    } finally {
      setPublishing(false);
    }
  };

  const openEditorRoute = (projectId: string) => {
    const next = { screen: "editor" as const, projectId };
    setRoute(next);
    pushRoute(next);
  };

  const goToProjects = () => {
    const next = { screen: "projects" as const };
    setRoute(next);
    pushRoute(next);
  };

  const openProject = (projectId: string) => {
    const next = projects.find((item) => item.id === projectId);
    if (!next) return;
    setProject(next);
    setPreviewProject({ ...next, files: { ...next.files } });
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setDatabaseTarget("local");
    openEditorRoute(next.id);
  };

  const createProject = (name: string, template: "tasks" | "notes" = "tasks") => {
    const base = createWorkspaceProject(name, projects);
    const next = template === "notes" ? withPrivateNotes(base) : base;
    setProjects((current) => [...current, next]);
    setProject(next);
    setPreviewProject(next);
    setThreadResetToken((token) => token + 1);
    setActiveFile("app.js");
    setDatabaseTarget("local");
    openEditorRoute(next.id);
  };

  if (screen === "projects") {
    return <ProjectDashboard projects={projects} deployments={deployments} onOpenProject={openProject} onCreateProject={createProject} />;
  }

  const sections: { id: PanelId; label: string; icon: typeof Code2 }[] = [
    { id: "code", label: "Code", icon: Code2 },
    { id: "data", label: "Data", icon: Database },
    { id: "files", label: "Files", icon: FolderOpen },
    { id: "auth", label: "Auth", icon: UserRound },
  ];

  const reveal = (panel: PanelId) => updateLayout(openPanel(layout, panel));

  const explorerPanel: PanelRender = {
    id: "explorer",
    title: project.slug,
    icon: <FolderOpen size={12} />,
    body: (
      <nav className="explorer-panel" aria-label="Project files">
        <div className="explorer-heading">
          <p className="explorer-group">PROJECT</p>
          <button aria-label="New file" title="New file" onClick={() => { setNewEntry("file"); setTreeError(""); }}><FilePlus2 size={12} /></button>
          <button aria-label="New folder" title="New folder" onClick={() => { setNewEntry("folder"); setTreeError(""); }}><FolderPlus size={12} /></button>
          <button aria-label="Delete selected path" title="Delete selected path" disabled={!selectedPath || protectedProjectPaths.has(selectedPath)} onClick={deleteSelectedPath}><Trash2 size={12} /></button>
        </div>
        {newEntry && <div className="explorer-create">
          <label htmlFor="new-project-path">{newEntry === "file" ? "File path" : "Folder path"}</label>
          <input id="new-project-path" autoFocus value={newEntryPath} placeholder={newEntry === "file" ? "src/features/tasks.ts" : "src/features"} onChange={(event) => setNewEntryPath(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") addTreeEntry(); if (event.key === "Escape") setNewEntry(null); }} />
          <button onClick={addTreeEntry}>Create</button>
          <button onClick={() => setNewEntry(null)}>Cancel</button>
          {treeError && <span role="alert">{treeError}</span>}
        </div>}
        {[...project.directories].sort().map((directory) => (
          <button key={`dir:${directory}`} className={`tree-directory ${selectedPath === directory ? "selected" : ""}`} style={{ paddingLeft: `${8 + directory.split("/").length * 12}px` }} onClick={() => setSelectedPath(directory)}>
            <FolderOpen size={13} /> {directory.split("/").at(-1)}
          </button>
        ))}
        {[...Object.keys(project.files), "schema.ts"].filter((file, index, all) => all.indexOf(file) === index).sort().map((file) => {
          const meta = fileMetaFor(file);
          const Icon = meta.icon;
          return (
            <button
              key={file}
              className={activeFile === file ? "active" : ""}
              style={{ paddingLeft: `${8 + file.split("/").length * 12}px` }}
              onClick={() => { setActiveFile(file); setSelectedPath(file); reveal("code"); }}
            >
              <Icon size={13} /> {meta.label}
            </button>
          );
        })}
      </nav>
    ),
  };

  const codePanel: PanelRender = {
    id: "code",
    title: activeMeta.label,
    icon: <Code2 size={11} />,
    actions: (
      <>
        <span className="panel-meta">{activeMeta.language} · {lines} lines</span>
        <button className="active" onClick={runPreview}><Play size={11} fill="currentColor" /> Run</button>
      </>
    ),
    body: (
      <div className="code-panel">
        <div className="code-area">
          <div className="line-numbers" aria-hidden="true" ref={gutterRef}>
            {Array.from({ length: lines }, (_, index) => <span key={index}>{index + 1}</span>)}
          </div>
          <textarea
            ref={editorRef}
            value={activeSource}
            onChange={(event) => updateFile(event.target.value)}
            // The textarea is the only scroller; the gutter is dragged along by
            // hand, or every number is wrong from the first scroll onwards.
            onScroll={(event) => {
              if (gutterRef.current) gutterRef.current.scrollTop = event.currentTarget.scrollTop;
            }}
            spellCheck={false}
            aria-label={`${activeFile} code editor`}
          />
        </div>
        <footer className="editor-status">
          <span><Wifi size={12} /> runtime local</span>
          <span>UTF-8</span>
          <span className="editor-status-right"><span className="status-light" /> SQLite connected</span>
        </footer>
      </div>
    ),
  };

  const capabilityPanel = (id: "data" | "files" | "auth", label: string, icon: ReactNode): PanelRender => ({
    id,
    title: label,
    icon,
    actions: id === "data" ? <button onClick={() => void exportDatabase()}><Download size={11} /> Export</button> : undefined,
    body: (
      <Suspense fallback={<div className="capability-loading"><RefreshCw className="spin" size={17} /> Loading…</div>}>
        <CapabilityWorkspace
          view={id === "data" ? "database" : id}
          host={hostRef.current}
          databaseModel={targetDatabaseModel}
          onEditSchema={() => { setActiveFile("schema.ts"); reveal("code"); }}
          onDatabaseModelAccepted={acceptDatabaseModel}
          onDatabaseModelRestored={acceptDatabaseModel}
          onExportDatabase={() => void exportDatabase()}
          onFilesChanged={runPreview}
          deployedDatabase={deployedDatabase}
          databaseTarget={databaseTarget}
          deployedLabel={activeDeployment?.url.replace(/^https?:\/\//, "")}
          onDatabaseTargetChange={setDatabaseTarget}
        />
      </Suspense>
    ),
  });

  const previewPanel: PanelRender = {
    id: "preview",
    title: `${project.slug}.local`,
    icon: <Play size={12} />,
    actions: (
      <>
        <button className={viewport === "desktop" ? "active" : ""} title="Desktop" onClick={() => setViewport("desktop")}><Laptop size={12} /></button>
        <button className={viewport === "mobile" ? "active" : ""} title="Mobile" onClick={() => setViewport("mobile")}><Smartphone size={12} /></button>
        <button title="Reload preview" onClick={runPreview}><RefreshCw size={11} /></button>
      </>
    ),
    body: (
      <div className={`preview-canvas ${viewport}`}>
        <iframe key={`${project.id}:${previewEpoch}`} ref={setPreviewFrame} title="Mini-app preview" sandbox="allow-scripts" srcDoc={previewDocument} />
      </div>
    ),
  };

  const agentPanel: PanelRender = {
    id: "agent",
    title: "Builder agent",
    icon: <Sparkles size={12} />,
    actions: (
      <button title="Agent settings" onClick={() => setSettingsOpen(true)}><Settings2 size={11} /></button>
    ),
    body: <AgentPanel thread={agentThread} sessionConfig={agentSession} onOpenSettings={() => setSettingsOpen(true)} />,
  };

  return (
    <div className="workshop">
      <header className="topbar">
        <button className="brand" aria-label="Littlebox home" onClick={() => goToProjects()}>
          <span className="brand-mark"><Box size={17} strokeWidth={2.4} /></span>
          <span>littlebox</span>
          <span className="alpha-tag">ALPHA</span>
        </button>

        <button className="project-switcher" onClick={() => goToProjects()}>
          <span className="project-dot" />
          <strong>{project.name}</strong>
          <ChevronDown size={14} />
        </button>

        <div className="top-actions">
          <span
            className={`save-state ${saveError ? "is-failed" : saved ? "is-saved" : ""}`}
            title={saveError || undefined}
            role={saveError ? "alert" : undefined}
          >
            {saveError ? <CircleAlert size={12} /> : saved ? <Check size={12} /> : <RefreshCw size={12} className="spin" />}
            {saveError ? "Not saved locally" : saved ? "Saved locally" : "Saving"}
          </span>
          <button className="icon-button" title={exporting ? "Exporting project" : "Export project"} disabled={exporting} onClick={() => void exportProject()}><Download size={16} /></button>
          <button className="icon-button" title="Reset local prototype" onClick={resetProject}><RotateCcw size={15} /></button>
          <button className="deploy-button" onClick={() => { setDeployment(null); setDeployError(""); setDeployOpen(true); }}><Rocket size={15} /> Deploy</button>
          <button className="avatar" aria-label="Account" onClick={() => goToProjects()}>DR</button>
        </div>
      </header>

      {operationError && <div className="operation-error" role="alert">{operationError}<button onClick={() => setOperationError("")}>Dismiss</button></div>}
      <nav className="section-nav" aria-label="Workspace sections">
        {sections.map((section, index) => {
          const Icon = section.icon;
          return (
            <button
              key={section.id}
              className={`${isVisible(layout, section.id) ? "active" : ""} ${isOpen(layout, section.id) ? "is-open" : ""}`}
              onClick={() => reveal(section.id)}
            >
              <Icon size={13} /> {section.label}
              <kbd>⌘{index + 1}</kbd>
            </button>
          );
        })}
        <span className="section-nav-spacer" />
        {/* Every panel has a way back: a closed one is reopened from here. */}
        <div className="section-toggles">
          {([
            { id: "explorer", label: "Explorer", icon: FolderOpen, key: "⌘B" },
            { id: "preview", label: "Preview", icon: Play, key: "⌘U" },
            { id: "agent", label: "Agent", icon: Sparkles, key: "⌘J" },
          ] as { id: PanelId; label: string; icon: typeof Code2; key: string }[]).map((view) => {
            const Icon = view.icon;
            const open = isOpen(layout, view.id);
            return (
              <button
                key={view.id}
                className={open ? "on" : ""}
                aria-pressed={open}
                title={`${view.label} · ${view.key}`}
                onClick={() => updateLayout(togglePanel(layout, view.id))}
              >
                <Icon size={12} /> {view.label}
              </button>
            );
          })}
        </div>
      </nav>

      <WorkbenchLayout layout={layout} onLayoutChange={updateLayout} panels={[explorerPanel, codePanel, capabilityPanel("data", "Data", <Database size={11} />), capabilityPanel("files", "Files", <FolderOpen size={11} />), capabilityPanel("auth", "Auth", <UserRound size={11} />), previewPanel, agentPanel]} />

      <AgentSettingsModal
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        sessionConfig={agentSession}
        onSessionConfigChange={setAgentSession}
        keyStore={keyStore}
        onKeyStoreChange={() => setKeyVersion((version) => version + 1)}
        providers={PROVIDERS}
      />

      {deployOpen && <DeployDialog project={project} deployment={deployment} publishing={publishing} error={deployError} onClose={() => setDeployOpen(false)} onDeploy={(visibility) => void deploy(visibility)} />}
    </div>
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

const protectedProjectPaths = new Set(["index.html", "styles.css", "app.js", "schema.sql", "actions.json", "schema.ts"]);

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
