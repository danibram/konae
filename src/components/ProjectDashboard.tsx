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

type Props = {
  projects: MiniAppProject[];
  deployments: DeploymentIndex;
  ownerSession: OwnerSession;
  external: ProjectMetadata[];
  missing: ProjectMetadata[];
  drafts: MiniAppProject[];
  onImportDraft: (index: number) => void;
  onOpenProject: (projectId: string, entry?: "preview" | "agent" | "code") => void;
  onCreateProject: (name: string, template: "tasks" | "notes" | "scratch") => void;
};

export function ProjectDashboard({ projects, deployments, ownerSession, external, missing, drafts, onImportDraft, onOpenProject, onCreateProject }: Props) {
  const [newestFirst, setNewestFirst] = useState(true);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [nameTouched, setNameTouched] = useState(false);
  const [template, setTemplate] = useState<"tasks" | "notes">("tasks");
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
      <WorkspaceNav active="projects" projectCount={projects.length} ownerSession={ownerSession} />

      <main className="account-main">
        <section className="account-content">
          <div className="account-hero">
            <Button variant="primary" className="new-project-button" onClick={() => setCreating(true)}><Plus size={15} /> New project</Button>
            <h1>Describe an app, or edit its files. Both paths pass the same checks, and the app can leave with you.</h1>
            <p className="hero-note">Owners deploy and keep the data. People who use an app sign in inside that app.</p>
          </div>
          {external.length > 0 && <section><h2>External projects</h2><p>These projects keep their sources on their owner's computer.</p><ul>{external.map((item) => <li key={item.id}>{item.name}</li>)}</ul></section>}
          {missing.length > 0 && <section><h2>Projects without saved sources</h2><p>These projects have metadata on the server, but no saved editor sources.</p><ul>{missing.map((item) => <li key={item.id}>{item.name}</li>)}</ul></section>}
          {drafts.length > 0 && <details><summary>Import drafts from this browser</summary><p>Choose a draft to restore missing sources or create a new project in this account.</p><ul>{drafts.map((draft, index) => <li key={`${draft.id}-${index}`}>{draft.name} <Button onClick={() => onImportDraft(index)}>{missing.some((item) => item.id === draft.id) ? "Restore sources" : "Import as new"}</Button></li>)}</ul></details>}

          <div className="project-list-heading">
            <div><p>All projects</p><strong>{filtered.length} project{filtered.length === 1 ? "" : "s"}</strong></div>
            <button
              className="project-sort"
              aria-label={`Sort by last updated, ${newestFirst ? "newest" : "oldest"} first`}
              onClick={() => setNewestFirst((current) => !current)}
            >
              Last updated {newestFirst ? "↓" : "↑"}
            </button>
          </div>

          <div className="project-grid">
            {filtered.map((project, index) => (
              <ProjectCard
                key={project.id}
                project={project}
                deployment={deployments[project.id]}
                index={index}
                onOpen={(entry) => onOpenProject(project.id, entry)}
              />
            ))}
            {filtered.length === 0 && <div className="projects-empty"><strong>No projects yet</strong><p>Create one and describe what it should do.</p></div>}
          </div>
          <p className="take-it">Take it with you. Open a project, then Export or Deploy.</p>
        </section>
      </main>
      <Dialog
        open={creating}
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
            <p className="start-note">Describe is next. A shared list is the start for anything else.</p>
            <label className={template === "tasks" ? "selected" : ""}>
              <input type="radio" name="project-template" value="tasks" checked={template === "tasks"} onChange={() => setTemplate("tasks")} />
              <strong>Shared list</strong>
              <small>Anyone with the link can add a row.</small>
            </label>
            <label className={template === "notes" ? "selected" : ""}>
              <input type="radio" name="project-template" value="notes" checked={template === "notes"} onChange={() => setTemplate("notes")} />
              <strong>Private notes</strong>
              <small>Each person signs in and only sees their own.</small>
            </label>
          </fieldset>
        </form>
      </Dialog>
    </div>
  );
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
        </div>
        <div className="project-meta"><span><Clock3 size={11} /> {relativeTime(project.updatedAt)}</span></div>
        <footer>
          <Button variant="secondary" size="sm" onClick={() => onOpen("preview")}>Open</Button>
          <button type="button" className="project-door-link" onClick={() => onOpen("agent")}>Describe</button>
          <button type="button" className="project-door-link edit-files" onClick={() => onOpen("code")}>Edit files</button>
          {deployment && <a href={deployment.url} target="_blank" rel="noopener"><ExternalLink size={13} /> Open app</a>}
        </footer>
      </div>
    </Card>
  );
}

function posterLines(project: MiniAppProject): string[] {
  const html = project.files["index.html"] ?? "";
  if (project.id === "app_littlebox_architecture" || html.includes("data-map=\"leaves\"")) {
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
