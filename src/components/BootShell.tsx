import { Box } from "lucide-react";
import { type ReactNode } from "react";
import { WorkspaceNav } from "./WorkspaceNav";
import type { OwnerSession } from "./owner-session";
import { Button, LoadingBlock, Select, Skeleton } from "./ui";
import "./usage.css";
import "./project-list.css";

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

export function BootShell({ kind = bootKindFor(window.location.pathname), ownerSession, label = "Loading Konae" }: { kind?: BootKind; ownerSession: OwnerSession; label?: string }) {
  if (kind === "editor") return <EditorBoot label={label} />;
  return <div className="account-home">
    <WorkspaceNav active={kind} ownerSession={ownerSession} />
    <main className="account-main" aria-busy="true">
      {kind === "usage" ? <UsageBoot label={label} /> : kind === "design-system" ? <DesignSystemBoot label={label} /> : <ProjectsBoot label={label} signedIn={ownerSession.state === "signed-in"} />}
    </main>
  </div>;
}

function ProjectsBoot({ label, signedIn }: { label: string; signedIn: boolean }) {
  // Until the session is known nothing is drawn but the frame: a signed-out visit leaves for sign-in without a flash of the list.
  return <section className="account-content project-list">
    {signedIn && <>
      <header className="project-list-head"><h1>Projects</h1><Skeleton width={128} height={36} /></header>
      <LoadingBlock label={label} sinceNavigation className="project-list-boot">
        <div className="project-list-tools"><Skeleton width={320} height={40} /><Skeleton width={180} height={40} /></div>
        <Skeleton width={90} height={18} className="boot-gap" />
        <ul className="project-cards">
          {[0, 1, 2].map((index) => <li key={index} className="project-card-item">
            <Skeleton width="60%" height={20} />
            <Skeleton width="90%" height={14} className="boot-gap" />
            <Skeleton width="45%" height={14} className="boot-gap" />
          </li>)}
        </ul>
      </LoadingBlock>
    </>}
  </section>;
}

function UsageBoot({ label }: { label: string }) {
  return <section className="account-content usage-main">
    <header className="usage-heading">
      <h1>Usage</h1>
      <div className="usage-controls"><label className="usage-period">Period<Select value={30} disabled onChange={() => undefined}><option value={30}>Last 30 days</option></Select></label>
        <Button disabled>Refresh</Button></div>
    </header>
    <p className="usage-range"><Skeleton width={360} height={14} /></p>
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
      <p className="account-kicker"><span>◆</span> Konae UI</p>
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
        <span>konae</span>
      </span>
      <div className="project-switcher"><Skeleton width={107} height={17} /></div>
      <div className="top-actions"><Skeleton width={100} height={31} radius={0} className="boot-wide" /><Skeleton width={100} height={31} radius={0} className="boot-wide" /><Skeleton width={32} height={31} radius={0} /></div>
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
