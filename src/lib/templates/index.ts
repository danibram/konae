import { withVisibility } from "../../deployments/manifest";
import { withPrivateNotes } from "../private-notes";
import type { MiniAppProject } from "../types";
import { defaultModuleStates } from "../app-folder";
import { sourcesUseActions, sourcesUseAuth } from "../module-use";
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
  const modules = defaultModuleStates(base.modules);
  const applyModules = (files: Record<string, string>, visibility: "public" | "private") => {
    const state = { ...modules };
    if (sourcesUseActions(files)) state.actions = { ...state.actions, on: true };
    else delete state.actions;
    if (Object.values(files).some((source) => /miniapp\s*\.\s*files/.test(source))) state.files = { ...state.files, on: true };
    else delete state.files;
    state.auth = { ...state.auth, on: visibility === "private" || sourcesUseAuth(Object.values(files)) };
    return state;
  };
  if (id === "tasks") return { ...base, modules: applyModules(base.files, "public"), manifest: withVisibility(base.manifest, "public") };
  if (id === "notes") {
    const notes = withPrivateNotes(base);
    return { ...notes, modules: applyModules(notes.files, "private"), manifest: withVisibility(notes.manifest, "private") };
  }
  const source = templateSource(id);
  if (!source) return base;
  return {
    ...base,
    directories: [],
    files: { ...source.files },
    modules: applyModules(source.files, source.visibility),
    manifest: withVisibility(base.manifest, source.visibility),
    updatedAt: Date.now(),
  };
}
