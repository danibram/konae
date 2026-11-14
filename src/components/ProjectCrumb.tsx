import { ChevronDown, LayoutGrid } from "lucide-react";
import type { MouseEvent } from "react";
import { Menu } from "./ui";
import "./project-crumb.css";

/** `published` sets the Draft or Published word beside it in the switcher; left out, the item shows none. */
export type CrumbProject = { id: string; name: string; published?: boolean };

/** Moves the editor to a path the way the back button does, so the app's route follows it. */
export function openPath(path: string) {
  if (window.location.pathname === path) return;
  window.history.pushState({}, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

const plainClick = (event: MouseEvent) => event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;

/**
 * The way home from inside an app: "Projects / <app name ▾>". "Projects" is a
 * link to the list; the name opens a menu that starts with "All projects", so a
 * phone that hides the link still has a way home, then every project in the
 * order the list shows them, each with its state, and focus on the current one.
 * The editor and the page of an app edited in a folder both draw it.
 */
export function ProjectCrumb({ current, projects, onProjects, onOpen, className }: {
  current: CrumbProject;
  /** Every project of the owner, including ones edited in a folder. */
  projects: CrumbProject[];
  onProjects?: () => void;
  onOpen?: (projectId: string) => void;
  className?: string;
}) {
  const open = onOpen ?? ((projectId: string) => openPath(`/app/${encodeURIComponent(projectId)}`));
  const listed = projects.some((item) => item.id === current.id) ? projects : [current, ...projects];
  const home = () => { if (onProjects) onProjects(); else openPath("/"); };
  return <nav className={["project-crumb", className].filter(Boolean).join(" ")} aria-label="Breadcrumb">
    <a href="/" className="project-crumb-home" onClick={(event) => {
      if (!plainClick(event)) return;
      event.preventDefault();
      home();
    }}>Projects</a>
    <span className="project-crumb-sep" aria-hidden="true">/</span>
    <Menu
      label={`${current.name}, switch project`}
      showsLabel
      align="start"
      triggerClassName="project-crumb-current"
      trigger={<><span>{current.name}</span><ChevronDown size={14} aria-hidden="true" /></>}
      items={[
        { label: "All projects", icon: <LayoutGrid size={14} aria-hidden="true" />, onSelect: home },
        ...listed.map((item, index) => ({
          label: item.name,
          current: item.id === current.id,
          detail: item.published === undefined ? undefined : item.published ? "Published" : "Draft",
          separated: index === 0,
          onSelect: () => { if (item.id !== current.id) open(item.id); },
        })),
      ]}
    />
  </nav>;
}
