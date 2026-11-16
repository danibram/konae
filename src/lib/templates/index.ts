import { withVisibility } from "../../deployments/manifest";
import { withPrivateNotes } from "../private-notes";
import type { MiniAppProject } from "../types";
import { digestTemplate } from "./digest";
import { formTemplate } from "./form";
import { galleryTemplate } from "./gallery";
import { membersTemplate } from "./members";
import type { TemplateSource } from "./types";
import { webhookTemplate } from "./webhook";

export type { TemplateSource } from "./types";

export type TemplateId = "tasks" | "notes" | "members" | "form" | "webhook" | "digest" | "gallery";

export type TemplateChoice = { id: TemplateId; name: string; sentence: string; visibility: "public" | "private" };

export const defaultTemplate: TemplateId = "notes";

const sources = [membersTemplate, formTemplate, webhookTemplate, digestTemplate, galleryTemplate] as const;

export const templateChoices: readonly TemplateChoice[] = [
  { id: "tasks", name: "Shared list", sentence: "Anyone with the link adds and ticks off rows.", visibility: "public" },
  { id: "notes", name: "Private notes", sentence: "Each person signs in and sees only their own.", visibility: "private" },
  ...sources.map(({ id, name, sentence, visibility }) => ({ id: id as TemplateId, name, sentence, visibility })),
];

export function templateSource(id: TemplateId): TemplateSource | undefined {
  return sources.find((source) => source.id === id);
}

export function applyTemplate(base: MiniAppProject, id: TemplateId): MiniAppProject {
  if (id === "tasks") return { ...base, manifest: withVisibility(base.manifest, "public") };
  if (id === "notes") {
    const notes = withPrivateNotes(base);
    return { ...notes, manifest: withVisibility(notes.manifest, "private") };
  }
  const source = templateSource(id);
  if (!source) return base;
  return {
    ...base,
    directories: [],
    files: { ...source.files },
    manifest: withVisibility(base.manifest, source.visibility),
    updatedAt: Date.now(),
  };
}
