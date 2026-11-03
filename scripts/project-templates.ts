// The Capsules a project can be born as, as one JSON document both doors
// read: `konae init` writes one into a folder and the Kernel's
// `project_create` returns one. They come from src/lib/templates, the list
// the editor's new-project dialog offers.
import { applyTemplate, templateChoices } from "../src/lib/templates";
import { starterProject } from "../src/lib/starter";

export const PROJECT_TEMPLATES_FILE = "kernel-rs/konae/src/project-templates.json";

/** The template a project gets when none is named: each person signs in and sees only their own rows. */
export const DEFAULT_TEMPLATE = "notes";

export type ProjectTemplates = {
  default: string;
  templates: Record<string, { name: string; sentence: string; visibility: "public" | "private"; files: Record<string, string> }>;
};

export function renderProjectTemplates(): string {
  const templates: ProjectTemplates["templates"] = {};
  for (const { id, name, sentence } of templateChoices) {
    const born = applyTemplate(structuredClone(starterProject), id);
    const files: Record<string, string> = {};
    // The Capsule's files, as a folder holds them.
    for (const [filename, source] of Object.entries(born.files).sort(([a], [b]) => a.localeCompare(b))) if (typeof source === "string") files[filename] = source;
    templates[id] = { name, sentence, visibility: born.manifest.visibility, files };
  }
  return `${JSON.stringify({ default: DEFAULT_TEMPLATE, templates } satisfies ProjectTemplates, null, 2)}\n`;
}
