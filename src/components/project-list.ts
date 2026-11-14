import { releasedAt } from "../deployments/deployed-changes";
import type { DeploymentReceipt } from "../deployments/client";
import type { ProjectMetadata } from "../lib/project-sources";
import type { MiniAppProject } from "../lib/types";

/** What a person runs in an app's folder to copy its published data there, in the form the owner account page uses. */
export const pullCommand = "konae pull --dir .";

/** One row of the project list, whether the editor holds its files or a folder does. */
export type ProjectEntry = {
  id: string;
  name: string;
  /** The page's own one-line purpose, when the editor holds its files. */
  purpose: string;
  /** When its files last changed in the editor. A folder app has none here. */
  editedAt?: number;
  deployment?: DeploymentReceipt;
  /** Its files live in the folder it was published from. */
  inFolder: boolean;
};

export type ProjectSort = "edited" | "name";

const entities: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'", apos: "'" };

/** The page's declared purpose, as text: `data-purpose` holds it escaped, the way an attribute must. */
export function projectPurpose(project: MiniAppProject): string {
  const raw = project.files["index.html"]?.match(/data-purpose="([^"]+)"/)?.[1] ?? "";
  return raw.replace(/&(amp|lt|gt|quot|#39|apos);/g, (_, name: string) => entities[name]);
}

export function projectEntries(projects: readonly MiniAppProject[], folders: readonly ProjectMetadata[], deployments: Readonly<Record<string, DeploymentReceipt>>): ProjectEntry[] {
  return [
    ...projects.map((project) => ({ id: project.id, name: project.name, purpose: projectPurpose(project), editedAt: project.updatedAt, deployment: deployments[project.id], inFolder: false })),
    ...folders.filter((item) => !projects.some((project) => project.id === item.id))
      .map((item) => ({ id: item.id, name: item.name, purpose: "", deployment: deployments[item.id], inFolder: true })),
  ];
}

/** The moment the list sorts by: the last edit, or for a folder app its last publish. */
const touchedAt = (entry: ProjectEntry) => entry.editedAt ?? (entry.deployment ? releasedAt(entry.deployment) : 0);

/**
 * The list as a person reads it: published apps first, then drafts, whatever
 * edits their files; inside each group the search narrows and the sort orders.
 */
export function groupProjects(entries: readonly ProjectEntry[], query: string, sort: ProjectSort) {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const matches = (entry: ProjectEntry) => {
    const text = `${entry.name} ${entry.purpose} ${entry.deployment?.url ?? ""}`.toLowerCase();
    return words.every((word) => text.includes(word));
  };
  const order = (a: ProjectEntry, b: ProjectEntry) => sort === "name"
    ? a.name.localeCompare(b.name, "en", { sensitivity: "base" })
    : touchedAt(b) - touchedAt(a);
  const shown = entries.filter(matches).sort(order);
  return { published: shown.filter((entry) => entry.deployment), drafts: shown.filter((entry) => !entry.deployment) };
}

/** Words a name does not end on: "Chore rota for our" reads as cut off, "Chore rota" does not. */
const trailing = new Set(["a", "an", "and", "at", "by", "for", "from", "in", "into", "my", "of", "on", "or", "our", "the", "their", "to", "where", "which", "who", "with", "your"]);

/** A name for a new project taken from what it should do: its first few words, capitalised, never ending on a small word. */
export function nameFromDescription(description: string): string {
  const first = description.trim().split(/[.!?\n,;:]/)[0]?.trim() ?? "";
  const words = first.replace(/["“”«»]/g, "").replace(/^(an?|the)\s+/i, "").split(/\s+/).filter(Boolean).slice(0, 4);
  while (words.length > 1 && trailing.has(words[words.length - 1].toLowerCase())) words.pop();
  const name = words.join(" ").slice(0, 40).trim();
  return name ? name[0].toUpperCase() + name.slice(1) : "";
}

/** `name`, or `name 2`, `name 3`… when a project already carries it, so two cards never read the same. */
export function uniqueName(name: string, taken: readonly string[]): string {
  const used = new Set(taken.map((item) => item.trim().toLowerCase()));
  if (!used.has(name.trim().toLowerCase())) return name;
  let suffix = 2;
  while (used.has(`${name} ${suffix}`.toLowerCase())) suffix += 1;
  return `${name} ${suffix}`;
}

/**
 * The switcher's order: the Projects list's own, published apps first and the
 * most recently edited on top, so the two never disagree.
 */
export function listOrder(entries: readonly ProjectEntry[]): ProjectEntry[] {
  const { published, drafts } = groupProjects(entries, "", "edited");
  return [...published, ...drafts];
}

/**
 * The projects the switcher lists, in the list's order, each with its state.
 * An app edited in a folder that was never published has no page to open, so
 * it is left out.
 */
export function switcherProjects(projects: readonly MiniAppProject[], folders: readonly ProjectMetadata[], deployments: Readonly<Record<string, DeploymentReceipt>>): { id: string; name: string; published: boolean }[] {
  return listOrder(projectEntries(projects, folders, deployments))
    .filter((entry) => !entry.inFolder || entry.deployment)
    .map((entry) => ({ id: entry.id, name: entry.name, published: Boolean(entry.deployment) }));
}
