import { Box, Plus } from "lucide-react";
import { type ReactNode } from "react";
import { WorkspaceNav } from "./WorkspaceNav";
import type { OwnerSession } from "./owner-session";
import { Button, Card, LoadingBlock, Select, Skeleton } from "./ui";
import "./usage.css";

/**
 * What the owner sees from the first paint until the real screen is ready.
 * It draws the same chrome the finished screen draws, with the same boxes, so
 * the session check, the code chunk and the project load all happen inside one
 * stable frame. The page chrome never changes between stages. A skeleton fades
 * in only after the loading delay, counted from the page load.
 */
export type BootKind = "projects" | "usage" | "design-system" | "editor";

export function bootKindFor(pathname: string): BootKind {
  if (pathname === "/usage") return "usage";
  if (pathname === "/design-system") return "design-system";
  if (/^\/app\/[^/?#]+/.test(pathname)) return "editor";
  return "projects";
}

export function BootShell({ kind = bootKindFor(window.location.pathname), ownerSession, label = "Loading Littlebox" }: { kind?: BootKind; ownerSession: OwnerSession; label?: string }) {
  if (kind === "editor") return <EditorBoot label={label} />;
  return <div className="account-home">
    <WorkspaceNav active={kind} ownerSession={ownerSession} />
    <main className="account-main" aria-busy="true">
      {kind === "usage" ? <UsageBoot label={label} /> : kind === "design-system" ? <DesignSystemBoot label={label} /> : <ProjectsBoot label={label} />}
    </main>
  </div>;
}

function ProjectsBoot({ label }: { label: string }) {
  return <section className="account-content">
    <div className="account-hero">
      <Button variant="primary" className="new-project-button" disabled><Plus size={15} /> New project</Button>
      <h1>Your apps</h1>
      <p className="hero-note">Drafts open in the editor. Publish an app to give it a live address.</p>
    </div>
    <div className="project-list-heading">
      <div><p>All projects</p><Skeleton width={72} height={21} /></div>
      <button type="button" className="project-sort" disabled>Last updated ↓</button>
    </div>
    <LoadingBlock label={label} sinceNavigation className="dashboard-app-group">
      <Skeleton width={110} height={30} />
      <Skeleton width="60%" height={22} />
      <div className="project-grid">
        {[0, 1].map((index) => <Card key={index} className="project-card">
          <Skeleton height={170} radius={0} />
          <div className="project-card-body boot-card-body"><Skeleton width="55%" height={20} /><Skeleton width="80%" height={12} className="boot-gap" /></div>
        </Card>)}
      </div>
    </LoadingBlock>
  </section>;
}

function UsageBoot({ label }: { label: string }) {
  return <section className="account-content usage-main">
    <header className="account-hero usage-heading">
      <div><h1>Usage</h1><p>What your published apps did in the period. Drafts are listed without figures until you publish them.</p></div>
      <div className="usage-controls"><label>Period<Select value={30} disabled onChange={() => undefined}><option value={30}>Last 30 days</option></Select></label>
        <Button disabled>Refresh</Button></div>
    </header>
    <div className="usage-range"><Skeleton width={180} height={14} /><Skeleton width={150} height={14} /></div>
    <LoadingBlock label={label} sinceNavigation><UsageLoadingShapes /></LoadingBlock>
  </section>;
}

/** The Apps heading and a few table rows, shared by the boot and the first usage read. */
export function UsageLoadingShapes() {
  return <>
    <Skeleton width={60} height={22} />
    <Skeleton height={44} radius={0} />
    <Skeleton height={44} radius={0} />
    <Skeleton height={44} radius={0} />
  </>;
}

function DesignSystemBoot({ label }: { label: string }) {
  return <section className="account-content ds-workspace">
    <header className="account-hero ds-hero">
      <p className="account-kicker"><span>◆</span> Littlebox UI</p>
      <h1>Design system</h1>
      <p>Component examples. These controls do not change projects or upload files. Open a project to work with its files.</p>
    </header>
    <div className="ds-page-inner">
      <LoadingBlock label={label} sinceNavigation>
        <Skeleton width={140} height={22} />
        <Skeleton height={120} />
      </LoadingBlock>
    </div>
  </section>;
}

/** The editor's top bar and section bar, with the project name and the actions still to come. */
function EditorBoot({ label }: { label: string }): ReactNode {
  return <div className="workshop" aria-busy="true">
    <header className="topbar">
      <span className="brand">
        <span className="brand-mark"><Box size={17} strokeWidth={2.4} /></span>
        <span>littlebox</span>
      </span>
      <div className="project-switcher"><Skeleton width={107} height={17} /></div>
      <div className="top-actions"><Skeleton width={100} height={31} radius={0} /><Skeleton width={100} height={31} radius={0} /><Skeleton width={32} height={31} radius={0} /></div>
    </header>
    <nav className="section-nav" aria-hidden="true" />
    <div className="boot-editor-body">
      <LoadingBlock label={label} sinceNavigation>
        <Skeleton height={18} width="40%" />
        <Skeleton height={14} />
        <Skeleton height={14} width="70%" />
      </LoadingBlock>
    </div>
  </div>;
}
