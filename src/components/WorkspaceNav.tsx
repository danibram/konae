import { BarChart3, Box, FileCode2, LayoutGrid, Palette, Sparkles, UserRound } from "lucide-react";
import { useState } from "react";
import { ownerInitials, platformAccessHref, type OwnerSession } from "./owner-session";
import { Dialog, Skeleton } from "./ui";

type Props = {
  active: "projects" | "design-system" | "usage";
  projectCount?: number;
  ownerSession: OwnerSession;
};

export function WorkspaceNav({ active, projectCount, ownerSession }: Props) {
  const accessHref = platformAccessHref(window.location.pathname);
  const initials = ownerInitials(ownerSession);
  const [moreOpen, setMoreOpen] = useState(false);

  return (
    <aside className="account-sidebar">
      <div className="account-brand"><span><Box size={17} /></span><strong>littlebox</strong></div>
      <nav>
        <a href="/" className={active === "projects" ? "active" : undefined} aria-current={active === "projects" ? "page" : undefined}>
          <LayoutGrid size={16} /> Projects {projectCount !== undefined && <span>{projectCount}</span>}
        </a>
        <a href="/usage" className={active === "usage" ? "active" : undefined} aria-current={active === "usage" ? "page" : undefined}><BarChart3 size={16} /> Usage</a>
        <a className="nav-wide" href="/architecture.html"><FileCode2 size={16} /> Architecture</a>
        <a className={active === "design-system" ? "active" : undefined} href="/design-system"><Palette size={16} /> Design system</a>
        <button type="button" className="nav-more-trigger" aria-haspopup="dialog" aria-expanded={moreOpen} onClick={() => setMoreOpen(true)}>
          More
        </button>
      </nav>
      <div className="account-sidebar-note">
        <Sparkles size={16} />
        <div><strong>Take it with you</strong><small>Open a project, then Export or Deploy.</small></div>
      </div>
      {ownerSession.state === "checking"
        ? <div className="account-person is-checking" aria-hidden="true">
          <Skeleton width={32} height={32} radius="50%" />
          <div><Skeleton width="70%" height={12} /><Skeleton width="85%" height={10} /></div>
        </div>
        : <a className={`account-person is-${ownerSession.state}`} href={accessHref}>
          <span aria-hidden="true">{initials ?? <UserRound size={15} />}</span>
          <div>
            <strong>{ownerSession.state === "signed-in" ? ownerSession.email ?? "Owner signed in" : "Owner sign-in"}</strong>
            {ownerSession.state === "signed-in" && <small>Can deploy from this browser</small>}
          </div>
        </a>}
      <Dialog
        open={moreOpen}
        title="More"
        description="Workspace links"
        className="nav-more-dialog"
        onClose={() => setMoreOpen(false)}
      >
        <nav className="nav-more-sheet" aria-label="More workspace links">
          <a href="/" aria-current={active === "projects" ? "page" : undefined} onClick={() => setMoreOpen(false)}>Projects</a>
          <a href="/architecture.html" onClick={() => setMoreOpen(false)}>Architecture</a>
          <a href="/usage" aria-current={active === "usage" ? "page" : undefined} onClick={() => setMoreOpen(false)}>Usage</a>
          <a href={accessHref} onClick={() => setMoreOpen(false)}>Owner sign-in</a>
          <a href="/design-system" aria-current={active === "design-system" ? "page" : undefined} onClick={() => setMoreOpen(false)}>Design system</a>
        </nav>
      </Dialog>
    </aside>
  );
}
