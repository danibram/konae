import { ChevronDown, LayoutGrid } from "lucide-react";
import type { MouseEvent } from "react";
import { Menu } from "./ui";
import "./project-crumb.css";

export type CrumbProject = { id: string; name: string; published?: boolean };

export function openPath(path: string) {
  if (window.location.pathname === path) return;
  window.history.pushState({}, "", path);
  window.dispatchEvent(new PopStateEvent("popstate"));
}

const plainClick = (event: MouseEvent) => event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;

export function ProjectCrumb({ current, projects, onProjects, onOpen, className }: {
  current: CrumbProject;
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
