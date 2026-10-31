// Writes kernel-rs/src/project-templates.json from src/lib/templates.
// scripts/konae.test.ts fails when the checked-in file differs from this output.
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { PROJECT_TEMPLATES_FILE, renderProjectTemplates } from "./project-templates";

await writeFile(path.resolve(import.meta.dir, "..", PROJECT_TEMPLATES_FILE), renderProjectTemplates());
console.log(`wrote ${PROJECT_TEMPLATES_FILE}`);
