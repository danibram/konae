import {
  ArrowRight,
  Box,
  Clock3,
  Code2,
  ExternalLink,
  FileCode2,
  Globe2,
  LayoutGrid,
  LockKeyhole,
  Plus,
  Search,
  Settings2,
  Sparkles,
} from "lucide-react";
import { useMemo, useState } from "react";
import type { DeploymentReceipt } from "../deployments/client";
import { architectureProjectId } from "../lib/architecture";
import type { DeploymentIndex } from "../lib/workspace";
import type { MiniAppProject } from "../lib/types";

type Props = {
  projects: MiniAppProject[];
  deployments: DeploymentIndex;
  onOpenProject: (projectId: string) => void;
  onCreateProject: (name: string, template: "tasks" | "notes") => void;
};

export function ProjectDashboard({ projects, deployments, onOpenProject, onCreateProject }: Props) {
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [template, setTemplate] = useState<"tasks" | "notes">("tasks");
  const filtered = useMemo(() => {
    const search = query.trim().toLowerCase();
    return search ? projects.filter((project) => `${project.name} ${project.slug}`.toLowerCase().includes(search)) : projects;
  }, [projects, query]);
  const published = projects.filter((project) => deployments[project.id]).length;

  const create = () => {
    onCreateProject(name, template);
    setName("");
    setCreating(false);
  };

  return (
    <div className="account-home">
      <aside className="account-sidebar">
        <div className="account-brand"><span><Box size={17} /></span><strong>littlebox</strong><em>ALPHA</em></div>
        <nav>
          <button className="active"><LayoutGrid size={16} /> Projects <span>{projects.length}</span></button>
          <button onClick={() => onOpenProject(architectureProjectId)}><FileCode2 size={16} /> Architecture</button>
        </nav>
        <div className="account-sidebar-note">
          <Sparkles size={16} />
          <div><strong>Kernel local</strong><small>Identity and storage online</small></div>
          <i />
        </div>
        <div className="account-person">
          <span>DR</span>
          <div><strong>Local owner</strong><small>Personal workspace</small></div>
          <Settings2 size={15} />
        </div>
      </aside>

      <main className="account-main">
        <header className="account-topbar">
          <div><p>WORKSPACE / PERSONAL</p><strong>Your projects</strong></div>
          <label className="project-search"><Search size={15} /><input aria-label="Search projects" value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search projects…" /></label>
          <button className="new-project-button" onClick={() => setCreating(true)}><Plus size={15} /> New project</button>
        </header>

        <section className="account-content">
          <div className="account-hero">
            <div>
              <p className="account-kicker"><span>01</span> YOUR LITTLE SOFTWARE SHELF</p>
              <h1>Small apps.<br /><em>All yours.</em></h1>
              <p>Open a project to change its code and data, or jump straight into its latest published Deployment.</p>
            </div>
            <div className="account-stats">
              <div><strong>{String(projects.length).padStart(2, "0")}</strong><span>PROJECTS</span></div>
              <div><strong>{String(published).padStart(2, "0")}</strong><span>PUBLISHED</span></div>
              <div><strong>{String(projects.length - published).padStart(2, "0")}</strong><span>DRAFTS</span></div>
            </div>
          </div>

          {creating && (
            <form className="new-project-form" onSubmit={(event) => { event.preventDefault(); create(); }}>
              <div><p>NEW / PROJECT</p><strong>What are you building?</strong></div>
              <input autoFocus aria-label="Project name" value={name} onChange={(event) => setName(event.target.value)} placeholder="Weekend planner" maxLength={80} />
              <select aria-label="Project template" value={template} onChange={(event) => setTemplate(event.target.value as "tasks" | "notes")}><option value="tasks">Shared tasks</option><option value="notes">Private notes with login</option></select>
              <button type="button" onClick={() => setCreating(false)}>Cancel</button>
              <button type="submit">Create <ArrowRight size={14} /></button>
            </form>
          )}

          <div className="project-list-heading">
            <div><p>PROJECT INDEX</p><strong>{filtered.length} item{filtered.length === 1 ? "" : "s"}</strong></div>
            <span>LAST UPDATED ↓</span>
          </div>

          <div className="project-grid">
            {filtered.map((project, index) => (
              <ProjectCard
                key={project.id}
                project={project}
                deployment={deployments[project.id]}
                index={index}
                onOpen={() => onOpenProject(project.id)}
              />
            ))}
            {filtered.length === 0 && <div className="projects-empty"><Search size={23} /><strong>No projects found</strong><p>Try another name or create a new mini-app.</p></div>}
          </div>
        </section>
      </main>
    </div>
  );
}

function ProjectCard({ project, deployment, index, onOpen }: {
  project: MiniAppProject;
  deployment?: DeploymentReceipt;
  index: number;
  onOpen: () => void;
}) {
  const visibility = deployment?.manifest.visibility;
  return (
    <article className={`project-card project-tone-${index % 4}`}>
      <button className="project-card-preview" onClick={onOpen} aria-label={`Edit ${project.name}`}>
        <div className="project-window-bar"><i /><i /><i /><code>{project.slug}.local</code></div>
        <div className="project-poster">
          <span>{String(index + 1).padStart(2, "0")}</span>
          <strong>{project.name}</strong>
          <small>{project.files["schema.sql"].match(/CREATE TABLE/gi)?.length ?? 0} TABLE · {project.manifest.runtime.database.actions.length} ACTIONS</small>
        </div>
        <span className="project-edit-cue">EDIT PROJECT <ArrowRight size={13} /></span>
      </button>
      <div className="project-card-body">
        <div className="project-card-title">
          <div><p>{project.slug}</p><h2>{project.name}</h2></div>
          {visibility ? <span className={`deployment-badge ${visibility}`}>{visibility === "public" ? <Globe2 size={11} /> : <LockKeyhole size={11} />}{visibility}</span> : <span className="deployment-badge draft"><Code2 size={11} />draft</span>}
        </div>
        <div className="project-meta"><span><Clock3 size={11} /> {relativeTime(project.updatedAt)}</span><span>{Object.keys(project.files).length} source files</span></div>
        <footer>
          <button onClick={onOpen}><Code2 size={13} /> Open editor</button>
          {deployment ? <a href={deployment.url} target="_blank" rel="noopener"><ExternalLink size={13} /> Open app</a> : <span>Not published yet</span>}
        </footer>
      </div>
    </article>
  );
}

function relativeTime(timestamp: number) {
  const elapsed = Math.max(0, Date.now() - timestamp);
  if (elapsed < 60_000) return "just now";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}m ago`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}h ago`;
  return new Intl.DateTimeFormat("en", { month: "short", day: "numeric" }).format(timestamp);
}
