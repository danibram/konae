import { ArrowRight, BarChart3, Check, Code2, Copy, Download, MoreHorizontal, Plus, Search, Sparkles, Trash2 } from "lucide-react";
import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { Alert, Button, Dialog, EmptyState, Field, Input, Menu, Select, Textarea, ToastRegion, type MenuItem, type ToastItem } from "./ui";
import type { DeploymentIndex } from "../lib/workspace";
import type { MiniAppProject } from "../lib/types";
import type { OwnerSession } from "./owner-session";
import type { ProjectMetadata } from "../lib/project-sources";
import { WorkspaceNav } from "./WorkspaceNav";
import { AppStatus, UndeployedMarker, useRelativeTime } from "./AppStatus";
import { changesNotDeployed, releasedAt } from "../deployments/deployed-changes";
import { useSourceDigests } from "../deployments/source-digest";
import { groupProjects, listOrder, nameFromDescription, projectEntries, uniqueName, type ProjectEntry, type ProjectSort } from "./project-list";
import { exactTime } from "../lib/relative-time";
import { defaultTemplate, templateChoices, type TemplateId } from "../lib/templates";
import "./project-list.css";

type Props = {
  projects: MiniAppProject[];
  deployments: DeploymentIndex;
  ownerSession: OwnerSession;
  missing: ProjectMetadata[];
  notice?: string;
  onDeleteMissing: (project: ProjectMetadata, name: string) => Promise<void>;
  onDeleteProject?: (project: ProjectMetadata, name: string) => Promise<void>;
  onOpenProject: (projectId: string, entry?: "preview" | "agent" | "code") => void;
  onCreateProject: (name: string, template: TemplateId | "scratch", description?: string) => void;
  modelConnected?: boolean;
  returnFocusTo?: string;
};


const plainClick = (event: MouseEvent) => event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;

export function ProjectDashboard({ projects, deployments, ownerSession, missing, notice, onDeleteMissing, onDeleteProject, onOpenProject, onCreateProject, modelConnected = false, returnFocusTo }: Props) {
  const entries = useMemo(() => projectEntries(projects, missing, deployments), [projects, missing, deployments]);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<ProjectSort>("edited");
  const groups = useMemo(() => groupProjects(entries, query, sort), [entries, query, sort]);
  const [deleting, setDeleting] = useState<ProjectEntry | null>(null);
  const [creating, setCreating] = useState<{ template: TemplateId | null } | null>(null);
  const [exporting, setExporting] = useState("");
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const toast = (message: string, tone: ToastItem["tone"] = "success") => {
    const id = Date.now();
    setToasts((current) => [...current, { id, message, tone }]);
    window.setTimeout(() => setToasts((current) => current.filter((item) => item.id !== id)), 4000);
  };
  useEffect(() => { document.title = "Projects · Konae"; }, []);
  useEffect(() => {
    if (!returnFocusTo) return;
    const card = document.querySelector<HTMLElement>(`[data-project-id="${CSS.escape(returnFocusTo)}"] .project-card-link`);
    (card ?? document.querySelector<HTMLElement>(".project-list-head h1"))?.focus();
  }, []);

  const digests = useSourceDigests(projects);
  const undeployed = (entry: ProjectEntry) => {
    const project = entry.deployment && projects.find((item) => item.id === entry.id);
    return Boolean(project && changesNotDeployed(project, entry.deployment, digests.get(project.id) ?? null));
  };
  const open = (entry: ProjectEntry, door: "preview" | "agent" | "code" = "preview") => {
    onOpenProject(entry.id, door);
  };
  const copyAddress = (url: string) => {
    void navigator.clipboard?.writeText(url)
      .then(() => toast("Address copied"))
      .catch(() => toast("Could not copy. Select the address on the card instead.", "danger"));
  };
  const exportApp = async (entry: ProjectEntry) => {
    const project = projects.find((item) => item.id === entry.id);
    if (!project || exporting) return;
    setExporting(entry.id);
    toast(`Exporting ${entry.name}…`, "info");
    try {
      const { createProjectExport, exportedMessage } = await import("../lib/export-project");
      const result = await createProjectExport(project, entry.deployment?.id);
      const url = URL.createObjectURL(result.blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = result.filename;
      anchor.click();
      URL.revokeObjectURL(url);
      toast(exportedMessage(result.filename, Boolean(entry.deployment), result.sourcesAhead));
    } catch (error) {
      toast(`Export failed: ${error instanceof Error ? error.message : String(error)}`, "danger");
    } finally {
      setExporting("");
    }
  };
  const deleteHandler = deleting ? (deleting.sourceError ? onDeleteMissing : onDeleteProject) : undefined;
  const focusAfterDelete = (removed: ProjectEntry) => {
    const order = listOrder(entries).filter((entry) => groups.published.includes(entry) || groups.drafts.includes(entry));
    const at = order.findIndex((entry) => entry.id === removed.id);
    const next = order[at + 1] ?? order[at - 1];
    window.setTimeout(() => {
      const target = next && document.querySelector<HTMLElement>(`[data-project-id="${CSS.escape(next.id)}"] .project-card-link`);
      (target ?? document.querySelector<HTMLElement>(".project-list-head h1"))?.focus();
    }, 0);
  };

  return (
    <div className="account-home">
      <WorkspaceNav active="projects" projectCount={entries.length} ownerSession={ownerSession} />

      <main className="account-main">
        <section className="account-content project-list">
          <header className="project-list-head">
            <h1 tabIndex={-1}>Projects</h1>
            <Button variant="primary" onClick={() => setCreating({ template: null })}><Plus size={15} aria-hidden="true" /> New project</Button>
          </header>
          {notice && <Alert className="project-list-notice" tone="info">{notice}</Alert>}

          {entries.length === 0
            ? <EmptyState
              className="project-list-empty"
              icon={<Sparkles size={22} />}
              title="No projects yet"
              description="Say what you need in plain words, and Konae makes an app that runs."
              action={<div className="project-list-empty-actions">
                <Button variant="primary" onClick={() => setCreating({ template: null })}><Plus size={15} aria-hidden="true" /> Describe your first app</Button>
                <p>Or start from a template</p>
                <div>{templateChoices.filter((choice) => choice.id !== defaultTemplate).slice(0, 3).map((choice) => <Button key={choice.id} size="sm" onClick={() => setCreating({ template: choice.id })}>{choice.name}</Button>)}</div>
              </div>}
            />
            : <>
              <div className="project-list-tools">
                <label className="project-list-search">
                  <Search size={15} aria-hidden="true" />
                  <span className="sr-only">Search projects</span>
                  <Input type="search" placeholder="Search projects" value={query} onChange={(event) => setQuery(event.target.value)} />
                </label>
                <label className="project-list-sort">
                  <span>Sort</span>
                  <Select value={sort} onChange={(event) => setSort(event.target.value as ProjectSort)}>
                    <option value="edited">Last edited</option>
                    <option value="name">Name</option>
                  </Select>
                </label>
              </div>
              {([["Published", groups.published], ["Drafts", groups.drafts]] as const).map(([title, group]) => group.length > 0 && <section className="project-list-group" aria-labelledby={`projects-${title}`} key={title}>
                <h2 id={`projects-${title}`}>{title}</h2>
                <ul className="project-cards">
                  {group.map((entry) => <ProjectCard
                    key={entry.id}
                    entry={entry}
                    onOpen={(door) => open(entry, door)}
                    onCopy={copyAddress}
                    onDelete={entry.sourceError ? () => setDeleting(entry) : onDeleteProject ? () => setDeleting(entry) : undefined}
                    onExport={entry.sourceError ? undefined : () => void exportApp(entry)}
                    exporting={exporting === entry.id}
                    undeployed={undeployed(entry)}
                  />)}
                </ul>
              </section>)}
              {!groups.published.length && !groups.drafts.length && <p className="project-list-none" role="status">No project matches "{query.trim()}".</p>}
            </>}
        </section>
      </main>
      {deleting && deleteHandler && <DeleteProjectDialog
        project={deleting}
        published={Boolean(deleting.deployment)}
        onClose={() => setDeleting(null)}
        onDelete={deleteHandler}
        onDeleted={(name) => { toast(`${name} deleted`); focusAfterDelete(deleting); }}
        onExport={deleting.sourceError ? undefined : () => void exportApp(deleting)}
        exporting={exporting === deleting.id}
      />}
      {creating && <NewProjectDialog
        initialTemplate={creating.template}
        modelConnected={modelConnected}
        takenNames={entries.map((entry) => entry.name)}
        onClose={() => setCreating(null)}
        onCreate={(name, template, description) => { setCreating(null); onCreateProject(name, template, description); }}
      />}
      <ToastRegion toasts={toasts} onDismiss={(id) => setToasts((current) => current.filter((item) => item.id !== id))} />
    </div>
  );
}

function ProjectCard({ entry, onOpen, onCopy, onDelete, onExport, exporting = false, undeployed = false }: {
  entry: ProjectEntry;
  undeployed?: boolean;
  onOpen: (door: "preview" | "agent" | "code") => void;
  onCopy: (url: string) => void;
  onDelete?: () => void;
  onExport?: () => void;
  exporting?: boolean;
}) {
  const { deployment } = entry;
  const editedTime = useRelativeTime(entry.editedAt);
  const opens = true;
  const href = `/app/${encodeURIComponent(entry.id)}`;
  const items: MenuItem[] = [
    ...[
      { label: "Describe a change", icon: <Sparkles size={14} />, onSelect: () => onOpen("agent") },
      { label: "Edit files", icon: <Code2 size={14} />, onSelect: () => onOpen("code") },
    ],
    ...(deployment ? [
      { label: "Copy address", icon: <Copy size={14} />, onSelect: () => onCopy(deployment.url) },
      { label: "Usage", icon: <BarChart3 size={14} />, onSelect: () => window.location.assign(`/usage?deployment=${encodeURIComponent(deployment.id)}`) },
    ] : []),
    ...(onExport ? [{ label: exporting ? "Exporting…" : "Export app folder", icon: <Download size={14} />, disabled: exporting, onSelect: onExport }] : []),
    ...(onDelete ? [{ label: "Delete…", icon: <Trash2 size={14} />, danger: true, onSelect: onDelete }] : []),
  ];
  return <li className="project-card-item" data-opens={opens || undefined} data-project-id={entry.id}>
    <h3>
      {opens
        ? <a className="project-card-link" href={href} onClick={(event) => { if (!plainClick(event)) return; event.preventDefault(); onOpen("preview"); }}>{entry.name}</a>
        : <span>{entry.name}</span>}
    </h3>
    {entry.purpose
      ? <p className="project-card-purpose">{entry.purpose}</p>
      : !entry.sourceError && <p className="project-card-purpose is-quiet">No description yet.</p>}
    {entry.sourceError && <p className="project-card-purpose is-quiet">The app source folder is missing or unreadable.</p>}
    <AppStatus className="project-card-status" url={deployment?.url} publishedAt={deployment ? releasedAt(deployment) : undefined} />
    <p className="project-card-meta">
      {entry.editedAt !== undefined && <time dateTime={new Date(entry.editedAt).toISOString()} title={exactTime(entry.editedAt)}>Edited {editedTime}</time>}
      {undeployed && <UndeployedMarker />}
    </p>
    {items.length > 0 && <div className="project-card-menu">
      <Menu label={`More for ${entry.name}`} trigger={<MoreHorizontal size={16} />} triggerClassName="project-card-more" items={items} />
    </div>}
  </li>;
}

function NewProjectDialog({ initialTemplate, modelConnected, takenNames, onClose, onCreate }: {
  initialTemplate: TemplateId | null;
  modelConnected: boolean;
  takenNames: readonly string[];
  onClose: () => void;
  onCreate: (name: string, template: TemplateId | "scratch", description?: string) => void;
}) {
  const describe = useRef<HTMLTextAreaElement>(null);
  const [description, setDescription] = useState("");
  const [name, setName] = useState("");
  const [picked, setPicked] = useState<TemplateId | "scratch" | null>(initialTemplate);
  const [tried, setTried] = useState(false);
  const [showTemplates, setShowTemplates] = useState(initialTemplate !== null);
  const described = description.trim();
  const template = picked ?? (described ? "scratch" : null);
  const chosen = templateChoices.find((choice) => choice.id === template);
  const missingStart = template === null;
  const baseName = name.trim() || nameFromDescription(description) || chosen?.name || (template === "scratch" ? "Untitled app" : "");
  const finalName = baseName ? uniqueName(baseName, takenNames) : "";
  const create = () => {
    setTried(true);
    if (template === null) { setShowTemplates(true); describe.current?.focus(); return; }
    onCreate(finalName || "Untitled app", template, described || undefined);
  };
  const modelNote = template === null ? ""
    : chosen ? `No model is connected, so the app starts from the ${chosen.name} template. What you ${described ? "wrote" : "write"} waits in Describe until you connect one.`
      : `No model is connected, so the app starts as a private list${described ? " with what you wrote at the top" : ""}. ${described ? "Your words also wait" : "What you write waits"} in Describe until you connect one.`;
  return <Dialog
    open
    className="new-project"
    title="New project"
    initialFocus={describe}
    onClose={onClose}
    footer={<>
      <Button onClick={onClose}>Cancel</Button>
      <Button variant="primary" type="submit" form="new-project">Create <ArrowRight size={14} aria-hidden="true" /></Button>
    </>}
  >
    <form id="new-project" className="new-project-body" onSubmit={(event) => { event.preventDefault(); create(); }}>
      <Field label="What should it do?" help={tried && missingStart ? "Describe what it should do, or pick a template below." : "In plain words, the way you would tell a person. For example, a sign-up sheet for Saturday football where people add their name and I see who is coming."}>
        <Textarea
          ref={describe}
          rows={4}
          value={description}
          aria-invalid={tried && missingStart ? true : undefined}
          onChange={(event) => setDescription(event.target.value)}
          onKeyDown={(event) => { if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); create(); } }}
          maxLength={2000}
        />
      </Field>
      {!modelConnected && modelNote && <p className="new-project-model" role="note">{modelNote}</p>}
      <Field label="Project name" help={finalName && finalName !== baseName ? `${baseName} is taken, so this one is ${finalName}.` : "Optional. Left empty, the name comes from the first words above."}>
        <Input value={name} onChange={(event) => setName(event.target.value)} placeholder={finalName} maxLength={80} />
      </Field>
      <details className="ui-disclosure new-project-more" open={showTemplates} onToggle={(event) => setShowTemplates(event.currentTarget.open)}>
        <summary>Start from a template instead{chosen && <span className="new-project-chosen">{chosen.name}</span>}</summary>
        <fieldset className="new-project-templates">
          <legend className="sr-only">Start from</legend>
          <p>A template runs as it is, and you change it from there.</p>
          <label className={template === "scratch" ? "is-selected" : undefined}>
            <input type="radio" name="project-template" value="scratch" checked={template === "scratch"} onChange={() => setPicked("scratch")} />
            <strong>No template</strong>
            <small>A plain list titled with the project name, with what you wrote at the top. Private.</small>
          </label>
          {templateChoices.map((choice) => (
            <label key={choice.id} className={template === choice.id ? "is-selected" : undefined}>
              <input type="radio" name="project-template" value={choice.id} checked={template === choice.id} onChange={() => setPicked(choice.id)} />
              <strong>{choice.name}</strong>
              <small>{choice.sentence} {choice.visibility === "public" ? "Public." : "Private."}</small>
            </label>
          ))}
        </fieldset>
      </details>
      <FolderStart />
    </form>
  </Dialog>;
}

function FolderStart() {
  const [copied, setCopied] = useState<"" | "yes" | "no">("");
  const commands = ["konae init --dir .", `konae login --dir . --url ${window.location.origin} --browser`, "konae publish --dir ."].join("\n");
  const copy = () => {
    void navigator.clipboard?.writeText(commands).then(() => setCopied("yes"), () => setCopied("no"));
    window.setTimeout(() => setCopied(""), 2500);
  };
  return <details className="ui-disclosure new-project-folder">
    <summary>Work from a folder instead</summary>
    <div className="new-project-folder-body">
    <p>Download the konae command from a GitHub release. In an empty folder on your computer, run these commands. Login opens the installation in your browser so no password goes in the terminal. After publish, the app shows on Projects.</p>
    <pre><code>{commands}</code></pre>
    <Button size="sm" onClick={copy}>{copied === "yes" ? <Check size={13} aria-hidden="true" /> : <Copy size={13} aria-hidden="true" />}{copied === "yes" ? "Copied" : "Copy commands"}</Button>
    </div>
    <span className="sr-only" role="status">{copied === "yes" ? "Commands copied" : copied === "no" ? "Could not copy. Select the commands instead." : ""}</span>
  </details>;
}

function DeleteProjectDialog({ project, published, onClose, onDelete, onDeleted, onExport, exporting = false }: {
  project: ProjectEntry;
  published: boolean;
  onClose: () => void;
  onDelete: (project: ProjectMetadata, name: string) => Promise<void>;
  onDeleted: (name: string) => void;
  onExport?: () => void;
  exporting?: boolean;
}) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const field = useRef<HTMLInputElement>(null);
  const close = () => { if (busy) return; onClose(); };
  const matches = typed === project.name;
  const submit = async () => {
    if (!matches) return;
    setBusy(true);
    setFailure("");
    try {
      await onDelete({ id: project.id, name: project.name, slug: "" }, typed);
      onDeleted(project.name);
      onClose();
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
      setBusy(false);
    }
  };
  const keep = published && onExport
    ? <Button onClick={onExport} disabled={busy || exporting} loading={exporting}><Download size={14} aria-hidden="true" /> Export first</Button>
    : null;
  return <Dialog
    open
    title={`Delete ${project.name}`}
    description={published
      ? "This app is published. Deleting it stops its address and deletes its database, app accounts, uploaded files and backups. This cannot be undone. Export the app folder first to keep its data."
      : "This removes the app and its files from your account. This cannot be undone."}
    initialFocus={field}
    onClose={close}
    footer={<>
      {keep && <span className="project-delete-keep">{keep}</span>}
      <Button onClick={close} disabled={busy}>Cancel</Button>
      <Button variant="danger" type="submit" form="delete-project" disabled={!matches} loading={busy}>Delete app</Button>
    </>}
  >
    <form id="delete-project" className="ui-dialog-body" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <Field label={`Type ${project.name} to confirm`}>
        <Input ref={field} value={typed} onChange={(event) => setTyped(event.target.value)} aria-invalid={failure ? true : undefined} maxLength={80} autoComplete="off" />
      </Field>
      {failure && <p className="project-delete-failure" role="alert">{failure}</p>}
    </form>
  </Dialog>;
}
