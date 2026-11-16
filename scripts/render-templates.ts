import { writeFile } from "node:fs/promises";
import path from "node:path";
import { PROJECT_TEMPLATES_FILE, renderProjectTemplates } from "./project-templates";

await writeFile(path.resolve(import.meta.dir, "..", PROJECT_TEMPLATES_FILE), renderProjectTemplates());
console.log(`wrote ${PROJECT_TEMPLATES_FILE}`);
