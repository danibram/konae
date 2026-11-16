import { applyTemplate, templateChoices } from "../src/lib/templates";
import { starterProject } from "../src/lib/starter";

export const PROJECT_TEMPLATES_FILE = "kernel-rs/konae/src/project-templates.json";

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
    for (const [filename, source] of Object.entries(born.files).sort(([a], [b]) => a.localeCompare(b))) if (typeof source === "string") files[filename] = source;
    templates[id] = { name, sentence, visibility: born.manifest.visibility, files };
  }
  return `${JSON.stringify({ default: DEFAULT_TEMPLATE, templates } satisfies ProjectTemplates, null, 2)}\n`;
}
