import { BarChart3, Box, LayoutGrid, Palette, UserRound } from "lucide-react";
import { ownerInitials, platformAccessHref, type OwnerSession } from "./owner-session";
import { Skeleton } from "./ui";
import "./owner-nav.css";

type Props = {
  active: "projects" | "design-system" | "usage";
  projectCount?: number;
  ownerSession: OwnerSession;
};

/**
 * The owner's way around: Projects and Usage, and the account at the foot.
 * A wide screen draws it as a rail on the left; a narrow one as a bar on top,
 * with the two links as tabs and the account behind its avatar.
 */
export function WorkspaceNav({ active, projectCount, ownerSession }: Props) {
  const accessHref = platformAccessHref(window.location.pathname);
  const initials = ownerInitials(ownerSession);
  // The component gallery is public. Signed-in owners reach it by address, not from the nav.
  const showDesignSystemLink = ownerSession.state !== "signed-in" && ownerSession.state !== "checking";
  const signedIn = ownerSession.state === "signed-in";
  const accountLabel = signedIn ? `Owner account, ${ownerSession.email ?? "signed in"}` : "Owner sign-in";

  return (
    <aside className="account-sidebar owner-nav">
      <a className="account-brand" href="/" aria-label="Konae, projects"><span aria-hidden="true"><Box size={17} /></span><strong>konae</strong></a>
      <nav aria-label="Main">
        <a href="/" className={active === "projects" ? "active" : undefined} aria-current={active === "projects" ? "page" : undefined}>
          <LayoutGrid size={16} aria-hidden="true" /> Projects {projectCount !== undefined && <span className="owner-nav-count">{projectCount}</span>}
        </a>
        <a href="/usage" className={active === "usage" ? "active" : undefined} aria-current={active === "usage" ? "page" : undefined}><BarChart3 size={16} aria-hidden="true" /> Usage</a>
        {showDesignSystemLink && <a className={active === "design-system" ? "active" : undefined} aria-current={active === "design-system" ? "page" : undefined} href="/design-system"><Palette size={16} aria-hidden="true" /> Design system</a>}
      </nav>
      {ownerSession.state === "checking"
        ? <div className="account-person is-checking" aria-hidden="true">
          <Skeleton width={32} height={32} radius="50%" />
          <div className="owner-nav-who"><Skeleton width="80%" height={12} /></div>
        </div>
        : <a className={`account-person is-${ownerSession.state}`} href={accessHref} aria-label={accountLabel} title={accountLabel}>
          <span className="owner-nav-avatar" aria-hidden="true">{initials ?? <UserRound size={15} />}</span>
          <div className="owner-nav-who">{signedIn ? ownerSession.email ?? "Owner account" : "Owner sign-in"}</div>
        </a>}
    </aside>
  );
}
