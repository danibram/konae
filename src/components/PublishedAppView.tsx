import { lazy, Suspense, useMemo, useState } from "react";
import { Activity, ArrowLeft, BarChart3, Database, ExternalLink, Users } from "lucide-react";
import { HostedDatabaseClient, type DeploymentReceipt } from "../deployments/client";
import type { ProjectMetadata } from "../lib/project-sources";
import type { OwnerSession } from "./owner-session";
import { AppUsersView } from "./AppUsersView";
import { DatabaseStudio } from "./DatabaseStudio";
import { WorkspaceNav } from "./WorkspaceNav";
import { Badge, LoadingBlock } from "./ui";
import "./published-app.css";

const WhatHappenedView = lazy(() => import("./WhatHappenedView").then((module) => ({ default: module.WhatHappenedView })));

type Tab = "data" | "users" | "activity";
type Props = {
  app: ProjectMetadata;
  deployment: DeploymentReceipt;
  ownerSession: OwnerSession;
  projectCount: number;
  onBack: () => void;
};

/**
 * A published app whose sources live in the folder it was published from.
 * The owner reads and changes its rows, manages its accounts and sees what it
 * did. Its files, its visibility and its actions change in that folder, with a
 * publish, so nothing here edits them.
 */
export function PublishedAppView({ app, deployment, ownerSession, projectCount, onBack }: Props) {
  const [tab, setTab] = useState<Tab>("data");
  const database = useMemo(() => new HostedDatabaseClient(deployment.id), [deployment.id]);
  const address = deployment.url.replace(/^https?:\/\//, "");
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
    { id: "users", label: "Users", icon: Users },
    { id: "activity", label: "What happened", icon: Activity },
  ];
  return (
    <div className="account-home published-app">
      <WorkspaceNav active="projects" projectCount={projectCount} ownerSession={ownerSession} />
      <main className="account-main">
        <header className="published-app-head">
          <button type="button" className="published-app-back" onClick={onBack}><ArrowLeft size={14} /> Projects</button>
          <div className="published-app-title">
            <h1>{app.name}</h1>
            <Badge tone={deployment.manifest.visibility === "public" ? "success" : "accent"}>{deployment.manifest.visibility}</Badge>
            <Badge tone="neutral">No editor files</Badge>
          </div>
          <p>Its files live in the folder it was published from, so its pages, actions and visibility change there, with a publish. Here you work on what the published app holds.</p>
          <nav className="published-app-tabs" aria-label="Published app views">
            {tabs.map(({ id, label, icon: Icon }) => <button key={id} type="button" className={tab === id ? "active" : ""} aria-current={tab === id || undefined} onClick={() => setTab(id)}><Icon size={13} /> {label}</button>)}
            <a href={`/usage?deployment=${encodeURIComponent(deployment.id)}`}><BarChart3 size={13} /> Usage</a>
            <a href={deployment.url} target="_blank" rel="noopener"><ExternalLink size={13} /> {address}</a>
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
