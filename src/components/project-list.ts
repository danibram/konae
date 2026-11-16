import { releasedAt } from "../deployments/deployed-changes";
import type { DeploymentReceipt } from "../deployments/client";
import type { ProjectMetadata } from "../lib/project-sources";
import type { MiniAppProject } from "../lib/types";

export type ProjectEntry = {
  id: string;
  name: string;
  purpose: string;
  editedAt?: number;
  deployment?: DeploymentReceipt;
  sourceError?: boolean;
};

export type ProjectSort = "edited" | "name";

const entities: Record<string, string> = { amp: "&", lt: "<", gt: ">", quot: '"', "#39": "'", apos: "'" };

export function projectPurpose(project: MiniAppProject): string {
  const raw = project.files["index.html"]?.match(/data-purpose="([^"]+)"/)?.[1] ?? "";
  return raw.replace(/&(amp|lt|gt|quot|#39|apos);/g, (_, name: string) => entities[name]);
}

export function projectEntries(projects: readonly MiniAppProject[], folders: readonly ProjectMetadata[], deployments: Readonly<Record<string, DeploymentReceipt>>): ProjectEntry[] {
  return [
    ...projects.map((project) => ({ id: project.id, name: project.name, purpose: projectPurpose(project), editedAt: project.updatedAt, deployment: deployments[project.id] })),
    ...folders.filter((item) => !projects.some((project) => project.id === item.id))
      .map((item) => ({ id: item.id, name: item.name, purpose: "", deployment: deployments[item.id], sourceError: true })),
  ];
}

const touchedAt = (entry: ProjectEntry) => entry.editedAt ?? (entry.deployment ? releasedAt(entry.deployment) : 0);

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

const trailing = new Set(["a", "an", "and", "at", "by", "for", "from", "in", "into", "my", "of", "on", "or", "our", "the", "their", "to", "where", "which", "who", "with", "your"]);

export function nameFromDescription(description: string): string {
  const first = description.trim().split(/[.!?\n,;:]/)[0]?.trim() ?? "";
  const words = first.replace(/["“”«»]/g, "").replace(/^(an?|the)\s+/i, "").split(/\s+/).filter(Boolean).slice(0, 4);
  while (words.length > 1 && trailing.has(words[words.length - 1].toLowerCase())) words.pop();
  const name = words.join(" ").slice(0, 40).trim();
  return name ? name[0].toUpperCase() + name.slice(1) : "";
}

export function uniqueName(name: string, taken: readonly string[]): string {
  const used = new Set(taken.map((item) => item.trim().toLowerCase()));
  if (!used.has(name.trim().toLowerCase())) return name;
  let suffix = 2;
  while (used.has(`${name} ${suffix}`.toLowerCase())) suffix += 1;
  return `${name} ${suffix}`;
}

export function listOrder(entries: readonly ProjectEntry[]): ProjectEntry[] {
  const { published, drafts } = groupProjects(entries, "", "edited");
  return [...published, ...drafts];
}

export function switcherProjects(projects: readonly MiniAppProject[], folders: readonly ProjectMetadata[], deployments: Readonly<Record<string, DeploymentReceipt>>): { id: string; name: string; published: boolean }[] {
  return listOrder(projectEntries(projects, folders, deployments))
    .map((entry) => ({ id: entry.id, name: entry.name, published: Boolean(entry.deployment) }));
}
