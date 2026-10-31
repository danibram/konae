import {
  ArrowRight,
  Clock3,
  ExternalLink,
  Globe2,
  LockKeyhole,
  Plus,
} from "lucide-react";
import { useMemo, useState } from "react";
import { Badge, Button, Card, Dialog, Field, Input } from "./ui";
import type { DeploymentReceipt } from "../deployments/client";
import type { DeploymentIndex } from "../lib/workspace";
import type { MiniAppProject } from "../lib/types";
import type { OwnerSession } from "./owner-session";
import type { ProjectMetadata } from "../lib/managed-sources";
import { WorkspaceNav } from "./WorkspaceNav";
import { templateChoices, type TemplateId } from "../lib/templates";

type Props = {
  projects: MiniAppProject[];
  deployments: DeploymentIndex;
  ownerSession: OwnerSession;
  external: ProjectMetadata[];
  missing: ProjectMetadata[];
  notice?: string;
  drafts: MiniAppProject[];
  onImportDraft: (index: number) => void;
  onDismissDraft: (index: number) => void;
  onDeleteMissing: (project: ProjectMetadata, name: string) => Promise<void>;
  onOpenProject: (projectId: string, entry?: "preview" | "agent" | "code") => void;
  onCreateProject: (name: string, template: TemplateId | "scratch") => void;
};

export function ProjectDashboard({ projects, deployments, ownerSession, external, missing, notice, drafts, onImportDraft, onDismissDraft, onDeleteMissing, onOpenProject, onCreateProject }: Props) {
  const interrupted = missing.filter((item) => !projects.some((project) => project.id === item.id));
  const projectCount = projects.length + interrupted.length + external.length;
  const [deleting, setDeleting] = useState<ProjectMetadata | null>(null);
  const [newestFirst, setNewestFirst] = useState(true);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [template, setTemplate] = useState<TemplateId>("tasks");
  const filtered = useMemo(
    () => [...projects].sort((a, b) => newestFirst ? b.updatedAt - a.updatedAt : a.updatedAt - b.updatedAt),
    [projects, newestFirst],
  );
  const nameMissing = !name.trim();
  const nameError = nameTouched && nameMissing ? "A project name is required." : undefined;
  const create = () => {
    setNameTouched(true);
    if (nameMissing) return;
    onCreateProject(name.trim(), template);
    setName("");
    setNameTouched(false);
    setCreating(false);
  };

  return (
    <div className="account-home">
      <WorkspaceNav active="projects" projectCount={projectCount} ownerSession={ownerSession} />

      <main className="account-main">
        <section className="account-content">
          <div className="account-hero">
            <Button variant="primary" className="new-project-button" onClick={() => setCreating(true)}><Plus size={15} /> New project</Button>
            <h1>Your apps</h1>
            <p className="hero-note">Drafts open in the editor. Publish an app to give it a live address.</p>
            {notice && <p className="hero-note" role="status">{notice}</p>}
          </div>
          {external.length > 0 && <section className="dashboard-app-group" aria-label="External projects">
            <h2>External projects <small>{external.length}</small></h2>
            <p>These projects keep their sources on their owner's computer. Publish them with the Konae command line.</p>
            <div className="project-grid">
              {external.map((item) => {
                const deployment = deployments[item.id];
                return <Card key={item.id} className="project-card">
                  <div className="project-card-body">
                    <div className="project-card-title"><h2>{item.name}</h2><Badge>External</Badge></div>
                    <p>{deployment ? "Published. Its sources are edited outside Konae." : "Not published yet."}</p>
                    {deployment && <footer>
                      <a href={deployment.url} target="_blank" rel="noopener"><ExternalLink size={13} /> Open published app</a>
                      <a href={`/usage?deployment=${encodeURIComponent(deployment.id)}`}>Usage</a>
                    </footer>}
                  </div>
                </Card>;
              })}
            </div>
          </section>}
          {drafts.some((draft) => !missing.some((item) => item.id === draft.id)) && <details className="local-copies"><summary>Copies saved in this browser</summary><p>Earlier versions of Konae kept apps in the browser. These copies are not in your account. Import one to edit it here, or remove it from this browser.</p><ul>{drafts.map((draft, index) => !missing.some((item) => item.id === draft.id) && <li key={`${draft.id}-${index}`}><span>{draft.name}</span> <Button size="sm" onClick={() => onImportDraft(index)}>Import as new</Button> <Button size="sm" variant="ghost" onClick={() => onDismissDraft(index)}>Remove from this browser</Button></li>)}</ul></details>}

          <div className="project-list-heading">
            <div><p>All projects</p><strong>{projectCount} project{projectCount === 1 ? "" : "s"}</strong>{interrupted.length > 0 && <p>{projects.length} editable · {interrupted.length} without editor files{external.length > 0 ? ` · ${external.length} external` : ""}</p>}</div>
            <button
              className="project-sort"
              aria-label={`Sort by last updated, ${newestFirst ? "newest" : "oldest"} first`}
              onClick={() => setNewestFirst((current) => !current)}
            >
              Last updated {newestFirst ? "↓" : "↑"}
            </button>
          </div>

          {([true, false] as const).map((published) => {
            const group = filtered.filter((project) => Boolean(deployments[project.id]) === published);
            if (!group.length) return null;
            return <section className="dashboard-app-group" aria-label={published ? "Published apps" : "Draft apps"} key={String(published)}>
              <h2>{published ? "Published" : "Drafts"} <small>{group.length}</small></h2>
              {!published && <p>These apps have no published address yet. Open the editor, then Deploy when ready.</p>}
              <div className="project-grid">
            {group.map((project, index) => (
              <ProjectCard
                key={project.id}
                project={project}
                deployment={deployments[project.id]}
                index={index}
                onOpen={(entry) => onOpenProject(project.id, entry)}
              />
            ))}
              </div>
            </section>;
          })}
          {interrupted.length > 0 && <section className="dashboard-app-group dashboard-recovery" aria-label="Apps without editor files">
            <h2>Apps without editor files <small>{interrupted.length}</small></h2>
            <p>Your account has these apps, but the files the editor needs were never saved with them, so they cannot open here. A published app keeps running and keeps its data. An older version of Konae could leave an app like this when a save was cut off.</p>
            <div className="project-grid">
            {interrupted.map((item) => {
              const draftIndex = drafts.findIndex((draft) => draft.id === item.id);
              const deployment = deployments[item.id];
              return <Card key={item.id} className="project-card">
                <div className="project-card-body">
                  <div className="project-card-title"><h2>{item.name}</h2><Badge tone="warning">No editor files</Badge></div>
                  {draftIndex >= 0
                    ? <p>This browser holds a copy of its files. Restore it to edit the app here.</p>
                    : <p>This browser holds no copy. If you edited it in another browser, open Konae there. If you do not need it, delete it.</p>}
                  <footer>
                    {draftIndex >= 0 && <Button size="sm" onClick={() => onImportDraft(draftIndex)}>Restore editor files</Button>}
                    {deployment && <a href={deployment.url} target="_blank" rel="noopener"><ExternalLink size={13} /> Open published app</a>}
                    <Button size="sm" variant="ghost" onClick={() => setDeleting(item)}>Delete app…</Button>
                  </footer>
                </div>
              </Card>;
            })}
            </div>
          </section>}
          {projectCount === 0 && <div className="projects-empty"><strong>No projects yet</strong><p>Create one and describe what it should do.</p></div>}
          <section className="take-it-group" aria-labelledby="take-it-heading">
            <h2 id="take-it-heading">Take it with you</h2>
            <p>Open a project, then Export or Deploy.</p>
          </section>
        </section>
      </main>
      <DeleteProjectDialog project={deleting} published={Boolean(deleting && deployments[deleting.id])} onClose={() => setDeleting(null)} onDelete={onDeleteMissing} />
      <Dialog
        open={creating}
        className="new-project"
        title="Name this app"
        description="Pick a name, then how it starts."
        onClose={() => { setCreating(false); setNameTouched(false); }}
        footer={<>
          <Button onClick={() => { setCreating(false); setNameTouched(false); }}>Cancel</Button>
          <Button variant="primary" type="submit" form="new-project">Create <ArrowRight size={14} /></Button>
        </>}
      >
        <form id="new-project" className="new-project-dialog" onSubmit={(event) => { event.preventDefault(); create(); }}>
          <Field
            label="Project name"
            help={nameError ?? "Choose a name you can recognize later."}
          >
            <Input
              autoFocus
              value={name}
              aria-invalid={nameError ? true : undefined}
              onChange={(event) => { setName(event.target.value); if (nameTouched) setNameTouched(true); }}
              placeholder=""
              maxLength={80}
            />
          </Field>
          <fieldset className="start-cards">
            <legend>Start with</legend>
            <p className="start-note">Each one runs as it is. Describe what to change next.</p>
            {templateChoices.map((choice) => (
              <label key={choice.id} className={template === choice.id ? "selected" : ""}>
                <input type="radio" name="project-template" value={choice.id} checked={template === choice.id} onChange={() => setTemplate(choice.id)} />
                <strong>{choice.name}</strong>
                <small>{choice.sentence}</small>
              </label>
            ))}
          </fieldset>
        </form>
      </Dialog>
    </div>
  );
}

function DeleteProjectDialog({ project, published, onClose, onDelete }: {
  project: ProjectMetadata | null;
  published: boolean;
  onClose: () => void;
  onDelete: (project: ProjectMetadata, name: string) => Promise<void>;
}) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  if (!project) return null;
  const close = () => { if (busy) return; setTyped(""); setFailure(""); onClose(); };
  const matches = typed === project.name;
  const submit = async () => {
    if (!matches) return;
    setBusy(true);
    setFailure("");
    try {
      await onDelete(project, typed);
      setTyped("");
      onClose();
    } catch (error) {
      setFailure(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };
  return <Dialog
    open
    title={`Delete ${project.name}`}
    description={published
      ? "This app is published. Deleting it stops its address, and deletes its database, app accounts, uploaded files and backups on this Kernel. This cannot be undone. Export a package first if you want to keep the data."
      : "This removes the app from your account. This cannot be undone."}
    onClose={close}
    footer={<>
      <Button onClick={close} disabled={busy}>Cancel</Button>
      <Button variant="danger" type="submit" form="delete-project" disabled={!matches} loading={busy}>Delete app</Button>
    </>}
  >
    <form id="delete-project" className="ui-dialog-body" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
      <Field label={`Type ${project.name} to confirm`}>
        <Input autoFocus value={typed} onChange={(event) => setTyped(event.target.value)} aria-invalid={failure ? true : undefined} maxLength={80} />
      </Field>
      {failure && <p role="alert">{failure}</p>}
    </form>
  </Dialog>;
}

function ProjectCard({ project, deployment, index, onOpen }: {
  project: MiniAppProject;
  deployment?: DeploymentReceipt;
  index: number;
  onOpen: (entry: "preview" | "agent" | "code") => void;
}) {
  const visibility = deployment?.manifest.visibility;
  return (
    <Card className={`project-card project-tone-${index % 4}`}>
      <button className="project-card-preview" onClick={() => onOpen("preview")} aria-label={`Open ${project.name}`}>
        <div className="project-poster">{posterLines(project).map((line) => <span className="poster-line" key={line}>{line}</span>)}</div>
      </button>
      <div className="project-card-body">
        <div className="project-card-title">
          <div>
            <h2>{project.name}{project.id === "app_tiny_tasks" && <Badge className="starter-badge">Starter</Badge>}</h2>
            {pageSummary(project) && <p>{pageSummary(project)}</p>}
          </div>
          {visibility && <Badge tone={visibility === "public" ? "success" : "accent"} className={`deployment-badge ${visibility}`}>{visibility === "public" ? <Globe2 size={11} /> : <LockKeyhole size={11} />}{visibility}</Badge>}
          {!deployment && <Badge tone="neutral">Draft · Not published</Badge>}
        </div>
        <div className="project-meta"><span><Clock3 size={11} /> {relativeTime(project.updatedAt)}</span></div>
        <footer>
          <Button variant="secondary" size="sm" onClick={() => onOpen("preview")}>Open editor</Button>
          <button type="button" className="project-door-link" onClick={() => onOpen("agent")}>Describe</button>
          <button type="button" className="project-door-link edit-files" onClick={() => onOpen("code")}>Edit files</button>
          {deployment && <a href={`/usage?deployment=${encodeURIComponent(deployment.id)}`}>Usage</a>}
          {deployment && <a href={deployment.url} target="_blank" rel="noopener"><ExternalLink size={13} /> Open published app</a>}
        </footer>
      </div>
    </Card>
  );
}

function posterLines(project: MiniAppProject): string[] {
  const html = project.files["index.html"] ?? "";
  if (project.id === "app_konae_architecture" || html.includes("data-map=\"leaves\"")) {
    return ["Export and Deploy", "Accounts, rows, and rules leave together."];
  }
  if (html.includes("Add one small thing")) return ["Add one small thing…", "The list stops at 100 rows."];
  const summary = pageSummary(project);
  return summary ? [summary] : ["A small app."];
}

function pageSummary(project: MiniAppProject): string {
  const purpose = project.files["index.html"]?.match(/data-purpose="([^"]+)"/);
  return purpose?.[1] ?? "";
}

function relativeTime(timestamp: number) {
  return new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric" }).format(timestamp);
}
