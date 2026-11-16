import { writeFile } from "node:fs/promises";
import path from "node:path";
import { renderAll } from "../src/sdk/render";

const root = path.resolve(import.meta.dir, "..");
for (const [file, contents] of Object.entries(renderAll())) {
  await writeFile(path.join(root, file), contents);
  console.log(`wrote ${file}`);
}
