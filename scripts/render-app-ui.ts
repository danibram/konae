// Writes src/lib/templates/app-ui.generated.ts, the design system as the stylesheet a published app carries.
import { writeFile } from "node:fs/promises";
import path from "node:path";
import { renderAppUi } from "./app-ui-render";

const file = "src/lib/templates/app-ui.generated.ts";
await writeFile(path.resolve(import.meta.dir, "..", file), renderAppUi());
console.log(`wrote ${file}`);
