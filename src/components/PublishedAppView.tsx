import { releasedAt } from "../deployments/deployed-changes";
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { Activity, BarChart3, Box, Check, Copy, Database, UserRound } from "lucide-react";
import { HostedDatabaseClient, type DeploymentReceipt } from "../deployments/client";
import { McpClient } from "../mcp/client";
import type { ProjectMetadata } from "../lib/project-sources";
import type { OwnerSession } from "./owner-session";
import { AppUsersView } from "./AppUsersView";
import { AppStatus } from "./AppStatus";
import { DatabaseStudio } from "./DatabaseStudio";
import { ProjectCrumb, type CrumbProject } from "./ProjectCrumb";
import { pullCommand } from "./project-list";
import { Button, LoadingBlock } from "./ui";
import "./published-app.css";

const WhatHappenedView = lazy(() => import("./WhatHappenedView").then((module) => ({ default: module.WhatHappenedView })));

type Tab = "data" | "users" | "activity";
type Props = {
  app: ProjectMetadata;
  deployment: DeploymentReceipt;
  ownerSession: OwnerSession;
  projectCount: number;
  /** Every project of the owner, for the switcher. Read from the installation when not given. */
  projects?: CrumbProject[];
  onBack: () => void;
};

/**
 * A published app whose sources live in the folder it was published from.
 * The owner reads and changes its rows, manages its accounts and sees what it
 * did. Its files, its visibility and its actions change in that folder, with a
 * publish, so nothing here edits them.
 */
export function PublishedAppView({ app, deployment, ownerSession, projects, onBack }: Props) {
  const [tab, setTab] = useState<Tab>("data");
  const database = useMemo(() => new HostedDatabaseClient(deployment.id), [deployment.id]);
  const listed = useOwnerProjects(ownerSession, projects);
  const address = deployment.url.replace(/^https?:\/\//, "");
  useEffect(() => { document.title = `${app.name} · Konae`; }, [app.name]);
  const exportDatabase = async () => {
    const bytes = await database.exportDatabase();
    const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: "application/vnd.sqlite3" }));
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `${app.slug}.deployed.sqlite`;
    anchor.click();
    URL.revokeObjectURL(url);
  };
  const tabs: { id: Tab; label: string; icon: typeof Database }[] = [
    { id: "data", label: "Data", icon: Database },
    // The same name as the editor's tab for an app's accounts, so moving between two apps keeps one word.
    { id: "users", label: "Access", icon: UserRound },
    { id: "activity", label: "What happened", icon: Activity },
  ];
  return (
    <div className="account-home published-app">
      <main className="account-main">
        <header className="published-app-head">
          <h1 className="sr-only">{app.name}</h1>
          <div className="published-app-title">
            <a className="published-app-brand" href="/" aria-label="Konae, projects"><Box size={17} aria-hidden="true" /><span>konae</span></a>
            <ProjectCrumb current={{ id: app.id, name: app.name }} projects={listed} onProjects={onBack} />
            <AppStatus url={deployment.url} publishedAt={releasedAt(deployment)} />
          </div>
          <details className="published-app-folder ui-disclosure">
            <summary>Edited in a folder<span>In the app's folder</span></summary>
            <ul>
              <li><span>Publish a change</span><Command text="konae publish --dir ." /></li>
              <li><span>Copy its data to the folder</span><Command text={pullCommand} /></li>
            </ul>
          </details>
          <nav className="published-app-tabs" aria-label="Views of this app">
            {tabs.map(({ id, label, icon: Icon }) => <button key={id} type="button" className={tab === id ? "active" : ""} aria-current={tab === id || undefined} onClick={() => setTab(id)}><Icon size={14} aria-hidden="true" /> {label}</button>)}
            <a className="published-app-usage" href={`/usage?deployment=${encodeURIComponent(deployment.id)}`}><BarChart3 size={14} aria-hidden="true" /> Usage</a>
          </nav>
        </header>
        <section className="published-app-body" aria-label={tabs.find((item) => item.id === tab)?.label}>
          {tab === "data" && <DatabaseStudio host={database} onExportDatabase={() => void exportDatabase()} target="deployed" deployedLabel={address} />}
          {tab === "users" && <AppUsersView projectId={app.id} deploymentId={deployment.id} visibility={deployment.manifest.visibility} fileAccess={deployment.manifest.fileAccess} />}
          {tab === "activity" && <Suspense fallback={<LoadingBlock label="Loading this panel" rows={4} className="panel-loading" />}><WhatHappenedView deployment={{ id: deployment.id, url: deployment.url }} /></Suspense>}
        </section>
      </main>
    </div>
  );
}

/** One command to run in a terminal, with a button that copies it. */
function Command({ text }: { text: string }) {
  const [copied, setCopied] = useState<"" | "yes" | "no">("");
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(() => setCopied("yes"), () => setCopied("no"));
    window.setTimeout(() => setCopied(""), 2000);
  };
  return <span className="published-app-command">
    <code>{text}</code>
    <Button size="sm" onClick={copy} aria-label={`Copy ${text}`}>{copied === "yes" ? <Check size={13} aria-hidden="true" /> : <Copy size={13} aria-hidden="true" />}{copied === "yes" ? "Copied" : copied === "no" ? "Select it" : "Copy"}</Button>
  </span>;
}

/** The owner's projects for the switcher: the ones given, or the installation's list. */
function useOwnerProjects(ownerSession: OwnerSession, given?: CrumbProject[]): CrumbProject[] {
  const [read, setRead] = useState<CrumbProject[]>([]);
  const ownerId = ownerSession.state === "signed-in" ? ownerSession.ownerId : undefined;
  useEffect(() => {
    if (given || !ownerId) return;
    let active = true;
    void new McpClient("/mcp", { ownerId }).call<{ projects: { project: ProjectMetadata }[] }>("projects_list")
      .then((result) => { if (active) setRead(result.projects.map(({ project }) => ({ id: project.id, name: project.name }))); })
      .catch(() => { /* The switcher keeps the current app and the way to Projects. */ });
    return () => { active = false; };
  }, [given, ownerId]);
  return given ?? read;
}
