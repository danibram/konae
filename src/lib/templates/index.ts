import { withVisibility } from "../../deployments/manifest";
import { parseServerActionSource } from "../../kernel/actions";
import { withPrivateNotes } from "../private-notes";
import type { MiniAppProject } from "../types";
import { digestTemplate } from "./digest";
import { formTemplate } from "./form";
import { galleryTemplate } from "./gallery";
import { membersTemplate } from "./members";
import type { TemplateSource } from "./types";
import { webhookTemplate } from "./webhook";

export type { TemplateSource } from "./types";

/** "tasks" is Tiny Tasks in src/lib/starter.ts and "notes" is src/lib/private-notes.ts. */
export type TemplateId = "tasks" | "notes" | "members" | "form" | "webhook" | "digest" | "gallery";

export type TemplateChoice = { id: TemplateId; name: string; sentence: string };

const sources = [membersTemplate, formTemplate, webhookTemplate, digestTemplate, galleryTemplate] as const;

/** The list the new-project dialog offers, in its order. The build agent's prompt names the same ids. */
export const templateChoices: readonly TemplateChoice[] = [
  { id: "tasks", name: "Shared list", sentence: "Anyone with the link adds and ticks off rows." },
  { id: "notes", name: "Private notes", sentence: "Each person signs in and sees only their own." },
  ...sources.map(({ id, name, sentence }) => ({ id: id as TemplateId, name, sentence })),
];

export function templateSource(id: TemplateId): TemplateSource | undefined {
  return sources.find((source) => source.id === id);
}

/**
 * The project `base` reborn as a template, keeping its id, name and slug. The
 * manifest's declarations come from the template's actions.json through the
 * parser both doors share, so the preview admits exactly the template's actions.
 */
export function applyTemplate(base: MiniAppProject, id: TemplateId): MiniAppProject {
  // The shared list is for anyone with the link, so it is born public; private notes are not.
  if (id === "tasks") return { ...base, manifest: withVisibility(base.manifest, "public") };
  if (id === "notes") {
    const notes = withPrivateNotes(base);
    return { ...notes, manifest: withVisibility(notes.manifest, "private") };
  }
  const source = templateSource(id);
  if (!source) return base;
  const program = parseServerActionSource(source.files["actions.json"]);
  const declarations = Object.values(program.actions).map((action) => ({ name: action.name, kind: action.kind, access: action.access }));
  return {
    ...base,
    directories: [],
    files: { ...source.files },
    databaseModel: { source: source.model, acceptedSource: source.model, revision: "baseline" },
    manifest: withVisibility({
      ...base.manifest,
      runtime: { ...base.manifest.runtime, database: { ...base.manifest.runtime.database, actions: declarations } },
    }, source.visibility),
    updatedAt: Date.now(),
  };
}
