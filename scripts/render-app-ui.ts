// Writes src/lib/templates/app-ui.generated.ts, the design system as the stylesheet a published app carries,
// and the same stylesheet for the installation's home and the docs site.
import { copyFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { renderAppUi, renderBundles } from "./app-ui-render";

const file = "src/lib/templates/app-ui.generated.ts";
await writeFile(path.resolve(import.meta.dir, "..", file), renderAppUi());
console.log(`wrote ${file}`);

// The installation's home is a Konae app in the editor's own variant. Its
// row-limit demo runs the same check the Gate runs, so it carries copies of
// those two files; src/lib/templates/app-ui.test.ts fails when they drift.
const home = "examples/home/styles.css";
await writeFile(path.resolve(import.meta.dir, "..", home), renderBundles().brutal);
console.log(`wrote ${home}`);
const docs = "examples/docs-site/public/styles.css";
await writeFile(path.resolve(import.meta.dir, "..", docs), renderBundles().brutal);
console.log(`wrote ${docs}`);
for (const file of ["query-bound.ts", "sql-tokens.ts"]) {
  await copyFile(path.resolve(import.meta.dir, "../src/lib", file), path.resolve(import.meta.dir, "../examples/home", file));
  console.log(`wrote examples/home/${file}`);
}
